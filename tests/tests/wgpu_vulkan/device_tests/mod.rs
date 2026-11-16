mod downlevel;
mod hal_adapter;
mod injected;
mod internal;
mod queue_lock;

use std::{
    ffi::{c_void, CStr},
    sync::Mutex,
};

use ash::{ext, vk};
use wgpu_hal::{
    api::Vulkan,
    vulkan::{call_log, QueueLockScope},
};

/// The queue that wgpu-hal opens a device with, and the tests give `device_from_raw`.
pub const QUEUE: QueueLockScope = QueueLockScope::Queue {
    family_index: 0,
    queue_index: 0,
};

/// One test at a time: the tests share `call_log`'s records.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// The error messages that the validation layer reported through [`Messenger`].
static MESSAGES: Mutex<Vec<String>> = Mutex::new(Vec::new());

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// A debug messenger that records the validation layer's error messages in [`MESSAGES`], as long
/// as it lives. It must be dropped before the instance it belongs to.
pub struct Messenger {
    debug_utils: ext::debug_utils::Instance,
    raw: vk::DebugUtilsMessengerEXT,
}

impl Messenger {
    /// Takes the error messages reported so far.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *MESSAGES.lock().unwrap())
    }
}

impl Drop for Messenger {
    fn drop(&mut self) {
        // SAFETY: Created from this instance, which is still alive.
        unsafe {
            self.debug_utils
                .destroy_debug_utils_messenger(self.raw, None)
        };
    }
}

unsafe extern "system" fn record_message(
    _severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user_data: *mut c_void,
) -> vk::Bool32 {
    // SAFETY: The layer passes valid callback data.
    let message = unsafe { (*data).message_as_c_str() }.map_or_else(String::new, |message| {
        message.to_string_lossy().into_owned()
    });
    MESSAGES.lock().unwrap().push(message);
    vk::FALSE
}

/// The instance flags each test of every queue command runs with: without and with `DEBUG`,
/// which loads the `VK_EXT_debug_utils` device functions, so that their wrappers are installed.
pub const FLAGS: [wgpu::InstanceFlags; 2] =
    [wgpu::InstanceFlags::empty(), wgpu::InstanceFlags::DEBUG];

/// A wgpu instance with `flags` on a Vulkan instance whose device functions go through
/// `call_log`.
///
/// With `VALIDATION` in `flags`, the Khronos validation layer is enabled if it is installed, with
/// a [`Messenger`] that records its error messages, the thread-safety checker's included. The
/// checker is on by default, and set on explicitly where the layer takes settings
/// (`VK_EXT_layer_settings`).
/// `None`, having printed why, without Vulkan.
pub fn instance(flags: wgpu::InstanceFlags) -> Option<(wgpu::Instance, Option<Messenger>)> {
    // SAFETY: Loading the Vulkan library has no requirements.
    let Ok(entry) = (unsafe { ash::Entry::load() }) else {
        println!("skipped: no Vulkan");
        return None;
    };
    // SAFETY: No requirements.
    let version = unsafe { entry.try_enumerate_instance_version() }
        .unwrap()
        .unwrap_or(vk::API_VERSION_1_0);
    // SAFETY: No requirements.
    let layers = unsafe { entry.enumerate_instance_layer_properties() }.unwrap_or_default();
    let validation = flags.contains(wgpu::InstanceFlags::VALIDATION)
        && layers
            .iter()
            .any(|layer| layer.layer_name_as_c_str() == Ok(VALIDATION_LAYER));
    let mut extensions =
        wgpu_hal::vulkan::Instance::desired_extensions(&entry, version, flags).unwrap();
    let validation = validation && extensions.contains(&ext::debug_utils::NAME);
    // SAFETY: No requirements.
    let layer_settings = validation
        && unsafe { entry.enumerate_instance_extension_properties(Some(VALIDATION_LAYER)) }
            .unwrap_or_default()
            .iter()
            .any(|extension| extension.extension_name_as_c_str() == Ok(ext::layer_settings::NAME));
    if layer_settings {
        extensions.push(ext::layer_settings::NAME);
    }
    println!(
        "{flags:?}, validation layer: {validation}, thread-safety checker set on: {layer_settings}"
    );
    // As `wgpu_hal::vulkan::Instance::init` does.
    let app_info = vk::ApplicationInfo::default().api_version(if version < vk::API_VERSION_1_1 {
        vk::API_VERSION_1_0
    } else {
        vk::API_VERSION_1_3
    });
    let names = extensions
        .iter()
        .map(|name| name.as_ptr())
        .collect::<Vec<_>>();
    let layer_names = [VALIDATION_LAYER.as_ptr()];
    let mut create_flags = vk::InstanceCreateFlags::empty();
    if extensions.contains(&ash::khr::portability_enumeration::NAME) {
        create_flags |= vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR;
    }
    // The thread-safety checker reports two threads in commands on one queue at once.
    let thread_safety: *const vk::Bool32 = &vk::TRUE;
    let settings = [vk::LayerSettingEXT {
        value_count: 1,
        p_values: thread_safety.cast(),
        ..vk::LayerSettingEXT::default()
            .layer_name(VALIDATION_LAYER)
            .setting_name(c"thread_safety")
            .ty(vk::LayerSettingTypeEXT::BOOL32)
    }];
    let mut settings_info = vk::LayerSettingsCreateInfoEXT::default().settings(&settings);
    let mut info = vk::InstanceCreateInfo::default()
        .flags(create_flags)
        .application_info(&app_info)
        .enabled_extension_names(&names)
        .enabled_layer_names(if validation { &layer_names } else { &[] });
    if layer_settings {
        info = info.push_next(&mut settings_info);
    }
    // SAFETY: A valid create info.
    let handle = unsafe { entry.create_instance(&info, None) }
        .unwrap()
        .handle();
    // SAFETY: Created from `entry` just now.
    let raw = unsafe { call_log::load_instance(&entry, handle) };
    let messenger = validation.then(|| {
        MESSAGES.lock().unwrap().clear();
        let debug_utils = ext::debug_utils::Instance::new(&entry, &raw);
        let info = vk::DebugUtilsMessengerCreateInfoEXT::default()
            .message_severity(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR)
            .message_type(
                vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                    | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION,
            )
            .pfn_user_callback(Some(record_message));
        // SAFETY: A valid create info, of an instance with `VK_EXT_debug_utils`.
        let raw = unsafe { debug_utils.create_debug_utils_messenger(&info, None) }.unwrap();
        Messenger { debug_utils, raw }
    });
    // SAFETY: Created from `entry` with wgpu-hal's desired extensions; wgpu-hal destroys it.
    let hal = unsafe {
        wgpu_hal::vulkan::Instance::from_raw(
            entry,
            raw,
            version,
            0,
            None,
            extensions,
            flags,
            wgpu::MemoryBudgetThresholds::default(),
            false,
            None,
        )
    }
    .unwrap();
    // SAFETY: A Vulkan instance.
    let instance = unsafe { wgpu::Instance::from_hal::<Vulkan>(hal) };
    Some((instance, messenger))
}

/// The instance's Vulkan adapters, CPU ones included. Prints each.
pub fn adapters(instance: &wgpu::Instance) -> Vec<wgpu::Adapter> {
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    for adapter in &adapters {
        println!("{:?}", adapter.get_info().name);
    }
    adapters
}
