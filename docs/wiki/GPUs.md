# GPUs

Multi-GPU setups (such as AMD Advantage laptops, Intel Arc systems, hybrid NVIDIA graphics) and external GPUs (Thunderbolt/USB4 eGPU docks).

## Power management

<sup>Since: next release</sup>

When nothing is using a secondary GPU for outputs or leases, niri suspends it.
This saves power and allows the device to reach ACPI `D3cold` on supported hardware.
Render offload clients (such as games run with `DRI_PRIME=1` or `__NV_PRIME_RENDER_OFFLOAD=1`) wake and use the GPU directly when needed.

When an output is connected or when waking the GPU up, niri initializes it in a background thread (waiting up to 10 seconds for the PCIe link to come up) so slow PCIe links or eGPU docks don't stall the compositor.

On NVIDIA GPUs, runtime suspend is skipped unless video memory preservation (`NVreg_PreserveVideoMemoryAllocations=1`) is enabled, as suspending without it invalidates GPU allocations.

If automatic suspend causes issues with hotplug, wakeup, or rendering, you can disable it:

```kdl
debug {
    disable-idle-drm-device-suspend
}
```
