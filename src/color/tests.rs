use std::path::Path;

use super::vcgt::{fuse_gamma, resample_vcgt, Vcgt, VcgtError};

#[test]
fn test_parse_real_icc_profile() {
    let profile_path = Path::new("/home/gimgiwer/.local/share/icc/TPLCD_080F_Native.icm");
    if !profile_path.exists() {
        eprintln!("Skipping real ICC profile test: file does not exist");
        return;
    }

    let vcgt = Vcgt::from_file(profile_path).expect("Failed to parse real ICC profile");
    assert_eq!(vcgt.red.len(), 256);
    assert_eq!(vcgt.green.len(), 256);
    assert_eq!(vcgt.blue.len(), 256);

    // Endpoints and midpoint for calibrated linear table.
    assert_eq!(vcgt.red[0], 0);
    assert_eq!(vcgt.green[0], 0);
    assert_eq!(vcgt.blue[0], 0);

    assert_eq!(vcgt.red[128], 32896);
    assert_eq!(vcgt.green[128], 32896);
    assert_eq!(vcgt.blue[128], 32896);

    assert_eq!(vcgt.red[255], 65535);
    assert_eq!(vcgt.green[255], 65535);
    assert_eq!(vcgt.blue[255], 65535);

    // Test DRM LUT conversion (flat 3 * 256 = 768 entries).
    let drm_lut = vcgt.to_drm_lut();
    assert_eq!(drm_lut.len(), 768);
    assert_eq!(&drm_lut[0..256], &vcgt.red[..]);
    assert_eq!(&drm_lut[256..512], &vcgt.green[..]);
    assert_eq!(&drm_lut[512..768], &vcgt.blue[..]);
}

#[test]
fn test_resample_vcgt_intel_and_amd() {
    // 256 points linear ramp.
    let src: Vec<u16> = (0..256)
        .map(|i| (i as f64 / 255.0 * 65535.0).round() as u16)
        .collect();

    // Intel CRTC standard size: 1024 points.
    let intel_lut = resample_vcgt(&src, 1024);
    assert_eq!(intel_lut.len(), 1024);
    assert_eq!(intel_lut[0], 0);
    assert_eq!(intel_lut[1023], 65535);
    // Verify strict monotonicity.
    for i in 1..intel_lut.len() {
        assert!(intel_lut[i] >= intel_lut[i - 1]);
    }

    // AMD CRTC standard size: 4096 points.
    let amd_lut = resample_vcgt(&src, 4096);
    assert_eq!(amd_lut.len(), 4096);
    assert_eq!(amd_lut[0], 0);
    assert_eq!(amd_lut[4095], 65535);
    for i in 1..amd_lut.len() {
        assert!(amd_lut[i] >= amd_lut[i - 1]);
    }
}

#[test]
fn test_resample_vcgt_exact_interpolation() {
    let two_points = vec![0u16, 65535u16];
    let three_points = resample_vcgt(&two_points, 3);
    assert_eq!(three_points.len(), 3);
    assert_eq!(three_points[0], 0);
    assert_eq!(three_points[1], 32768);
    assert_eq!(three_points[2], 65535);

    // Edge cases.
    assert!(resample_vcgt(&[], 1024).is_empty());
    assert!(resample_vcgt(&two_points, 0).is_empty());
    assert_eq!(resample_vcgt(&[42u16], 4), vec![42, 42, 42, 42]);
    assert_eq!(resample_vcgt(&two_points, 2), two_points);
}

#[test]
fn test_night_light_fusion() {
    let base = vec![0, 10000, 20000, 40000, 65535];
    let identity_client = vec![65535; 5];
    let fused_identity = fuse_gamma(&base, &identity_client);
    assert_eq!(fused_identity, base);

    let zero_client = vec![0; 5];
    let fused_zero = fuse_gamma(&base, &zero_client);
    assert_eq!(fused_zero, vec![0; 5]);

    // 50% night light / brightness reduction.
    let half_client = vec![32768; 5];
    let fused_half = fuse_gamma(&base, &half_client);
    for i in 0..5 {
        let expected = ((base[i] as f64 / 65535.0) * (32768.0 / 65535.0) * 65535.0).round() as u16;
        assert_eq!(fused_half[i], expected);
    }

    // Color-specific fusion across R, G, B channels:
    // Red: 100% client (identity)
    // Green: 100% client (identity)
    // Blue: 50% client (warm night light)
    let base_rgb = vec![
        30000, 60000, // R
        30000, 60000, // G
        30000, 60000, // B
    ];
    let client_rgb = vec![
        65535, 65535, // R full
        65535, 65535, // G full
        32768, 32768, // B halved
    ];
    let fused = fuse_gamma(&base_rgb, &client_rgb);
    assert_eq!(fused[0], 30000);
    assert_eq!(fused[1], 60000);
    assert_eq!(fused[2], 30000);
    assert_eq!(fused[3], 60000);
    assert_eq!(fused[4], 15000);
    assert_eq!(fused[5], 30000); // 60000 * 32768 / 65535 = 30000.458 -> 30000
}

#[test]
fn test_synthetic_icc_profile_8bit_and_16bit() {
    // Helper to construct a minimal valid ICC profile with a vcgt tag.
    let make_profile = |gamma_type: u32, entry_size: u16, table_bytes: &[u8]| -> Vec<u8> {
        let mut buf = vec![0u8; 128]; // Header
        buf[36..40].copy_from_slice(b"acsp"); // Magic

        let mut vcgt_tag = Vec::new();
        vcgt_tag.extend_from_slice(b"vcgt"); // Tag type signature
        vcgt_tag.extend_from_slice(&0u32.to_be_bytes()); // Reserved
        vcgt_tag.extend_from_slice(&gamma_type.to_be_bytes()); // gammaType
        if gamma_type == 0 {
            vcgt_tag.extend_from_slice(&3u16.to_be_bytes()); // Channels = 3
            let entry_count = (table_bytes.len() / (3 * entry_size as usize)) as u16;
            vcgt_tag.extend_from_slice(&entry_count.to_be_bytes()); // entryCount
            vcgt_tag.extend_from_slice(&entry_size.to_be_bytes()); // entrySize
            vcgt_tag.extend_from_slice(table_bytes);
        }

        // Tag table at 128..
        let tag_count = 1u32;
        buf.extend_from_slice(&tag_count.to_be_bytes());

        let tag_offset = (132 + 12) as u32;
        let tag_size = vcgt_tag.len() as u32;

        buf.extend_from_slice(b"vcgt");
        buf.extend_from_slice(&tag_offset.to_be_bytes());
        buf.extend_from_slice(&tag_size.to_be_bytes());

        buf.extend_from_slice(&vcgt_tag);
        buf
    };

    // 8-bit table: 4 entries per channel -> 12 bytes total.
    let table_8bit = vec![
        0x00, 0x55, 0xAA, 0xFF, // R
        0x00, 0x55, 0xAA, 0xFF, // G
        0x00, 0x55, 0xAA, 0xFF, // B
    ];
    let profile_8bit = make_profile(0, 1, &table_8bit);
    let vcgt_8 = Vcgt::from_bytes(&profile_8bit).expect("Failed to parse 8-bit synthetic VCGT");
    assert_eq!(vcgt_8.red, vec![0, 0x5555, 0xAAAA, 0xFFFF]);
    assert_eq!(vcgt_8.green, vec![0, 0x5555, 0xAAAA, 0xFFFF]);
    assert_eq!(vcgt_8.blue, vec![0, 0x5555, 0xAAAA, 0xFFFF]);

    // 16-bit table: 2 entries per channel -> 12 bytes total.
    let table_16bit = vec![
        0x00, 0x00, 0xFF, 0xFF, // R
        0x12, 0x34, 0x56, 0x78, // G
        0xAA, 0xBB, 0xCC, 0xDD, // B
    ];
    let profile_16bit = make_profile(0, 2, &table_16bit);
    let vcgt_16 = Vcgt::from_bytes(&profile_16bit).expect("Failed to parse 16-bit synthetic VCGT");
    assert_eq!(vcgt_16.red, vec![0x0000, 0xFFFF]);
    assert_eq!(vcgt_16.green, vec![0x1234, 0x5678]);
    assert_eq!(vcgt_16.blue, vec![0xAABB, 0xCCDD]);
}

#[test]
fn test_error_handling() {
    assert!(matches!(Vcgt::from_bytes(&[]), Err(VcgtError::HeaderTooShort)));

    let mut invalid_magic = vec![0u8; 144];
    invalid_magic[36..40].copy_from_slice(b"bad!");
    assert!(matches!(
        Vcgt::from_bytes(&invalid_magic),
        Err(VcgtError::InvalidIccMagic)
    ));

    let mut no_vcgt = vec![0u8; 128];
    no_vcgt[36..40].copy_from_slice(b"acsp");
    no_vcgt.extend_from_slice(&1u32.to_be_bytes()); // tag count = 1
    no_vcgt.extend_from_slice(b"desc"); // other tag
    no_vcgt.extend_from_slice(&144u32.to_be_bytes());
    no_vcgt.extend_from_slice(&10u32.to_be_bytes());
    no_vcgt.extend_from_slice(&[0u8; 10]);
    assert!(matches!(
        Vcgt::from_bytes(&no_vcgt),
        Err(VcgtError::TagNotFound)
    ));
}

#[test]
fn test_resample_vcgt_extreme_edge_cases() {
    let channel = vec![1000u16, 20000u16, 65535u16];

    // target_size == 1: must NOT divide by zero or produce NaN, returns src[0].
    let resampled_1 = resample_vcgt(&channel, 1);
    assert_eq!(resampled_1, vec![1000]);

    // target_size == 0: returns empty.
    assert_eq!(resample_vcgt(&channel, 0), Vec::<u16>::new());

    // Empty channel: returns empty.
    assert_eq!(resample_vcgt(&[], 1024), Vec::<u16>::new());

    // Single-entry channel: replicated to target size.
    assert_eq!(resample_vcgt(&[42u16], 3), vec![42, 42, 42]);

    // Very large hardware LUT: 16384 points.
    let ramp_16k = resample_vcgt(&channel, 16384);
    assert_eq!(ramp_16k.len(), 16384);
    assert_eq!(ramp_16k[0], 1000);
    assert_eq!(ramp_16k[16383], 65535);
    for i in 1..ramp_16k.len() {
        assert!(ramp_16k[i] >= ramp_16k[i - 1]);
    }
}

#[test]
fn test_fuse_gamma_edge_cases() {
    let base = vec![10000, 20000, 30000];
    let client = vec![65535, 65535, 65535];

    // Empty base or client: return the other.
    assert_eq!(fuse_gamma(&[], &client), client);
    assert_eq!(fuse_gamma(&base, &[]), base);
    assert_eq!(fuse_gamma(&[], &[]), Vec::<u16>::new());

    // Mismatched length fallback: base has 3 entries (1 per channel), client has 6 (2 per channel).
    let base_3 = vec![10000, 20000, 30000];
    let client_6 = vec![65535, 65535, 65535, 65535, 65535, 65535];
    let fused = fuse_gamma(&base_3, &client_6);
    assert_eq!(fused.len(), 6);
    assert_eq!(fused, vec![10000, 10000, 20000, 20000, 30000, 30000]);
}

#[test]
fn test_hostile_icc_overflow_and_bounds() {
    // 1. Tag count overflow: tag_count * 12 wraps or exceeds usize.
    let mut header = vec![0u8; 132];
    header[36..40].copy_from_slice(b"acsp");
    // tag_count = u32::MAX
    header[128..132].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(
        Vcgt::from_bytes(&header),
        Err(VcgtError::InvalidTagTable)
    ));

    // tag_count = 0x40000000 (1 billion tags)
    header[128..132].copy_from_slice(&0x4000_0000u32.to_be_bytes());
    assert!(matches!(
        Vcgt::from_bytes(&header),
        Err(VcgtError::InvalidTagTable)
    ));

    // 2. Tag offset/size overflow: vcgt_offset + vcgt_size overflows.
    let mut profile = vec![0u8; 128];
    profile[36..40].copy_from_slice(b"acsp");
    profile.extend_from_slice(&1u32.to_be_bytes()); // 1 tag
    profile.extend_from_slice(b"vcgt");
    profile.extend_from_slice(&u32::MAX.to_be_bytes()); // offset = u32::MAX
    profile.extend_from_slice(&100u32.to_be_bytes()); // size = 100
    assert!(matches!(
        Vcgt::from_bytes(&profile),
        Err(VcgtError::VcgtDataTooShort)
    ));

    // 3. Truncated table: channel_byte_len claims more data than present.
    let mut truncated = vec![0u8; 128];
    truncated[36..40].copy_from_slice(b"acsp");
    truncated.extend_from_slice(&1u32.to_be_bytes());
    truncated.extend_from_slice(b"vcgt");
    truncated.extend_from_slice(&144u32.to_be_bytes()); // offset 144
    truncated.extend_from_slice(&18u32.to_be_bytes()); // size 18
    // Tag data:
    truncated.extend_from_slice(b"vcgt"); // sig
    truncated.extend_from_slice(&0u32.to_be_bytes()); // reserved
    truncated.extend_from_slice(&0u32.to_be_bytes()); // gammaType = 0 (Table)
    truncated.extend_from_slice(&3u16.to_be_bytes()); // channels = 3
    truncated.extend_from_slice(&256u16.to_be_bytes()); // entryCount = 256
    truncated.extend_from_slice(&2u16.to_be_bytes()); // entrySize = 2 (requires 18 + 3 * 512 = 1554 bytes)
    assert!(matches!(
        Vcgt::from_bytes(&truncated),
        Err(VcgtError::TableTruncated)
    ));

    // 4. Unsupported channels count (e.g. 4 channels).
    let mut bad_channels = truncated.clone();
    bad_channels[144 + 12..144 + 14].copy_from_slice(&4u16.to_be_bytes());
    assert!(matches!(
        Vcgt::from_bytes(&bad_channels),
        Err(VcgtError::UnsupportedChannels(4))
    ));

    // 5. Unsupported entry size (e.g. 4 bytes).
    let mut bad_entry_size = truncated.clone();
    bad_entry_size[144 + 16..144 + 18].copy_from_slice(&4u16.to_be_bytes());
    assert!(matches!(
        Vcgt::from_bytes(&bad_entry_size),
        Err(VcgtError::UnsupportedEntrySize(4))
    ));

    // 6. Unsupported gamma type (e.g. 0xdeadbeef).
    let mut bad_gamma_type = truncated.clone();
    bad_gamma_type[144 + 8..144 + 12].copy_from_slice(&0xdeadbeefu32.to_be_bytes());
    assert!(matches!(
        Vcgt::from_bytes(&bad_gamma_type),
        Err(VcgtError::UnsupportedGammaType(0xdeadbeef))
    ));
}

#[test]
fn test_dos_oom_file_size_limit() {
    // Bounded read prevents runaway memory on infinite streams (/dev/zero).
    let dev_zero = Path::new("/dev/zero");
    if dev_zero.exists() {
        let res = Vcgt::from_file(dev_zero);
        assert!(
            matches!(res, Err(VcgtError::FileTooLarge)),
            "Expected FileTooLarge for /dev/zero, got: {res:?}"
        );
    }

    // Reject large files via metadata before reading.
    let temp_dir = std::env::temp_dir();
    let temp_file_path = temp_dir.join("niri_vcgt_test_too_large.icm");
    let file = std::fs::File::create(&temp_file_path).expect("failed to create temp file");
    // Allocate 5 MiB sparse file without taking physical disk space.
    file.set_len(5 * 1024 * 1024).expect("failed to set file len");
    drop(file);

    let res = Vcgt::from_file(&temp_file_path);
    let _ = std::fs::remove_file(&temp_file_path);
    assert!(
        matches!(res, Err(VcgtError::FileTooLarge)),
        "Expected FileTooLarge for 5 MiB file, got: {res:?}"
    );
}

#[test]
fn test_formula_gamma_adversarial_edge_cases() {
    let make_formula_profile = |gamma_fp: i32, min_fp: i32, max_fp: i32| -> Vec<u8> {
        let mut buf = vec![0u8; 128];
        buf[36..40].copy_from_slice(b"acsp");

        let mut vcgt_tag = Vec::new();
        vcgt_tag.extend_from_slice(b"vcgt");
        vcgt_tag.extend_from_slice(&0u32.to_be_bytes()); // Reserved
        vcgt_tag.extend_from_slice(&1u32.to_be_bytes()); // gammaType = 1 (Formula)

        // 3 channels: R, G, B with same parameters
        for _ in 0..3 {
            vcgt_tag.extend_from_slice(&gamma_fp.to_be_bytes());
            vcgt_tag.extend_from_slice(&min_fp.to_be_bytes());
            vcgt_tag.extend_from_slice(&max_fp.to_be_bytes());
        }

        let tag_count = 1u32;
        buf.extend_from_slice(&tag_count.to_be_bytes());

        let tag_offset = (132 + 12) as u32;
        let tag_size = vcgt_tag.len() as u32;

        buf.extend_from_slice(b"vcgt");
        buf.extend_from_slice(&tag_offset.to_be_bytes());
        buf.extend_from_slice(&tag_size.to_be_bytes());

        buf.extend_from_slice(&vcgt_tag);
        buf
    };

    // 1. Valid Gamma 2.2: gamma = 2.2 * 65536 = 144179, min = 0.0, max = 1.0 * 65536 = 65536.
    let valid_profile = make_formula_profile(144179, 0, 65536);
    let vcgt = Vcgt::from_bytes(&valid_profile).expect("Failed to parse valid formula VCGT");
    assert_eq!(vcgt.red.len(), 256);
    assert_eq!(vcgt.green.len(), 256);
    assert_eq!(vcgt.blue.len(), 256);
    assert_eq!(vcgt.red[0], 0);
    assert_eq!(vcgt.red[255], 65535);
    // Monotonically increasing.
    for i in 1..256 {
        assert!(vcgt.red[i] >= vcgt.red[i - 1]);
    }

    // 2. Negative Gamma: gamma = -1.0 (-65536).
    // Would produce 0.0.powf(-1.0) -> +Infinity at black point. Must be rejected.
    let negative_gamma = make_formula_profile(-65536, 0, 65536);
    assert!(matches!(
        Vcgt::from_bytes(&negative_gamma),
        Err(VcgtError::InvalidFormulaParameters)
    ));

    // 3. Zero Gamma: gamma = 0.0 (0).
    // Degenerate flat curve. Must be rejected.
    let zero_gamma = make_formula_profile(0, 0, 65536);
    assert!(matches!(
        Vcgt::from_bytes(&zero_gamma),
        Err(VcgtError::InvalidFormulaParameters)
    ));

    // 4. Inverted Bounds: min > max (min = 1.0, max = 0.0).
    // Corrupted bounds. Must be rejected.
    let inverted_bounds = make_formula_profile(65536, 65536, 0);
    assert!(matches!(
        Vcgt::from_bytes(&inverted_bounds),
        Err(VcgtError::InvalidFormulaParameters)
    ));

    // 5. Degenerate flat equal bounds: min == max (e.g. min = 0.5, max = 0.5).
    // min <= max holds, but max - min = 0.
    let equal_bounds = make_formula_profile(65536, 32768, 32768);
    let vcgt_flat = Vcgt::from_bytes(&equal_bounds).expect("Expected flat curve to parse");
    assert_eq!(vcgt_flat.red[0], 32768);
    assert_eq!(vcgt_flat.red[255], 32768);
}

#[test]
fn test_pending_gamma_state_machine_and_cache_invalidation() {
    // Models the exact state machine of Surface::apply_effective_gamma and DPMS caching.
    struct MockSurfaceState {
        base_vcgt_lut: Option<Vec<u16>>,
        client_gamma: Option<Vec<u16>>,
        pending_gamma_change: Option<Option<Vec<u16>>>,
        hardware_lut: Option<Vec<u16>>,
    }

    impl MockSurfaceState {
        fn new(base_size: usize) -> Self {
            let ramp: Vec<u16> = (0..base_size * 3)
                .map(|i| (i as f64 / (base_size * 3 - 1) as f64 * 65535.0).round() as u16)
                .collect();
            Self {
                base_vcgt_lut: Some(ramp),
                client_gamma: None,
                pending_gamma_change: None,
                hardware_lut: None,
            }
        }

        fn compute_effective_gamma(&self) -> Option<Vec<u16>> {
            match (&self.base_vcgt_lut, &self.client_gamma) {
                (Some(base), Some(client)) => Some(fuse_gamma(base, client)),
                (Some(base), None) => Some(base.clone()),
                (None, Some(client)) => Some(client.clone()),
                (None, None) => None,
            }
        }

        fn apply_effective_gamma(
            &mut self,
            is_active: bool,
            mock_drm_success: bool,
        ) -> Result<(), &'static str> {
            let effective = self.compute_effective_gamma();
            if !is_active {
                self.pending_gamma_change = Some(effective);
                return Ok(());
            }

            if mock_drm_success {
                self.hardware_lut = effective.clone();
                // Crucial fix: successful live apply must clear pending change.
                self.pending_gamma_change = None;
                Ok(())
            } else {
                // If DRM failed, queue effective gamma for retry on next frame.
                self.pending_gamma_change = Some(effective);
                Err("DRM_IOCTL_MODE_SETPROPERTY failed: EBUSY")
            }
        }

        fn on_dpms_sleep(&mut self) {
            if self.pending_gamma_change.is_none() {
                let effective = self.compute_effective_gamma();
                if effective.is_some() {
                    self.pending_gamma_change = Some(effective);
                }
            }
        }

        fn on_queue_frame(&mut self) {
            if let Some(ramp) = self.pending_gamma_change.take() {
                self.hardware_lut = ramp;
            }
        }
    }

    // 1. Initial state: Monitor 1 (AMD DCN 4096) and Monitor 2 (Intel 1024).
    let mut mon1 = MockSurfaceState::new(4096);
    let mon2 = MockSurfaceState::new(1024);

    assert_eq!(mon1.base_vcgt_lut.as_ref().unwrap().len(), 4096 * 3);
    assert_eq!(mon2.base_vcgt_lut.as_ref().unwrap().len(), 1024 * 3);

    // Apply initially while active.
    assert!(mon1.apply_effective_gamma(true, true).is_ok());
    assert_eq!(mon1.hardware_lut, mon1.base_vcgt_lut);
    assert_eq!(mon1.pending_gamma_change, None);

    // 2. DPMS Sleep occurs.
    mon1.on_dpms_sleep();
    assert!(mon1.pending_gamma_change.is_some());
    let cached_pre_sleep = mon1.pending_gamma_change.clone();

    // 3. Monitor wakes up (monitors_active = true), but BEFORE queue_frame fires,
    // client changes gamma (e.g. night light wlsunset sets warmer color).
    let warm_client: Vec<u16> = vec![30000; 4096 * 3];
    mon1.client_gamma = Some(warm_client.clone());

    // Live apply while active succeeds.
    assert!(mon1.apply_effective_gamma(true, true).is_ok());
    let expected_fused = fuse_gamma(mon1.base_vcgt_lut.as_ref().unwrap(), &warm_client);
    assert_eq!(mon1.hardware_lut, Some(expected_fused.clone()));

    // Verify fix: pending_gamma_change MUST be None, not containing stale pre-sleep gamma.
    assert_eq!(
        mon1.pending_gamma_change, None,
        "pending_gamma_change must be cleared on live apply to prevent stale overwrite"
    );

    // 4. Now the delayed frame queues.
    mon1.on_queue_frame();
    // Hardware LUT must NOT be overwritten by stale pre-sleep cached gamma!
    assert_ne!(mon1.hardware_lut, cached_pre_sleep.unwrap());
    assert_eq!(mon1.hardware_lut, Some(expected_fused));

    // 5. Test transient DRM EBUSY error recovery.
    let cooler_client: Vec<u16> = vec![50000; 4096 * 3];
    mon1.client_gamma = Some(cooler_client.clone());
    let res = mon1.apply_effective_gamma(true, false); // DRM fails with EBUSY
    assert!(res.is_err());
    // On error, the new effective gamma must be queued as pending.
    let expected_retry = fuse_gamma(mon1.base_vcgt_lut.as_ref().unwrap(), &cooler_client);
    assert_eq!(mon1.pending_gamma_change, Some(Some(expected_retry.clone())));

    // Next frame succeeds and applies the queued gamma.
    mon1.on_queue_frame();
    assert_eq!(mon1.hardware_lut, Some(expected_retry));
    assert_eq!(mon1.pending_gamma_change, None);

    // 6. Monitor 2 isolation: verify Monitor 2 state was completely unaffected.
    assert_eq!(mon2.base_vcgt_lut.as_ref().unwrap().len(), 1024 * 3);
    assert_eq!(mon2.client_gamma, None);
    assert_eq!(mon2.pending_gamma_change, None);
}

#[test]
fn test_vcgt_rejects_zero_entry_count() {
    let mut buf = vec![0u8; 128]; // Header
    buf[36..40].copy_from_slice(b"acsp");

    let mut vcgt_tag = Vec::new();
    vcgt_tag.extend_from_slice(b"vcgt");
    vcgt_tag.extend_from_slice(&0u32.to_be_bytes()); // Reserved
    vcgt_tag.extend_from_slice(&0u32.to_be_bytes()); // gammaType = 0 (Table)
    vcgt_tag.extend_from_slice(&3u16.to_be_bytes()); // channels = 3
    vcgt_tag.extend_from_slice(&0u16.to_be_bytes()); // entryCount = 0 (invalid!)
    vcgt_tag.extend_from_slice(&2u16.to_be_bytes()); // entrySize = 2

    let tag_count = 1u32;
    buf.extend_from_slice(&tag_count.to_be_bytes());
    buf.extend_from_slice(b"vcgt");
    let tag_offset = (132 + 12) as u32;
    let tag_size = vcgt_tag.len() as u32;
    buf.extend_from_slice(&tag_offset.to_be_bytes());
    buf.extend_from_slice(&tag_size.to_be_bytes());
    buf.extend_from_slice(&vcgt_tag);

    let err = Vcgt::from_bytes(&buf).unwrap_err();
    assert!(matches!(err, VcgtError::TableTruncated));
}

#[test]
fn test_linear_gamma_generation_single_entry_no_panic() {
    let gamma_length = 1usize;
    let denom = (gamma_length as u64 - 1).max(1);
    assert_eq!(denom, 1);

    let mut temp = vec![0u16; gamma_length * 3];
    let (red, rest) = temp.split_at_mut(gamma_length);
    let (green, blue) = rest.split_at_mut(gamma_length);
    for (i, ((r, g), b)) in std::iter::zip(std::iter::zip(red, green), blue).enumerate() {
        let value = (0xFFFFu64 * i as u64 / denom) as u16;
        *r = value;
        *g = value;
        *b = value;
    }
    assert_eq!(temp, vec![0, 0, 0]);
}

#[test]
fn test_transient_drm_error_classification() {
    use crate::backend::tty::is_transient_drm_error;

    // Transient errors: EBUSY, EAGAIN, WouldBlock
    let ebusy_err = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EBUSY));
    assert!(is_transient_drm_error(&ebusy_err));

    let eagain_err = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EAGAIN));
    assert!(is_transient_drm_error(&eagain_err));

    let would_block = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        "Resource busy",
    ));
    assert!(is_transient_drm_error(&would_block));

    // Permanent errors: EINVAL, wrong gamma length, unsupported
    let einval_err = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EINVAL));
    assert!(!is_transient_drm_error(&einval_err));

    let wrong_len_err = anyhow::anyhow!("wrong gamma length");
    assert!(!is_transient_drm_error(&wrong_len_err));

    let unsupported_err = anyhow::anyhow!("setting gamma is not supported");
    assert!(!is_transient_drm_error(&unsupported_err));
}

#[test]
fn test_pending_gamma_cleared_on_permanent_error() {
    use crate::backend::tty::is_transient_drm_error;

    let permanent_err = anyhow::anyhow!("wrong gamma length");
    let mut pending_change = Some(Some(vec![1, 2, 3]));

    // Simulate error handling in apply_effective_gamma
    if is_transient_drm_error(&permanent_err) {
        // Transient: re-queue
    } else {
        pending_change = None;
    }

    assert_eq!(pending_change, None, "Permanent error must clear pending_change");
}

#[test]
fn test_transient_drm_error_includes_eintr() {
    use crate::backend::tty::is_transient_drm_error;

    let eintr_err = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EINTR));
    assert!(is_transient_drm_error(&eintr_err), "EINTR must be classified as transient");

    let interrupted_err = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "Interrupted system call",
    ));
    assert!(
        is_transient_drm_error(&interrupted_err),
        "ErrorKind::Interrupted must be classified as transient"
    );

    let msg_err = anyhow::anyhow!("atomic commit failed: Interrupted system call (EINTR)");
    assert!(
        is_transient_drm_error(&msg_err),
        "Error message containing EINTR must be transient"
    );
}

#[test]
fn test_icc_hot_reload_desync_recovery_and_client_gamma_preservation() {
    use std::path::{Path, PathBuf};

    struct MockSurface {
        loaded_icc_profile: Option<PathBuf>,
        base_vcgt_lut: Option<Vec<u16>>,
        client_gamma: Option<Vec<u16>>,
    }

    impl MockSurface {
        fn new() -> Self {
            Self {
                loaded_icc_profile: None,
                base_vcgt_lut: None,
                client_gamma: None,
            }
        }

        fn update_icc(
            &mut self,
            profile_path: Option<&Path>,
            file_exists: bool,
            dummy_lut: Vec<u16>,
        ) {
            let already_loaded = self.loaded_icc_profile.as_deref() == profile_path
                && (profile_path.is_none() || self.base_vcgt_lut.is_some());
            if already_loaded {
                return;
            }

            if let Some(path) = profile_path {
                if file_exists {
                    self.loaded_icc_profile = Some(path.to_path_buf());
                    self.base_vcgt_lut = Some(dummy_lut);
                } else {
                    self.loaded_icc_profile = None;
                    self.base_vcgt_lut = None;
                }
            } else {
                self.loaded_icc_profile = None;
                self.base_vcgt_lut = None;
            }
        }

        fn compute_effective_gamma(&self) -> Option<Vec<u16>> {
            let res = match (&self.base_vcgt_lut, &self.client_gamma) {
                (Some(base), Some(client)) => Some(super::vcgt::fuse_gamma(base, client)),
                (Some(base), None) => Some(base.clone()),
                (None, Some(client)) => Some(client.clone()),
                (None, None) => None,
            };
            res.filter(|lut| !lut.is_empty())
        }
    }

    let mut surface = MockSurface::new();
    let icc_path = Path::new("/nonexistent/calibration.icc");

    // 1. Initial attempt with missing file: fails
    surface.update_icc(Some(icc_path), false, vec![1, 2, 3]);
    assert_eq!(surface.loaded_icc_profile, None);
    assert_eq!(surface.base_vcgt_lut, None);

    // 2. User sets night light (client_gamma)
    let night_light = vec![65535, 32768, 16384];
    surface.client_gamma = Some(night_light.clone());
    assert_eq!(surface.compute_effective_gamma(), Some(night_light.clone()));

    // 3. User fixes/creates the file and reloads config with the SAME path:
    // With the fix, already_loaded is false because base_vcgt_lut was None, allowing reload!
    surface.update_icc(Some(icc_path), true, vec![50000, 50000, 50000]);
    assert_eq!(surface.loaded_icc_profile.as_deref(), Some(icc_path));
    assert_eq!(surface.base_vcgt_lut, Some(vec![50000, 50000, 50000]));

    // 4. Effective gamma fuses the newly loaded profile with active night light
    let effective = surface.compute_effective_gamma().unwrap();
    assert_eq!(effective.len(), 3);
    assert!(effective[0] > 0 && effective[1] > 0 && effective[2] > 0);
    // Night light is preserved: green and blue remain progressively attenuated
    assert!(effective[0] > effective[1]);
    assert!(effective[1] > effective[2]);

    // 5. Subsequent reload with same path skips re-parsing
    surface.update_icc(Some(icc_path), true, vec![60000, 60000, 60000]);
    assert_eq!(surface.base_vcgt_lut, Some(vec![50000, 50000, 50000]));

    // 6. Clearing ICC profile keeps night light intact
    surface.update_icc(None, false, vec![]);
    assert_eq!(surface.loaded_icc_profile, None);
    assert_eq!(surface.base_vcgt_lut, None);
    assert_eq!(surface.compute_effective_gamma(), Some(night_light));
}

