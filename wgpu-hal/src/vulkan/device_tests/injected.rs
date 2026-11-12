//! A lock that the application gives the device: present and the swapchain teardown take it
//! around exactly their queue commands.

use alloc::{sync::Arc, vec::Vec};
use core::time::Duration;
use std::sync::{Condvar, Mutex};

use ash::vk;

use super::super::{call_log, QueueLock, QueueLockScope};
use crate::{Device as _, Queue as _, Surface as _};

/// A lock that excludes like a mutex, for every scope, tells the wrappers which thread holds it,
/// and records the scope of each acquisition. A binary semaphore: it keeps no guard state.
#[derive(Default)]
pub(super) struct InstrumentedLock {
    locked: Mutex<bool>,
    unlocked: Condvar,
    scopes: Mutex<Vec<QueueLockScope>>,
}

// SAFETY: Every scope sets `locked`, which `unlock` clears, under the mutex. Neither unwinds.
unsafe impl QueueLock for InstrumentedLock {
    fn lock(&self, scope: QueueLockScope) {
        let mut locked = self.locked.lock().unwrap();
        while *locked {
            locked = self.unlocked.wait(locked).unwrap();
        }
        *locked = true;
        call_log::set_held(true);
        self.scopes.lock().unwrap().push(scope);
    }

    unsafe fn unlock(&self, _scope: QueueLockScope) {
        call_log::set_held(false);
        *self.locked.lock().unwrap() = false;
        self.unlocked.notify_one();
    }
}

/// Opens a device on `adapter` with `vkCreateDevice` and `device_from_raw`, on queue 0 of family
/// 0, giving it `lock`. Without `present_fences`, the device lacks the swapchain maintenance
/// extensions, so that its presents signal no fences.
pub(super) fn open(
    adapter: &super::super::Adapter,
    instance: &super::super::Instance,
    lock: Arc<InstrumentedLock>,
    present_fences: bool,
) -> (super::super::Device, super::super::Queue) {
    let features = wgt::Features::empty();
    let mut extensions = adapter.required_device_extensions(features);
    if !present_fences {
        extensions.retain(|extension| {
            !super::super::adapter::SWAPCHAIN_MAINTENANCE1
                .iter()
                .any(|(name, _)| name == extension)
        });
    }
    let mut physical_features = adapter.physical_device_features(&extensions, features);
    let priorities = [1.0];
    let queues = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(0)
        .queue_priorities(&priorities)];
    let names = extensions
        .iter()
        .map(|name| name.as_ptr())
        .collect::<Vec<_>>();
    let info = physical_features.add_to_device_create(
        vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&names),
    );
    // SAFETY: A valid create info, with wgpu-hal's required extensions and features.
    let raw = unsafe {
        instance
            .shared
            .raw
            .create_device(adapter.raw_physical_device(), &info, None)
    }
    .unwrap();
    // SAFETY: Created above as `device_from_raw` requires; wgpu-hal destroys it.
    let open = unsafe {
        adapter.device_from_raw(
            raw,
            None,
            &extensions,
            features,
            &wgt::Limits::downlevel_defaults(),
            &wgt::MemoryHints::default(),
            super::super::DeviceQueue::new(0, 0).lock(lock),
        )
    }
    .unwrap();
    (open.device, open.queue)
}

/// Configures a headless surface, presents to it, configures it again, discards a frame and
/// unconfigures it, then waits for the queue to become idle, on each adapter, with a lock that the
/// application gave the device, with present fences where the adapter supports them and without:
/// every queue command holds the lock, with the queue's scope, and no fence wait does.
#[test]
fn surface_operations_hold_the_lock_the_application_gave() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    for (flags, present_fences) in super::FLAGS
        .into_iter()
        .flat_map(|flags| [(flags, true), (flags, false)])
    {
        std::println!("{flags:?}");
        let Some((instance, surface)) = super::headless_instance(flags) else {
            return;
        };
        for (exposed, caps) in super::adapters(&instance, &surface) {
            let lock = Arc::new(InstrumentedLock::default());
            let (device, queue) = open(&exposed.adapter, &instance, lock.clone(), present_fences);
            std::println!("{:?}: {}", exposed.info.name, super::teardown_path(&device));
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
                surface.unconfigure(&device);
                surface.configure(&device, &config).unwrap();
                super::present_frame(&device, &queue, &surface, &fence, 3);
                let acquired = surface
                    .acquire_texture(Some(Duration::from_secs(1)), &fence)
                    .unwrap();
                surface.discard_texture(acquired.texture);
                surface.unconfigure(&device);
                queue.wait_for_idle().unwrap();
                device.destroy_fence(fence);
            }
            drop((queue, device));

            let calls = call_log::take().calls;
            for name in ["vkQueueSubmit", "vkQueuePresentKHR", "vkQueueWaitIdle"] {
                assert!(
                    calls.iter().any(|call| call.name == name),
                    "{name} was never called: {calls:#?}"
                );
            }
            let unlocked = calls
                .iter()
                .filter(|call| call.kind == call_log::Kind::Queue && !call.locked)
                .collect::<Vec<_>>();
            assert!(
                unlocked.is_empty(),
                "queue commands without the lock: {unlocked:#?}"
            );
            let locked_waits = calls
                .iter()
                .filter(|call| call.kind == call_log::Kind::FenceWait && call.locked)
                .collect::<Vec<_>>();
            assert!(
                locked_waits.is_empty(),
                "fence waits with the lock held: {locked_waits:#?}"
            );
            let expected = QueueLockScope::Queue {
                family_index: 0,
                queue_index: 0,
            };
            assert!(lock
                .scopes
                .lock()
                .unwrap()
                .iter()
                .all(|&scope| scope == expected));
            super::check_destroyed(&calls);
        }
    }
}
