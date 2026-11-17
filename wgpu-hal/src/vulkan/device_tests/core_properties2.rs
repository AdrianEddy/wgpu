//! An instance created for Vulkan 1.3 without `VK_KHR_get_physical_device_properties2`, as a
//! caller's may be, inspects its physical devices with Vulkan 1.1's commands, which the extension's
//! alias: its adapters are those of an instance with the extension. Before, wgpu-hal took the
//! commands from the extension alone, and such an instance's adapters lacked every feature,
//! property and limit that it queries through them.

use alloc::{format, string::String, vec::Vec};

use ash::{ext, khr, vk};

use crate::{Device as _, Instance as _};

/// An instance created for Vulkan 1.3 with wgpu-hal's desired extensions, but without
/// `VK_KHR_get_physical_device_properties2` unless `with_extension`, with `thresholds`. `None`,
/// having printed why, without Vulkan or with a Vulkan 1.0 loader, on which an instance is 1.0.
fn instance(
    with_extension: bool,
    thresholds: wgt::MemoryBudgetThresholds,
) -> Option<super::super::Instance> {
    // SAFETY: Loading the Vulkan library has no requirements.
    let Ok(entry) = (unsafe { ash::Entry::load() }) else {
        std::println!("skipped: no Vulkan");
        return None;
    };
    // SAFETY: No requirements.
    let version = unsafe { entry.try_enumerate_instance_version() }
        .unwrap()
        .unwrap_or(vk::API_VERSION_1_0);
    if version < vk::API_VERSION_1_1 {
        std::println!("skipped: a Vulkan 1.0 loader");
        return None;
    }
    let flags = wgt::InstanceFlags::empty();
    let mut extensions =
        super::super::Instance::desired_extensions(&entry, version, flags).unwrap();
    if !with_extension {
        extensions.retain(|&extension| extension != khr::get_physical_device_properties2::NAME);
    }
    let app_info = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);
    let names = extensions
        .iter()
        .map(|name| name.as_ptr())
        .collect::<Vec<_>>();
    let info = vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&names);
    // SAFETY: A valid create info.
    let raw = unsafe { entry.create_instance(&info, None) }.unwrap();
    // SAFETY: Created from `entry` for Vulkan 1.3 with `extensions`; wgpu-hal destroys it.
    let instance = unsafe {
        super::super::Instance::from_raw(
            entry, raw, version, 0, None, extensions, flags, thresholds, false, None,
        )
    }
    .unwrap();
    Some(instance)
}

/// What tells a physical device from another across instances: its Vulkan 1.0 properties.
fn identity(adapter: &super::super::Adapter) -> (u32, u32, u32, [u8; vk::UUID_SIZE], String) {
    let properties = adapter.phd_capabilities.properties();
    (
        properties.vendor_id,
        properties.device_id,
        properties.driver_version,
        properties.pipeline_cache_uuid,
        format!("{:?}", properties.device_name_as_c_str()),
    )
}

/// The adapters of an instance without the extension are those of an instance with it: the same
/// features, limits, downlevel capabilities and information (the driver's name and the subgroup
/// sizes among them), extensions, private capabilities and format support. The instance with the
/// extension inspects its physical devices through the commands too: the driver's name, which
/// only `VkPhysicalDeviceDriverProperties` gives, is known on each device of Vulkan 1.2 or later.
#[test]
fn an_instance_without_the_extension_has_the_adapters_of_one_with_it() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some(with) = instance(true, wgt::MemoryBudgetThresholds::default()) else {
        return;
    };
    let Some(without) = instance(false, wgt::MemoryBudgetThresholds::default()) else {
        return;
    };
    // SAFETY: No requirements.
    let reference = unsafe { with.enumerate_adapters(None) };
    // SAFETY: No requirements.
    let adapters = unsafe { without.enumerate_adapters(None) };
    assert_eq!(adapters.len(), reference.len());
    let mut wrong = Vec::new();
    for exposed in &adapters {
        let name = &exposed.info.name;
        let matching = reference
            .iter()
            .filter(|other| identity(&other.adapter) == identity(&exposed.adapter))
            .collect::<Vec<_>>();
        let [other] = matching[..] else {
            panic!("{name:?}: {} adapters of the same identity", matching.len());
        };
        if other.adapter.phd_capabilities.device_api_version >= vk::API_VERSION_1_2
            && other.info.driver == "?"
        {
            wrong.push(format!("{name:?}: the reference has no driver properties"));
        }
        if exposed.features != other.features {
            wrong.push(format!(
                "{name:?}: features: lacks {:?}, has more {:?}",
                other.features - exposed.features,
                exposed.features - other.features
            ));
        }
        if exposed.capabilities.limits != other.capabilities.limits {
            wrong.push(format!(
                "{name:?}: limits: {:?} against {:?}",
                exposed.capabilities.limits, other.capabilities.limits
            ));
        }
        if exposed.capabilities.downlevel != other.capabilities.downlevel {
            wrong.push(format!(
                "{name:?}: downlevel: {:?} against {:?}",
                exposed.capabilities.downlevel, other.capabilities.downlevel
            ));
        }
        if exposed.info != other.info {
            wrong.push(format!(
                "{name:?}: information: {:?} against {:?}",
                exposed.info, other.info
            ));
        }
        // The extensions that the physical device lists and that each adapter keeps.
        // SAFETY: A physical device of the instance.
        let listed = unsafe {
            with.shared
                .raw
                .enumerate_device_extension_properties(other.adapter.raw)
        }
        .unwrap();
        let extensions = |adapter: &super::super::Adapter| {
            listed
                .iter()
                .filter_map(|extension| extension.extension_name_as_c_str().ok())
                .filter(|&name| adapter.phd_capabilities.supports_extension(name))
                .map(|name| format!("{name:?}"))
                .collect::<Vec<_>>()
        };
        if extensions(&exposed.adapter) != extensions(&other.adapter) {
            wrong.push(format!(
                "{name:?}: extensions: {:?} against {:?}",
                extensions(&exposed.adapter),
                extensions(&other.adapter)
            ));
        }
        for (what, mine, theirs) in [
            (
                "private capabilities",
                format!("{:?}", exposed.adapter.private_caps),
                format!("{:?}", other.adapter.private_caps),
            ),
            (
                "format support",
                format!("{:?}", exposed.adapter.format_support),
                format!("{:?}", other.adapter.format_support),
            ),
        ] {
            if mine != theirs {
                wrong.push(format!("{name:?}: {what}: {mine} against {theirs}"));
            }
        }
        std::println!(
            "{name:?}: {} features, driver {:?}",
            exposed.features.iter().count(),
            exposed.info.driver
        );
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A device of an instance without the extension reads its memory budget, through
/// `vkGetPhysicalDeviceMemoryProperties2`, where a threshold asks for it: creating a buffer checks
/// the budget. Each device whose physical device lists `VK_EXT_memory_budget` has it (RADV's, which
/// wgpu-hal hides, aside). Before, wgpu-hal enabled the extension and then panicked looking for the
/// command in `VK_KHR_get_physical_device_properties2`.
#[test]
fn a_device_of_an_instance_without_the_extension_reads_its_memory_budget() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let thresholds = wgt::MemoryBudgetThresholds {
        for_resource_creation: Some(100),
        for_device_loss: None,
    };
    let Some(instance) = instance(false, thresholds) else {
        return;
    };
    let mut budgets = 0;
    // SAFETY: No requirements.
    for exposed in unsafe { instance.enumerate_adapters(None) } {
        let name = &exposed.info.name;
        // SAFETY: No feature; the device is dropped before the instance.
        let open = unsafe {
            crate::Adapter::open(
                &exposed.adapter,
                wgt::Features::empty(),
                &wgt::Limits::downlevel_defaults(),
                &wgt::MemoryHints::default(),
            )
        }
        .unwrap();
        // SAFETY: A physical device of the instance.
        let listed = unsafe {
            instance
                .shared
                .raw
                .enumerate_device_extension_properties(exposed.adapter.raw)
        }
        .unwrap()
        .iter()
        .any(|extension| extension.extension_name_as_c_str() == Ok(ext::memory_budget::NAME));
        // wgpu-hal hides the extension on RADV (Mesa work item 15725).
        let expected = listed
            && !exposed
                .adapter
                .phd_capabilities
                .is_driver(vk::DriverId::MESA_RADV);
        if !open.device.shared.private_caps.memory_budget {
            assert!(
                !expected,
                "{name:?}: lists VK_EXT_memory_budget, but the device does not read its budget"
            );
            std::println!("{name:?}: no VK_EXT_memory_budget");
            continue;
        }
        let desc = crate::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgt::BufferUses::COPY_DST,
            memory_flags: crate::MemoryFlags::empty(),
        };
        // SAFETY: A valid descriptor; the buffer is destroyed before the device.
        let (buffer, _size) = unsafe { open.device.create_buffer(&desc) }
            .unwrap_or_else(|error| panic!("{name:?}: {error}"));
        // SAFETY: Created by this device, unused.
        unsafe { open.device.destroy_buffer(buffer) };
        budgets += 1;
        std::println!("{name:?}: the budget was read");
    }
    std::println!("{budgets} devices read their budget");
}
