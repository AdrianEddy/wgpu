//! The swapchain's teardown waits for the presenting queue, not the device.

use alloc::vec::Vec;
use core::time::Duration;

use super::calls::{self, Kind};
use crate::{Device as _, Queue as _, Surface as _};

/// Configures a headless surface, presents to it, configures it again, discards a frame and
/// unconfigures it, on each adapter. Each teardown waits for the device's queue to become idle,
/// after its last present, and nothing waits for the whole device.
#[test]
fn the_teardown_waits_for_the_presenting_queue_not_the_device() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    for flags in super::FLAGS {
        std::println!("{flags:?}");
        let Some((instance, surface)) = super::headless_instance(flags) else {
            return;
        };
        for (exposed, caps) in super::adapters(&instance, &surface) {
            let (device, queue) = super::open(&exposed.adapter);
            let raw_queue = queue.as_raw();
            let config = super::config(&caps);
            // SAFETY: The device's own fence.
            let fence = unsafe { device.create_fence() }.unwrap();
            calls::take();

            // SAFETY: Each call meets wgpu-hal's requirements: every submission completes before
            // the surface is configured again or unconfigured, and before the idle wait.
            unsafe {
                surface.configure(&device, &config).unwrap();
                super::present_frame(&device, &queue, &surface, &fence, 1);
                super::present_frame(&device, &queue, &surface, &fence, 2);
                calls::mark("unconfigure");
                surface.unconfigure(&device);
                calls::mark("unconfigured");
                surface.configure(&device, &config).unwrap();
                super::present_frame(&device, &queue, &surface, &fence, 3);
                let acquired = surface
                    .acquire_texture(Some(Duration::from_secs(1)), &fence)
                    .unwrap();
                surface.discard_texture(acquired.texture);
                calls::mark("unconfigure");
                surface.unconfigure(&device);
                calls::mark("unconfigured");
                queue.wait_for_idle().unwrap();
                device.destroy_fence(fence);
            }
            drop((queue, device));

            let calls = calls::take().calls;
            check_teardowns(&calls, raw_queue);
            super::check_destroyed(&calls);
        }
    }
}

/// Checks that each teardown in `calls`, between the marks `unconfigure` and `unconfigured`,
/// waits for `queue` to become idle, and that nothing waits for the whole device.
fn check_teardowns(calls: &[calls::Call], queue: ash::vk::Queue) {
    assert!(
        !calls.iter().any(|call| call.kind == Kind::DeviceWaitIdle),
        "vkDeviceWaitIdle was called: {calls:#?}"
    );
    let starts = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.name == "unconfigure")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 2, "{calls:#?}");
    for start in starts {
        let end = start
            + calls[start..]
                .iter()
                .position(|call| call.name == "unconfigured")
                .unwrap();
        assert!(
            calls[start..end]
                .iter()
                .any(|call| call.name == "vkQueueWaitIdle" && call.queue == queue),
            "a teardown did not wait for the presenting queue: {:#?}",
            &calls[start..end]
        );
    }
}
