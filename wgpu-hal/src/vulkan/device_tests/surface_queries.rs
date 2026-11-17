//! The functions with which a surface queries the present scaling it supports
//! (`VK_KHR_get_surface_capabilities2`) are looked up once, when the surface is created. Before, each
//! query looked them up again: `vkGetInstanceProcAddr` twice, on every configuration and every
//! request for the surface's capabilities (review 6, item 6).

use core::{
    ffi::{c_char, CStr},
    sync::atomic::{AtomicUsize, Ordering},
};
use std::sync::{Mutex, OnceLock};

use ash::{khr, vk};

/// The Vulkan loader, to which [`counting_get_instance_proc_addr`] passes every lookup on.
static LOADER: OnceLock<ash::Entry> = OnceLock::new();

/// How many times an instance looked up `vkGetPhysicalDeviceSurfaceCapabilities2KHR`.
static LOOKUPS: AtomicUsize = AtomicUsize::new(0);

/// How many times [`counting_query`] was called.
static CALLS: AtomicUsize = AtomicUsize::new(0);

/// The instance's own `vkGetPhysicalDeviceSurfaceCapabilities2KHR`, last looked up.
static QUERY: Mutex<Option<vk::PFN_vkGetPhysicalDeviceSurfaceCapabilities2KHR>> = Mutex::new(None);

const QUERY_NAME: &CStr = c"vkGetPhysicalDeviceSurfaceCapabilities2KHR";

/// The loader's `vkGetInstanceProcAddr`, counting the lookups of [`QUERY_NAME`] for an instance,
/// for which it gives [`counting_query`] instead.
unsafe extern "system" fn counting_get_instance_proc_addr(
    instance: vk::Instance,
    name: *const c_char,
) -> vk::PFN_vkVoidFunction {
    let loader = LOADER.get().unwrap();
    // SAFETY: Our caller's arguments, passed on.
    let function = unsafe { (loader.static_fn().get_instance_proc_addr)(instance, name) };
    // SAFETY: `name` is a null-terminated string.
    if instance == vk::Instance::null() || unsafe { CStr::from_ptr(name) } != QUERY_NAME {
        return function;
    }
    let function = function?;
    LOOKUPS.fetch_add(1, Ordering::SeqCst);
    // SAFETY: The loader's function of that name, of that signature.
    *QUERY.lock().unwrap() = Some(unsafe {
        core::mem::transmute::<
            unsafe extern "system" fn(),
            vk::PFN_vkGetPhysicalDeviceSurfaceCapabilities2KHR,
        >(function)
    });
    let counting: vk::PFN_vkGetPhysicalDeviceSurfaceCapabilities2KHR = counting_query;
    // SAFETY: A function pointer, given back as Vulkan's void function.
    Some(unsafe {
        core::mem::transmute::<
            vk::PFN_vkGetPhysicalDeviceSurfaceCapabilities2KHR,
            unsafe extern "system" fn(),
        >(counting)
    })
}

/// The instance's own `vkGetPhysicalDeviceSurfaceCapabilities2KHR`, counting its calls.
unsafe extern "system" fn counting_query(
    physical_device: vk::PhysicalDevice,
    surface_info: *const vk::PhysicalDeviceSurfaceInfo2KHR<'_>,
    capabilities: *mut vk::SurfaceCapabilities2KHR<'_>,
) -> vk::Result {
    CALLS.fetch_add(1, Ordering::SeqCst);
    let query = QUERY.lock().unwrap().unwrap();
    // SAFETY: Our caller's arguments, passed on to the instance's own function.
    unsafe { query(physical_device, surface_info, capabilities) }
}

/// Where the instance can query the present scaling a surface supports (its extensions, as
/// `NativeSurface` decides), the surface looks the query's functions up once, at its creation:
/// asking for its capabilities on each adapter, which queries the scaling of each present mode on
/// an adapter whose devices have `swapchainMaintenance1`, and configuring it with a scaling mode,
/// which queries it again, look none up. The query runs, through the function looked up then.
#[test]
fn the_present_scaling_query_is_looked_up_with_the_surface() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    // SAFETY: Loading the Vulkan library has no requirements.
    let Ok(loader) = (unsafe { ash::Entry::load() }) else {
        std::println!("skipped: no Vulkan");
        return;
    };
    LOADER.get_or_init(|| loader);
    // SAFETY: The counting function passes every lookup on to the loader's.
    let entry = unsafe {
        ash::Entry::from_static_fn(ash::StaticFn {
            get_instance_proc_addr: counting_get_instance_proc_addr,
        })
    };
    let before = LOOKUPS.load(Ordering::SeqCst);
    let Some((instance, surface)) = super::headless_instance_of(entry, wgt::InstanceFlags::empty())
    else {
        return;
    };
    let at_creation = LOOKUPS.load(Ordering::SeqCst) - before;
    let extensions = &instance.shared.extensions;
    let can_query = extensions.contains(&khr::get_surface_capabilities2::NAME)
        && super::super::adapter::SWAPCHAIN_MAINTENANCE1
            .iter()
            .any(|(_, surface_extension)| extensions.contains(surface_extension));
    std::println!(
        "the instance can query: {can_query}; lookups at the surface's creation: {at_creation}"
    );
    assert_eq!(at_creation, usize::from(can_query));

    let calls = CALLS.load(Ordering::SeqCst);
    let mut queried = false;
    for (exposed, caps) in super::adapters(&instance, &surface) {
        let maintained =
            super::super::adapter::SWAPCHAIN_MAINTENANCE1
                .iter()
                .any(|&(device_extension, _)| {
                    exposed
                        .adapter
                        .phd_capabilities
                        .supports_extension(device_extension)
                });
        queried |= can_query && maintained;
        let (device, _queue) = super::open(&exposed.adapter);
        let config = crate::SurfaceConfiguration {
            scaling_mode: wgt::SurfaceScalingMode::Stretch,
            ..super::config(&caps)
        };
        // SAFETY: Nothing uses the surface, which is not configured.
        let configured = unsafe { crate::Surface::configure(&surface, &device, &config) };
        std::println!(
            "{:?}: swapchain maintenance {maintained}, scaling modes {:?}, Stretch: {configured:?}",
            exposed.info.name,
            caps.scaling_modes
        );
        if configured.is_ok() {
            // SAFETY: Configured just now, and nothing uses it.
            unsafe { crate::Surface::unconfigure(&surface, &device) };
        }
    }
    let calls = CALLS.load(Ordering::SeqCst) - calls;
    let lookups = LOOKUPS.load(Ordering::SeqCst) - before;
    std::println!("queries: {calls}, lookups in all: {lookups}");
    assert_eq!(
        lookups, at_creation,
        "a query looked its functions up again"
    );
    if queried {
        assert!(calls > 0, "nothing queried the present scaling");
    }
}
