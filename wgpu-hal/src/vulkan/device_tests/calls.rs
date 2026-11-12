//! Wrappers of the Vulkan device functions that access a queue, through which a test watches the
//! calls that wgpu-hal makes. wgpu-hal's own tests use it.
//!
//! [`load_instance`] loads an instance whose `vkGetDeviceProcAddr` hands out these wrappers. Every
//! device function that wgpu-hal loads goes through it: `ash::Device`'s tables, and the loaders of
//! extensions, which resolve their functions through the instance. Each wrapper records its call.
//!
//! The wrappers cover every command that `vk.xml` marks as requiring host access to its queue to
//! be externally synchronized (`externsync="maybe"`), and `vkSetDebugUtilsObjectNameEXT` naming a
//! queue; `vkDeviceWaitIdle`; the fence and semaphore waits; the queue retrievals; and
//! `vkDestroyDevice`.

use alloc::vec::Vec;
use core::{
    ffi::{c_char, c_void, CStr},
    mem, ptr,
    sync::atomic::{AtomicUsize, Ordering},
};
use std::sync::Mutex;

use ash::vk;

/// What a recorded call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A command that requires host access to its queue to be externally synchronized, unless the
    /// queue is internally synchronized.
    Queue,
    /// `vkDeviceWaitIdle`, which requires it for every queue of the device.
    DeviceWaitIdle,
    /// A wait for fences or timeline semaphores: a GPU wait that is not a queue operation.
    FenceWait,
    /// `vkGetDeviceQueue` (flags 0) or `vkGetDeviceQueue2` (the flags it passes).
    GetQueue { flags: u32 },
    /// `vkDestroyDevice`.
    DestroyDevice,
    /// A point that the test marked with [`mark`].
    Mark,
}

#[derive(Clone, Debug)]
pub struct Call {
    pub name: &'static str,
    pub kind: Kind,
    /// The queue that the call accesses, or a null handle.
    pub queue: vk::Queue,
    /// How many queue commands were in progress, on any thread, when the call started.
    pub in_flight: usize,
}

/// What was recorded between two [`take`]s.
#[derive(Debug)]
pub struct Recorded {
    pub calls: Vec<Call>,
}

static CALLS: Mutex<Vec<Call>> = Mutex::new(Vec::new());
/// The real functions, by name: those of the device a test has open.
static REAL: Mutex<Vec<(&'static str, usize)>> = Mutex::new(Vec::new());
static REAL_GET_DEVICE_PROC_ADDR: Mutex<Option<vk::PFN_vkGetDeviceProcAddr>> = Mutex::new(None);
static IN_QUEUE_COMMAND: AtomicUsize = AtomicUsize::new(0);

/// Takes what was recorded since the last call, and starts recording afresh.
pub fn take() -> Recorded {
    let mut calls = CALLS.lock().unwrap();
    Recorded {
        calls: mem::take(&mut *calls),
    }
}

/// Records a point of the test, such as the start of a call to wgpu-hal.
pub fn mark(name: &'static str) {
    record(name, Kind::Mark, vk::Queue::null());
}

fn record(name: &'static str, kind: Kind, queue: vk::Queue) {
    let in_flight = IN_QUEUE_COMMAND.load(Ordering::SeqCst);
    CALLS.lock().unwrap().push(Call {
        name,
        kind,
        queue,
        in_flight,
    });
}

/// The real function `name`, as a function pointer of type `F`.
///
/// # Safety
///
/// `F` is the function's type.
unsafe fn real<F: Copy>(name: &'static str) -> F {
    let address = REAL
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|&&(real, _)| real == name)
        .unwrap()
        .1;
    assert_eq!(size_of::<F>(), size_of::<usize>());
    // SAFETY: `address` is the function `name`, whose type our caller names.
    unsafe { mem::transmute_copy(&address) }
}

/// Runs `f`, a queue command, counting the queue commands in progress.
fn queue_command<R>(name: &'static str, queue: vk::Queue, f: impl FnOnce() -> R) -> R {
    record(name, Kind::Queue, queue);
    IN_QUEUE_COMMAND.fetch_add(1, Ordering::SeqCst);
    let result = f();
    IN_QUEUE_COMMAND.fetch_sub(1, Ordering::SeqCst);
    result
}

unsafe extern "system" fn queue_submit(
    queue: vk::Queue,
    count: u32,
    submits: *const vk::SubmitInfo<'_>,
    fence: vk::Fence,
) -> vk::Result {
    queue_command("vkQueueSubmit", queue, || unsafe {
        real::<vk::PFN_vkQueueSubmit>("vkQueueSubmit")(queue, count, submits, fence)
    })
}

unsafe extern "system" fn queue_submit2(
    queue: vk::Queue,
    count: u32,
    submits: *const vk::SubmitInfo2<'_>,
    fence: vk::Fence,
) -> vk::Result {
    queue_command("vkQueueSubmit2", queue, || unsafe {
        real::<vk::PFN_vkQueueSubmit2>("vkQueueSubmit2")(queue, count, submits, fence)
    })
}

unsafe extern "system" fn queue_submit2_khr(
    queue: vk::Queue,
    count: u32,
    submits: *const vk::SubmitInfo2<'_>,
    fence: vk::Fence,
) -> vk::Result {
    queue_command("vkQueueSubmit2KHR", queue, || unsafe {
        real::<vk::PFN_vkQueueSubmit2>("vkQueueSubmit2KHR")(queue, count, submits, fence)
    })
}

unsafe extern "system" fn queue_bind_sparse(
    queue: vk::Queue,
    count: u32,
    infos: *const vk::BindSparseInfo<'_>,
    fence: vk::Fence,
) -> vk::Result {
    queue_command("vkQueueBindSparse", queue, || unsafe {
        real::<vk::PFN_vkQueueBindSparse>("vkQueueBindSparse")(queue, count, infos, fence)
    })
}

unsafe extern "system" fn queue_present(
    queue: vk::Queue,
    info: *const vk::PresentInfoKHR<'_>,
) -> vk::Result {
    queue_command("vkQueuePresentKHR", queue, || unsafe {
        real::<vk::PFN_vkQueuePresentKHR>("vkQueuePresentKHR")(queue, info)
    })
}

unsafe extern "system" fn queue_wait_idle(queue: vk::Queue) -> vk::Result {
    queue_command("vkQueueWaitIdle", queue, || unsafe {
        real::<vk::PFN_vkQueueWaitIdle>("vkQueueWaitIdle")(queue)
    })
}

unsafe extern "system" fn queue_begin_label(
    queue: vk::Queue,
    label: *const vk::DebugUtilsLabelEXT<'_>,
) {
    queue_command("vkQueueBeginDebugUtilsLabelEXT", queue, || unsafe {
        real::<vk::PFN_vkQueueBeginDebugUtilsLabelEXT>("vkQueueBeginDebugUtilsLabelEXT")(
            queue, label,
        )
    })
}

unsafe extern "system" fn queue_end_label(queue: vk::Queue) {
    queue_command("vkQueueEndDebugUtilsLabelEXT", queue, || unsafe {
        real::<vk::PFN_vkQueueEndDebugUtilsLabelEXT>("vkQueueEndDebugUtilsLabelEXT")(queue)
    })
}

unsafe extern "system" fn queue_insert_label(
    queue: vk::Queue,
    label: *const vk::DebugUtilsLabelEXT<'_>,
) {
    queue_command("vkQueueInsertDebugUtilsLabelEXT", queue, || unsafe {
        real::<vk::PFN_vkQueueInsertDebugUtilsLabelEXT>("vkQueueInsertDebugUtilsLabelEXT")(
            queue, label,
        )
    })
}

unsafe extern "system" fn queue_set_performance_configuration_intel(
    queue: vk::Queue,
    configuration: vk::PerformanceConfigurationINTEL,
) -> vk::Result {
    queue_command(
        "vkQueueSetPerformanceConfigurationINTEL",
        queue,
        || unsafe {
            real::<vk::PFN_vkQueueSetPerformanceConfigurationINTEL>(
                "vkQueueSetPerformanceConfigurationINTEL",
            )(queue, configuration)
        },
    )
}

/// `vkQueueSetPerfHintQCOM`, which `ash` 0.38 does not define: its info is passed through.
type QueueSetPerfHintQcom = unsafe extern "system" fn(vk::Queue, *const c_void) -> vk::Result;

unsafe extern "system" fn queue_set_perf_hint_qcom(
    queue: vk::Queue,
    info: *const c_void,
) -> vk::Result {
    queue_command("vkQueueSetPerfHintQCOM", queue, || unsafe {
        real::<QueueSetPerfHintQcom>("vkQueueSetPerfHintQCOM")(queue, info)
    })
}

unsafe extern "system" fn set_debug_utils_object_name(
    device: vk::Device,
    info: *const vk::DebugUtilsObjectNameInfoEXT<'_>,
) -> vk::Result {
    let call = || unsafe {
        real::<vk::PFN_vkSetDebugUtilsObjectNameEXT>("vkSetDebugUtilsObjectNameEXT")(device, info)
    };
    // Only naming a queue accesses a queue.
    if unsafe { (*info).object_type } == vk::ObjectType::QUEUE {
        let queue = vk::Handle::from_raw(unsafe { (*info).object_handle });
        queue_command("vkSetDebugUtilsObjectNameEXT", queue, call)
    } else {
        call()
    }
}

unsafe extern "system" fn device_wait_idle(device: vk::Device) -> vk::Result {
    record("vkDeviceWaitIdle", Kind::DeviceWaitIdle, vk::Queue::null());
    unsafe { real::<vk::PFN_vkDeviceWaitIdle>("vkDeviceWaitIdle")(device) }
}

unsafe extern "system" fn wait_for_fences(
    device: vk::Device,
    count: u32,
    fences: *const vk::Fence,
    wait_all: vk::Bool32,
    timeout: u64,
) -> vk::Result {
    record("vkWaitForFences", Kind::FenceWait, vk::Queue::null());
    unsafe {
        real::<vk::PFN_vkWaitForFences>("vkWaitForFences")(device, count, fences, wait_all, timeout)
    }
}

unsafe extern "system" fn wait_semaphores(
    device: vk::Device,
    info: *const vk::SemaphoreWaitInfo<'_>,
    timeout: u64,
) -> vk::Result {
    record("vkWaitSemaphores", Kind::FenceWait, vk::Queue::null());
    unsafe { real::<vk::PFN_vkWaitSemaphores>("vkWaitSemaphores")(device, info, timeout) }
}

unsafe extern "system" fn wait_semaphores_khr(
    device: vk::Device,
    info: *const vk::SemaphoreWaitInfo<'_>,
    timeout: u64,
) -> vk::Result {
    record("vkWaitSemaphoresKHR", Kind::FenceWait, vk::Queue::null());
    unsafe { real::<vk::PFN_vkWaitSemaphores>("vkWaitSemaphoresKHR")(device, info, timeout) }
}

unsafe extern "system" fn get_device_queue(
    device: vk::Device,
    family: u32,
    index: u32,
    queue: *mut vk::Queue,
) {
    unsafe {
        real::<vk::PFN_vkGetDeviceQueue>("vkGetDeviceQueue")(device, family, index, queue);
        record("vkGetDeviceQueue", Kind::GetQueue { flags: 0 }, *queue);
    }
}

unsafe extern "system" fn get_device_queue2(
    device: vk::Device,
    info: *const vk::DeviceQueueInfo2<'_>,
    queue: *mut vk::Queue,
) {
    unsafe {
        real::<vk::PFN_vkGetDeviceQueue2>("vkGetDeviceQueue2")(device, info, queue);
        let flags = (*info).flags.as_raw();
        record("vkGetDeviceQueue2", Kind::GetQueue { flags }, *queue);
    }
}

unsafe extern "system" fn destroy_device(
    device: vk::Device,
    allocator: *const vk::AllocationCallbacks<'_>,
) {
    record("vkDestroyDevice", Kind::DestroyDevice, vk::Queue::null());
    unsafe { real::<vk::PFN_vkDestroyDevice>("vkDestroyDevice")(device, allocator) }
}

/// The wrapper of the device function `name`, if there is one.
fn wrapper(name: &CStr) -> Option<(&'static str, *const ())> {
    let wrappers: [(&'static CStr, *const ()); 19] = [
        (c"vkQueueSubmit", queue_submit as *const ()),
        (c"vkQueueSubmit2", queue_submit2 as *const ()),
        (c"vkQueueSubmit2KHR", queue_submit2_khr as *const ()),
        (c"vkQueueBindSparse", queue_bind_sparse as *const ()),
        (c"vkQueuePresentKHR", queue_present as *const ()),
        (c"vkQueueWaitIdle", queue_wait_idle as *const ()),
        (
            c"vkQueueBeginDebugUtilsLabelEXT",
            queue_begin_label as *const (),
        ),
        (
            c"vkQueueEndDebugUtilsLabelEXT",
            queue_end_label as *const (),
        ),
        (
            c"vkQueueInsertDebugUtilsLabelEXT",
            queue_insert_label as *const (),
        ),
        (
            c"vkQueueSetPerformanceConfigurationINTEL",
            queue_set_performance_configuration_intel as *const (),
        ),
        (
            c"vkQueueSetPerfHintQCOM",
            queue_set_perf_hint_qcom as *const (),
        ),
        (
            c"vkSetDebugUtilsObjectNameEXT",
            set_debug_utils_object_name as *const (),
        ),
        (c"vkDeviceWaitIdle", device_wait_idle as *const ()),
        (c"vkWaitForFences", wait_for_fences as *const ()),
        (c"vkWaitSemaphores", wait_semaphores as *const ()),
        (c"vkWaitSemaphoresKHR", wait_semaphores_khr as *const ()),
        (c"vkGetDeviceQueue", get_device_queue as *const ()),
        (c"vkGetDeviceQueue2", get_device_queue2 as *const ()),
        (c"vkDestroyDevice", destroy_device as *const ()),
    ];
    wrappers
        .into_iter()
        .find(|&(wrapped, _)| wrapped == name)
        .map(|(wrapped, wrapper)| (wrapped.to_str().unwrap(), wrapper))
}

/// The instance's `vkGetDeviceProcAddr`.
unsafe extern "system" fn get_device_proc_addr(
    device: vk::Device,
    name: *const c_char,
) -> vk::PFN_vkVoidFunction {
    let real_get = REAL_GET_DEVICE_PROC_ADDR.lock().unwrap().unwrap();
    let real_function = unsafe { real_get(device, name) };
    let name = unsafe { CStr::from_ptr(name) };
    match (real_function, wrapper(name)) {
        (Some(function), Some((name, wrapper))) => {
            REAL.lock().unwrap().push((name, function as usize));
            // SAFETY: Only cast to the generic function pointer type, which the loader casts back.
            Some(unsafe { mem::transmute::<*const (), unsafe extern "system" fn()>(wrapper) })
        }
        (real_function, _) => real_function,
    }
}

/// Loads `handle`, an instance created from `entry`, with the wrappers' `vkGetDeviceProcAddr`,
/// and starts recording.
///
/// # Safety
///
/// `handle` is a valid instance created from `entry`. One instance is loaded at a time.
pub unsafe fn load_instance(entry: &ash::Entry, handle: vk::Instance) -> ash::Instance {
    // SAFETY: A valid instance and name.
    let real_get =
        unsafe { entry.get_instance_proc_addr(handle, c"vkGetDeviceProcAddr".as_ptr()) }.unwrap();
    // SAFETY: The function `vkGetDeviceProcAddr`.
    *REAL_GET_DEVICE_PROC_ADDR.lock().unwrap() = Some(unsafe {
        mem::transmute::<unsafe extern "system" fn(), vk::PFN_vkGetDeviceProcAddr>(real_get)
    });
    REAL.lock().unwrap().clear();
    take();
    // SAFETY: The instance's functions, and the wrapper of `vkGetDeviceProcAddr`.
    unsafe {
        ash::Instance::load_with(
            |name| {
                if name == c"vkGetDeviceProcAddr" {
                    get_device_proc_addr as *const c_void
                } else {
                    entry
                        .get_instance_proc_addr(handle, name.as_ptr())
                        .map_or(ptr::null(), |function| function as *const c_void)
                }
            },
            handle,
        )
    }
}
