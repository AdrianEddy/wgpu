//! The swapchain's teardown waits for its presents, not for the device: on their fences where
//! presents signal fences, else for the presenting queue to become idle.

use core::time::Duration;

use crate::vulkan::call_log::{self, Kind};
use crate::{Device as _, Queue as _, Surface as _};

/// Configures a headless surface, presents to it, configures it again, discards a frame and
/// unconfigures it, on each adapter. Each teardown waits for its presents: where presents signal
/// fences, on its last present's fence, with no queue command; else for the device's queue to
/// become idle. Nothing waits for the whole device.
#[test]
fn the_teardown_waits_for_its_presents_not_the_device() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    for flags in super::FLAGS {
        std::println!("{flags:?}");
        let Some((instance, surface)) = super::headless_instance(flags) else {
            return;
        };
        for (exposed, caps) in super::adapters(&instance, &surface) {
            let (device, queue) = super::open(&exposed.adapter);
            let present_fences = device.presents_signal_fences();
            std::println!("{:?}: {}", exposed.info.name, super::teardown_path(&device));
            let raw_queue = queue.as_raw();
            let config = super::config(&caps);
            // SAFETY: The device's own fence.
            let fence = unsafe { device.create_fence() }.unwrap();
            call_log::take();

            // SAFETY: Each call meets wgpu-hal's requirements: every submission completes before
            // the surface is configured again or unconfigured, and before the idle wait.
            unsafe {
                surface.configure(&device, &config).unwrap();
                super::present_frame(&device, &queue, &surface, &fence, 1);
                super::present_frame(&device, &queue, &surface, &fence, 2);
                call_log::mark("unconfigure");
                surface.unconfigure(&device);
                call_log::mark("unconfigured");
                surface.configure(&device, &config).unwrap();
                super::present_frame(&device, &queue, &surface, &fence, 3);
                let acquired = surface
                    .acquire_texture(Some(Duration::from_secs(1)), &fence)
                    .unwrap();
                surface.discard_texture(acquired.texture);
                call_log::mark("unconfigure");
                surface.unconfigure(&device);
                call_log::mark("unconfigured");
                queue.wait_for_idle().unwrap();
                device.destroy_fence(fence);
            }
            drop((queue, device));

            let calls = call_log::take().calls;
            check_teardowns(&calls, raw_queue, present_fences);
            super::check_destroyed(&calls);
        }
    }
}

/// Checks that each teardown in `calls` waits for its presents to `queue`: on the fence of the
/// last one if `present_fences`, making no queue command, else for `queue` to become idle; and
/// that nothing waits for the whole device.
fn check_teardowns(calls: &[call_log::Call], queue: ash::vk::Queue, present_fences: bool) {
    assert!(
        !calls.iter().any(|call| call.kind == Kind::DeviceWaitIdle),
        "vkDeviceWaitIdle was called: {calls:#?}"
    );
    let teardowns = super::teardowns(calls);
    assert_eq!(teardowns.len(), 2, "{calls:#?}");
    for span in teardowns {
        let teardown = &calls[span.clone()];
        if present_fences {
            let last_present = calls[..span.start]
                .iter()
                .rposition(|call| call.name == "vkQueuePresentKHR")
                .unwrap();
            let [fence] = calls[last_present].fences[..] else {
                panic!("a present without one fence: {:#?}", calls[last_present]);
            };
            assert!(
                teardown
                    .iter()
                    .any(|call| call.name == "vkWaitForFences" && call.fences.contains(&fence)),
                "a teardown did not wait on its last present's fence: {teardown:#?}"
            );
            assert!(
                !teardown.iter().any(|call| call.kind == Kind::Queue),
                "a teardown made a queue command: {teardown:#?}"
            );
        } else {
            assert!(
                teardown
                    .iter()
                    .any(|call| call.name == "vkQueueWaitIdle" && call.queue == queue),
                "a teardown did not wait for the presenting queue: {teardown:#?}"
            );
        }
    }
}
