//! The GPU device the game-data tests share. Creating a wgpu instance enumerates every
//! backend's drivers (Vulkan and DX12 on Windows); doing that from several test threads at
//! once corrupted the process heap now and then (STATUS_HEAP_CORRUPTION with the default
//! parallel test threads, never with one thread or one backend). One instance and device for
//! the whole test process avoids it; wgpu devices are safe to use from several threads.

use std::sync::OnceLock;

static DEVICE: OnceLock<Option<(wgpu::Device, wgpu::Queue, Vec<u32>)>> = OnceLock::new();

/// The shared headless device, created by the first test that asks (`None` without an
/// adapter).
pub fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    device_with_msaa().map(|(d, q, _)| (d, q))
}

/// [`device`] with the MSAA sample counts it supports for the 3D view.
pub fn device_with_msaa() -> Option<(wgpu::Device, wgpu::Queue, Vec<u32>)> {
    DEVICE
        .get_or_init(|| {
            let d = match std::env::var_os("HWC_TEST_LOW_POWER") {
                // The integrated GPU, where a window on the desktop's display may land.
                Some(_) => low_power(),
                None => hwc_render::headless_device_with_msaa().ok(),
            };
            if let Some((device, ..)) = &d {
                device.set_device_lost_callback(|reason, message| {
                    if reason != wgpu::DeviceLostReason::Destroyed {
                        eprintln!(
                            "test GPU lost ({reason:?}): {message}
the last frames drawn:
{}",
                            crate::draw_check::history()
                        );
                    }
                });
            }
            d
        })
        .clone()
}

fn low_power() -> Option<(wgpu::Device, wgpu::Queue, Vec<u32>)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        ..Default::default()
    }))
    .ok()?;
    eprintln!("test GPU: {:?}", adapter.get_info());
    let (device, queue) = hwc_render::request_device(&adapter).ok()?;
    let counts = hwc_render::quality::supported_sample_counts(&adapter, device.features());
    Some((device, queue, counts))
}

/// Lists the adapters (`--ignored --nocapture`).
#[test]
#[ignore]
fn list_adapters() {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    for a in pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())) {
        eprintln!("adapter: {:?}", a.get_info());
    }
}
