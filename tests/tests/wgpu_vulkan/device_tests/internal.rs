//! A queue created internally synchronized (`VK_KHR_internally_synchronized_queues`) takes no
//! lock.

use std::{
    ffi::{c_void, CStr},
    panic, ptr,
};

use ash::vk;
use wgpu_hal::{
    api::Vulkan,
    vulkan::{call_log, CreateDeviceCallbackArgs, DeviceQueue},
};

/// `VK_DEVICE_QUEUE_CREATE_INTERNALLY_SYNCHRONIZED_BIT_KHR`, which `ash` 0.38 lacks.
pub(super) const INTERNALLY_SYNCHRONIZED: vk::DeviceQueueCreateFlags =
    vk::DeviceQueueCreateFlags::from_raw(1 << 2);
const INTERNALLY_SYNCHRONIZED_QUEUES: &CStr = c"VK_KHR_internally_synchronized_queues";

/// `VkPhysicalDeviceInternallySynchronizedQueuesFeaturesKHR`, which `ash` 0.38 lacks.
#[repr(C)]
struct InternallySynchronizedQueuesFeatures {
    s_type: vk::StructureType,
    p_next: *mut c_void,
    internally_synchronized_queues: vk::Bool32,
}

impl InternallySynchronizedQueuesFeatures {
    fn new(enabled: bool) -> Self {
        Self {
            // `VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_INTERNALLY_SYNCHRONIZED_QUEUES_FEATURES_KHR`:
            // extension 505, offset 0.
            s_type: vk::StructureType::from_raw(1_000_504_000),
            p_next: ptr::null_mut(),
            internally_synchronized_queues: enabled.into(),
        }
    }
}

/// Whether `adapter` can create internally synchronized queues. Prints why not.
pub(super) fn supports_internally_synchronized_queues(adapter: &wgpu::Adapter) -> bool {
    // SAFETY: Only inspected.
    let hal = unsafe { adapter.as_hal::<Vulkan>() }.unwrap();
    let instance = hal.shared_instance().raw_instance();
    // SAFETY: A physical device of the instance.
    let extensions =
        unsafe { instance.enumerate_device_extension_properties(hal.raw_physical_device()) }
            .unwrap_or_default();
    let supported = extensions
        .iter()
        .any(|extension| extension.extension_name_as_c_str() == Ok(INTERNALLY_SYNCHRONIZED_QUEUES))
        && hal.shared_instance().instance_api_version() >= vk::API_VERSION_1_1
        && {
            let mut feature = InternallySynchronizedQueuesFeatures::new(false);
            let mut features = vk::PhysicalDeviceFeatures2 {
                p_next: (&mut feature as *mut InternallySynchronizedQueuesFeatures).cast(),
                ..Default::default()
            };
            // SAFETY: The chain holds a valid structure of the extension, which the device
            // supports.
            unsafe {
                instance.get_physical_device_features2(hal.raw_physical_device(), &mut features)
            };
            feature.internally_synchronized_queues == vk::TRUE
        };
    if !supported {
        println!(
            "skipped {:?}: no VK_KHR_internally_synchronized_queues",
            adapter.get_info().name
        );
    }
    supported
}

/// Opens a device on `adapter` with [`create`] and `device_from_raw`, on queue 0 of family 0,
/// created with `create_flags`, enabling the extension and its feature when `enable_extension`.
fn open(
    adapter: &wgpu::Adapter,
    create_flags: vk::DeviceQueueCreateFlags,
    enable_extension: bool,
) -> Result<wgpu_hal::OpenDevice<Vulkan>, wgpu_hal::DeviceError> {
    open_with(
        adapter,
        DeviceQueue::new(0, 0).create_flags(create_flags),
        enable_extension,
    )
}

/// Opens a device on `adapter` as [`open`] does, giving `device_from_raw` `queue`, which names
/// queue 0 of family 0.
pub(super) fn open_with(
    adapter: &wgpu::Adapter,
    queue: DeviceQueue,
    enable_extension: bool,
) -> Result<wgpu_hal::OpenDevice<Vulkan>, wgpu_hal::DeviceError> {
    let (raw, extensions) = create(adapter, queue.create_flags, enable_extension);
    // SAFETY: Created by `create` as `device_from_raw` requires, but for the flags when
    // `enable_extension` is false.
    unsafe { from_raw(adapter, raw, &extensions, queue) }
}

/// Creates a device on `adapter` with `vkCreateDevice`, on queue 0 of family 0: created with
/// `create_flags`, with the extension and its feature enabled, when `enable_extension`; else with no
/// flags. Returns it with its extensions.
fn create(
    adapter: &wgpu::Adapter,
    create_flags: vk::DeviceQueueCreateFlags,
    enable_extension: bool,
) -> (ash::Device, Vec<&'static CStr>) {
    let features = wgpu::Features::empty();
    // SAFETY: Used to create a device, as `device_from_raw` requires.
    let hal = unsafe { adapter.as_hal::<Vulkan>() }.unwrap();
    let mut extensions = hal.required_device_extensions(features);
    if enable_extension {
        extensions.push(INTERNALLY_SYNCHRONIZED_QUEUES);
    }
    let mut physical_features = hal.physical_device_features(&extensions, features);
    let priorities = [1.0];
    let queues = [vk::DeviceQueueCreateInfo::default()
        .flags(if enable_extension {
            create_flags
        } else {
            vk::DeviceQueueCreateFlags::empty()
        })
        .queue_family_index(0)
        .queue_priorities(&priorities)];
    let names = extensions
        .iter()
        .map(|name| name.as_ptr())
        .collect::<Vec<_>>();
    let mut info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queues)
        .enabled_extension_names(&names);
    info = physical_features.add_to_device_create(info);
    let mut feature = InternallySynchronizedQueuesFeatures::new(true);
    if enable_extension {
        feature.p_next = info.p_next.cast_mut();
        info.p_next = (&feature as *const InternallySynchronizedQueuesFeatures).cast();
    }
    // SAFETY: A valid create info, with wgpu-hal's required extensions and features.
    let raw = unsafe {
        hal.shared_instance()
            .raw_instance()
            .create_device(hal.raw_physical_device(), &info, None)
    }
    .expect("vkCreateDevice failed");
    (raw, extensions)
}

/// Gives `device_from_raw` `raw`, with `extensions` and `queue`, which names queue 0 of family 0.
///
/// # Safety
///
/// `raw` was created by [`create`] on `adapter`, with `extensions`, as `device_from_raw` requires,
/// but for `queue`'s flags where the call is meant to be refused. wgpu-hal destroys it once it
/// takes it.
unsafe fn from_raw(
    adapter: &wgpu::Adapter,
    raw: ash::Device,
    extensions: &[&'static CStr],
    queue: DeviceQueue,
) -> Result<wgpu_hal::OpenDevice<Vulkan>, wgpu_hal::DeviceError> {
    // SAFETY: Used to open a device.
    let hal = unsafe { adapter.as_hal::<Vulkan>() }.unwrap();
    // SAFETY: Our caller's contract.
    unsafe {
        hal.device_from_raw(
            raw,
            None,
            extensions,
            wgpu::Features::empty(),
            &adapter.limits(),
            &wgpu::MemoryHints::default(),
            queue,
        )
    }
}

/// Uses the device: a write, a map, a poll, a submission, and dropping it all.
fn exercise(device: wgpu::Device, queue: wgpu::Queue) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, &[1; 256]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    assert_eq!(buffer.slice(..).get_mapped_range().unwrap()[0], 1);
    buffer.unmap();
    queue.submit([]);
    drop((buffer, queue, device));
}

/// Checks the calls of a device with an internally synchronized queue: it was retrieved with
/// `vkGetDeviceQueue2` and the flag, and used, and nothing waited for the whole device.
fn check_internal(calls: &[call_log::Call]) {
    let retrievals = calls
        .iter()
        .filter(|call| matches!(call.kind, call_log::Kind::GetQueue { .. }))
        .collect::<Vec<_>>();
    assert!(!retrievals.is_empty());
    assert!(
        retrievals.iter().all(|call| call.kind
            == call_log::Kind::GetQueue {
                flags: INTERNALLY_SYNCHRONIZED.as_raw()
            }),
        "{retrievals:#?}"
    );
    assert!(calls.iter().any(|call| call.name == "vkQueueSubmit"));
    assert!(!calls
        .iter()
        .any(|call| call.kind == call_log::Kind::DeviceWaitIdle));
}

/// A queue given to `device_from_raw` as internally synchronized is retrieved with
/// `vkGetDeviceQueue2` and its flags, and takes no lock.
#[test]
fn an_internally_synchronized_queue_takes_no_lock() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, _messenger)) = super::instance(wgpu::InstanceFlags::empty()) else {
        return;
    };
    for adapter in super::adapters(&instance) {
        if !supports_internally_synchronized_queues(&adapter) {
            continue;
        }
        call_log::take();
        let open = open(&adapter, INTERNALLY_SYNCHRONIZED, true).unwrap();
        assert!(open.queue.queue_lock().is_none());
        // SAFETY: Opened from this adapter.
        let (device, queue) =
            unsafe { adapter.create_device_from_hal(open, &wgpu::DeviceDescriptor::default()) }
                .unwrap();
        exercise(device, queue);
        check_internal(&call_log::take().calls);
    }
}

/// `Adapter::open_with_callback` gives wgpu-hal the flags of the first queue create info: a
/// callback that makes the queue internally synchronized gets a queue without a lock.
#[test]
fn a_callback_can_make_the_queue_internally_synchronized() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, _messenger)) = super::instance(wgpu::InstanceFlags::empty()) else {
        return;
    };
    for adapter in super::adapters(&instance) {
        if !supports_internally_synchronized_queues(&adapter) {
            continue;
        }
        call_log::take();
        let mut feature = InternallySynchronizedQueuesFeatures::new(true);
        let feature_ptr = &mut feature as *mut InternallySynchronizedQueuesFeatures;
        let callback = Box::new(move |args: CreateDeviceCallbackArgs<'_, '_, '_>| {
            args.extensions.push(INTERNALLY_SYNCHRONIZED_QUEUES);
            args.queue_create_infos[0].flags |= INTERNALLY_SYNCHRONIZED;
            // SAFETY: `feature` outlives the device's creation, which this callback is part of.
            unsafe { (*feature_ptr).p_next = args.create_info.p_next.cast_mut() };
            args.create_info.p_next = feature_ptr.cast_const().cast();
        });
        let open = {
            // SAFETY: Used to open a device.
            let hal = unsafe { adapter.as_hal::<Vulkan>() }.unwrap();
            // SAFETY: The callback enables the extension and its feature for the flag it sets.
            unsafe {
                hal.open_with_callback(
                    wgpu::Features::empty(),
                    &adapter.limits(),
                    &wgpu::MemoryHints::default(),
                    Some(callback),
                )
            }
            .unwrap()
        };
        assert!(open.queue.queue_lock().is_none());
        // SAFETY: Opened from this adapter.
        let (device, queue) =
            unsafe { adapter.create_device_from_hal(open, &wgpu::DeviceDescriptor::default()) }
                .unwrap();
        exercise(device, queue);
        check_internal(&call_log::take().calls);
    }
}

/// Giving `device_from_raw` an internally synchronized queue without
/// `VK_KHR_internally_synchronized_queues` enabled is a usage error, caught before the queue is
/// retrieved.
#[test]
fn an_internally_synchronized_queue_needs_the_extension() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, _messenger)) = super::instance(wgpu::InstanceFlags::empty()) else {
        return;
    };
    for adapter in super::adapters(&instance) {
        let (raw, extensions) = create(&adapter, INTERNALLY_SYNCHRONIZED, false);
        call_log::take();
        let queue = DeviceQueue::new(0, 0).create_flags(INTERNALLY_SYNCHRONIZED);
        let opened = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            // SAFETY: Created by `create`, but for the flags, which must be refused.
            unsafe { from_raw(&adapter, raw.clone(), &extensions, queue) }
        }));
        let Err(payload) = opened else {
            panic!("device_from_raw took an internally synchronized queue without the extension");
        };
        // `device_from_raw` refused the device before taking it: destroy it before its instance,
        // which must outlive it (VUID-vkDestroyInstance-instance-00629; NVIDIA's driver crashes).
        // SAFETY: Nothing else uses it, or destroys it.
        unsafe { raw.destroy_device(None) };
        let message = *payload.downcast::<String>().unwrap();
        assert!(
            message.contains("VK_KHR_internally_synchronized_queues"),
            "{message}"
        );
        assert!(!call_log::take()
            .calls
            .iter()
            .any(|call| matches!(call.kind, call_log::Kind::GetQueue { .. })));
    }
}
