use std::fs::File;
use std::io::Read;
use std::path::Path;

/// ICC/VCGT parsing errors.
#[derive(Debug)]
pub enum VcgtError {
    Io(std::io::Error),
    FileTooLarge,
    HeaderTooShort,
    InvalidIccMagic,
    InvalidTagTable,
    TagNotFound,
    VcgtDataTooShort,
    UnsupportedGammaType(u32),
    UnsupportedChannels(u16),
    UnsupportedEntrySize(u16),
    TableTruncated,
    InvalidFormulaParameters,
}

impl std::fmt::Display for VcgtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error reading ICC profile: {err}"),
            Self::FileTooLarge => write!(f, "ICC profile file exceeds maximum allowed size (4 MiB)"),
            Self::HeaderTooShort => write!(f, "ICC profile is too short for a 128-byte header"),
            Self::InvalidIccMagic => write!(f, "Invalid ICC file signature (expected 'acsp')"),
            Self::InvalidTagTable => write!(f, "Invalid or truncated ICC tag table"),
            Self::TagNotFound => write!(f, "Tag 'vcgt' not found in ICC profile"),
            Self::VcgtDataTooShort => write!(f, "Tag 'vcgt' data is too short for header"),
            Self::UnsupportedGammaType(t) => {
                write!(f, "Unsupported VCGT gamma type: 0x{t:08x} (expected table 0x0)")
            }
            Self::UnsupportedChannels(c) => {
                write!(f, "Unsupported VCGT channel count: {c} (expected 3 for RGB)")
            }
            Self::UnsupportedEntrySize(s) => {
                write!(f, "Unsupported VCGT entry size: {s} bytes (expected 1 or 2)")
            }
            Self::TableTruncated => write!(f, "VCGT gamma table data is truncated"),
            Self::InvalidFormulaParameters => {
                write!(f, "Invalid or non-finite VCGT formula parameters")
            }
        }
    }
}

impl std::error::Error for VcgtError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for VcgtError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Parsed VCGT with 16-bit RGB ramps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vcgt {
    pub red: Vec<u16>,
    pub green: Vec<u16>,
    pub blue: Vec<u16>,
}

/// Max ICC size (4 MiB) to prevent OOM.
pub const MAX_ICC_SIZE: u64 = 4 * 1024 * 1024;

impl Vcgt {
    /// Parse VCGT from an ICC profile file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, VcgtError> {
        let file = File::open(path)?;

        // Fail fast on large files to avoid alloc.
        if let Ok(meta) = file.metadata() {
            if meta.is_file() && meta.len() > MAX_ICC_SIZE {
                return Err(VcgtError::FileTooLarge);
            }
        }

        // take() bounds reads from infinite sources.
        let mut data = Vec::with_capacity(8192);
        let bytes_read = file.take(MAX_ICC_SIZE + 1).read_to_end(&mut data)?;
        if bytes_read as u64 > MAX_ICC_SIZE {
            return Err(VcgtError::FileTooLarge);
        }

        Self::from_bytes(&data)
    }

    /// Parse VCGT from raw ICC profile bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self, VcgtError> {
        // Validate header size.
        if data.len() < 128 {
            return Err(VcgtError::HeaderTooShort);
        }

        // Check ICC signature.
        let magic = &data[36..40];
        if magic != b"acsp" {
            return Err(VcgtError::InvalidIccMagic);
        }

        // Read tag count.
        if data.len() < 132 {
            return Err(VcgtError::InvalidTagTable);
        }
        let tag_count = u32::from_be_bytes([data[128], data[129], data[130], data[131]]) as usize;

        let tag_table_end = tag_count
            .checked_mul(12)
            .and_then(|len| 132usize.checked_add(len))
            .ok_or(VcgtError::InvalidTagTable)?;
        if data.len() < tag_table_end {
            return Err(VcgtError::InvalidTagTable);
        }

        // Find 'vcgt' tag.
        const VCGT_SIGNATURE: [u8; 4] = *b"vcgt";
        let mut vcgt_offset_and_size = None;

        for i in 0..tag_count {
            let entry_offset = 132 + i * 12;
            let tag_sig = &data[entry_offset..entry_offset + 4];
            if tag_sig == VCGT_SIGNATURE {
                let offset = u32::from_be_bytes([
                    data[entry_offset + 4],
                    data[entry_offset + 5],
                    data[entry_offset + 6],
                    data[entry_offset + 7],
                ]) as usize;
                let size = u32::from_be_bytes([
                    data[entry_offset + 8],
                    data[entry_offset + 9],
                    data[entry_offset + 10],
                    data[entry_offset + 11],
                ]) as usize;
                vcgt_offset_and_size = Some((offset, size));
                break;
            }
        }

        let (vcgt_offset, vcgt_size) = vcgt_offset_and_size.ok_or(VcgtError::TagNotFound)?;

        let vcgt_end = vcgt_offset
            .checked_add(vcgt_size)
            .ok_or(VcgtError::VcgtDataTooShort)?;

        if vcgt_end > data.len() || vcgt_size < 18 {
            return Err(VcgtError::VcgtDataTooShort);
        }

        let vcgt_data = &data[vcgt_offset..vcgt_end];

        // Read gamma type (0 = Table, 1 = Formula).
        let gamma_type = u32::from_be_bytes([
            vcgt_data[8],
            vcgt_data[9],
            vcgt_data[10],
            vcgt_data[11],
        ]);

        match gamma_type {
            0 => Self::parse_table_type(vcgt_data),
            1 => Self::parse_formula_type(vcgt_data),
            other => Err(VcgtError::UnsupportedGammaType(other)),
        }
    }

    // Parse table gamma: verify 3 channels and dimensions.
    fn parse_table_type(vcgt_data: &[u8]) -> Result<Self, VcgtError> {
        if vcgt_data.len() < 18 {
            return Err(VcgtError::VcgtDataTooShort);
        }

        let channels = u16::from_be_bytes([vcgt_data[12], vcgt_data[13]]);
        let entry_count = u16::from_be_bytes([vcgt_data[14], vcgt_data[15]]) as usize;
        let entry_size = u16::from_be_bytes([vcgt_data[16], vcgt_data[17]]) as usize;

        if channels != 3 {
            return Err(VcgtError::UnsupportedChannels(channels));
        }

        if entry_count == 0 {
            return Err(VcgtError::TableTruncated);
        }

        if entry_size != 1 && entry_size != 2 {
            return Err(VcgtError::UnsupportedEntrySize(entry_size as u16));
        }

        let channel_byte_len = entry_count * entry_size;
        let total_required_len = 18 + 3 * channel_byte_len;
        if vcgt_data.len() < total_required_len {
            return Err(VcgtError::TableTruncated);
        }

        let table = &vcgt_data[18..];
        let r_bytes = &table[0..channel_byte_len];
        let g_bytes = &table[channel_byte_len..2 * channel_byte_len];
        let b_bytes = &table[2 * channel_byte_len..3 * channel_byte_len];

        let decode_channel = |slice: &[u8]| -> Vec<u16> {
            let mut out = Vec::with_capacity(entry_count);
            if entry_size == 1 {
                for &val in slice {
                    // * 257 scales 8-bit to 16-bit exactly.
                    out.push(u16::from(val) * 257);
                }
            } else {
                for chunk in slice.chunks_exact(2) {
                    out.push(u16::from_be_bytes([chunk[0], chunk[1]]));
                }
            }
            out
        };

        Ok(Self {
            red: decode_channel(r_bytes),
            green: decode_channel(g_bytes),
            blue: decode_channel(b_bytes),
        })
    }

    // Parse formula gamma parameters for each channel.
    fn parse_formula_type(vcgt_data: &[u8]) -> Result<Self, VcgtError> {
        // Header + 3 channels * 12 bytes.
        if vcgt_data.len() < 12 + 3 * 12 {
            return Err(VcgtError::VcgtDataTooShort);
        }

        let read_fixed = |slice: &[u8]| -> f64 {
            let int_val = i32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]);
            f64::from(int_val) / 65536.0
        };

        let evaluate_channel = |offset: usize| -> Result<Vec<u16>, VcgtError> {
            let gamma = read_fixed(&vcgt_data[offset..offset + 4]);
            let min = read_fixed(&vcgt_data[offset + 4..offset + 8]);
            let max = read_fixed(&vcgt_data[offset + 8..offset + 12]);

            // Reject invalid params to avoid NaN or singularities.
            if !gamma.is_finite() || !min.is_finite() || !max.is_finite() {
                return Err(VcgtError::InvalidFormulaParameters);
            }

            if gamma <= 0.0 || min > max {
                return Err(VcgtError::InvalidFormulaParameters);
            }

            let count = 256;
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                let x = i as f64 / (count - 1) as f64;
                let y = min + (max - min) * x.powf(gamma);

                // clamp passes NaN, so verify finiteness manually.
                if !y.is_finite() {
                    return Err(VcgtError::InvalidFormulaParameters);
                }

                let clamped = (y * 65535.0).round().clamp(0.0, 65535.0) as u16;
                out.push(clamped);
            }
            Ok(out)
        };

        Ok(Self {
            red: evaluate_channel(12)?,
            green: evaluate_channel(24)?,
            blue: evaluate_channel(36)?,
        })
    }

    /// Resample all three channels to target_size.
    pub fn resample(&self, target_size: usize) -> Self {
        Self {
            red: resample_vcgt(&self.red, target_size),
            green: resample_vcgt(&self.green, target_size),
            blue: resample_vcgt(&self.blue, target_size),
        }
    }

    /// Flatten channels into a single DRM gamma LUT vector.
    pub fn to_drm_lut(&self) -> Vec<u16> {
        let mut lut = Vec::with_capacity(self.red.len() + self.green.len() + self.blue.len());
        lut.extend_from_slice(&self.red);
        lut.extend_from_slice(&self.green);
        lut.extend_from_slice(&self.blue);
        lut
    }

    /// Number of entries per channel.
    pub fn len(&self) -> usize {
        self.red.len()
    }

    /// Check if tables are empty.
    pub fn is_empty(&self) -> bool {
        self.red.is_empty()
    }
}

/// Linearly resample a channel to target hardware LUT size.
pub fn resample_vcgt(channel: &[u16], target_size: usize) -> Vec<u16> {
    if channel.is_empty() || target_size == 0 {
        return Vec::new();
    }

    if channel.len() == 1 || target_size == 1 {
        return vec![channel[0]; target_size];
    }

    if channel.len() == target_size {
        return channel.to_vec();
    }

    let src_len = channel.len();
    let mut result = Vec::with_capacity(target_size);

    for i in 0..target_size {
        let pos = (i as f64 * (src_len - 1) as f64) / (target_size - 1) as f64;
        let idx = pos.floor() as usize;

        if idx >= src_len - 1 {
            result.push(channel[src_len - 1]);
        } else {
            let frac = pos - idx as f64;
            let val = (channel[idx] as f64) * (1.0 - frac) + (channel[idx + 1] as f64) * frac;
            let rounded = val.round().clamp(0.0, 65535.0) as u16;
            result.push(rounded);
        }
    }

    result
}

/// Fuse client gamma (night light) with base VCGT calibration.
pub fn fuse_gamma(base_vcgt: &[u16], client_lut: &[u16]) -> Vec<u16> {
    if base_vcgt.is_empty() {
        return client_lut.to_vec();
    }
    if client_lut.is_empty() {
        return base_vcgt.to_vec();
    }

    if base_vcgt.len() != client_lut.len() {
        // Resample base to match client LUT length if needed.
        let channels = 3;
        if client_lut.len().is_multiple_of(channels) && base_vcgt.len().is_multiple_of(channels) {
            let base_size = base_vcgt.len() / channels;
            let client_size = client_lut.len() / channels;
            if base_size > 0 && client_size > 0 {
                let base_r = resample_vcgt(&base_vcgt[0..base_size], client_size);
                let base_g = resample_vcgt(&base_vcgt[base_size..2 * base_size], client_size);
                let base_b = resample_vcgt(&base_vcgt[2 * base_size..3 * base_size], client_size);
                let mut resampled_base = Vec::with_capacity(client_lut.len());
                resampled_base.extend(base_r);
                resampled_base.extend(base_g);
                resampled_base.extend(base_b);
                return fuse_gamma_exact(&resampled_base, client_lut);
            }
        }
    }
    fuse_gamma_exact(base_vcgt, client_lut)
}

fn fuse_gamma_exact(base_vcgt: &[u16], client_lut: &[u16]) -> Vec<u16> {
    base_vcgt
        .iter()
        .zip(client_lut.iter())
        .map(|(&base, &client)| {
            ((f64::from(base) / 65535.0) * (f64::from(client) / 65535.0) * 65535.0)
                .round()
                .clamp(0.0, 65535.0) as u16
        })
        .collect()
}
