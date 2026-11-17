//! The commands of `VK_KHR_get_physical_device_properties2` that wgpu-hal uses
//! (`vkGetPhysicalDeviceProperties2`, `vkGetPhysicalDeviceFeatures2`,
//! `vkGetPhysicalDeviceMemoryProperties2` and `vkGetPhysicalDeviceFormatProperties2`), taken for
//! each physical device from where the Vulkan specification lets the instance use them with it:
//! Vulkan 1.1, which promoted the extension (its commands are aliases of the core ones), or the
//! extension.

use ash::{khr, vk};

/// Where a physical device's commands come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    /// Vulkan 1.1.
    Core,
    /// `VK_KHR_get_physical_device_properties2`, enabled on the instance.
    Extension,
}

/// Where a physical device of effective Vulkan version `effective_version`
/// ([`super::InstanceShared::effective_device_version`]), of an instance with
/// `VK_KHR_get_physical_device_properties2` enabled or not, takes the commands from, or `None`
/// where it can use neither.
///
/// - Vulkan 1.1 where its effective version is 1.1 or later: where the instance and the physical
///   device are both 1.1 or later. The specification (Initialization, "Extending Physical Device
///   Core Functionality"): "New core physical-device-level functionality can be used when both
///   `VkPhysicalDeviceProperties::apiVersion` and `VkApplicationInfo::apiVersion` are greater than
///   or equal to the version of Vulkan that added the new functionality" (and Fundamentals, "Valid
///   Usage for Newer Core Versions").
/// - Else the extension, where it is enabled, for any physical device: physical-device-level
///   functionality added by an instance extension "must not be used unless that extension is
///   supported by the instance as determined by `vkEnumerateInstanceExtensionProperties`, and that
///   extension is enabled in `VkInstanceCreateInfo`" (Fundamentals, "Valid Usage for Extensions").
///
/// Vulkan 1.1 is taken where both serve, as the specification places the commands.
pub(super) fn source(effective_version: u32, extension_enabled: bool) -> Option<Source> {
    if effective_version >= vk::API_VERSION_1_1 {
        Some(Source::Core)
    } else if extension_enabled {
        Some(Source::Extension)
    } else {
        None
    }
}

/// The commands for one physical device, from where [`source`] says.
#[derive(Clone, Copy)]
pub(super) enum Properties2<'a> {
    Core(&'a ash::Instance),
    Extension(&'a khr::get_physical_device_properties2::Instance),
}

impl Properties2<'_> {
    /// `vkGetPhysicalDeviceProperties2`.
    ///
    /// # Safety
    ///
    /// `phd` is a physical device of the instance, for which [`source`] gave this source, and
    /// `properties`' chain holds only structures of extensions that the physical device lists, or
    /// of core versions up to its effective version
    /// ([`super::InstanceShared::effective_device_version`]).
    pub(super) unsafe fn properties(
        self,
        phd: vk::PhysicalDevice,
        properties: &mut vk::PhysicalDeviceProperties2<'_>,
    ) {
        match self {
            // SAFETY: The caller's requirements.
            Self::Core(instance) => unsafe {
                instance.get_physical_device_properties2(phd, properties)
            },
            // SAFETY: As above.
            Self::Extension(extension) => unsafe {
                extension.get_physical_device_properties2(phd, properties)
            },
        }
    }

    /// `vkGetPhysicalDeviceFeatures2`.
    ///
    /// # Safety
    ///
    /// As for [`Self::properties`], with `features`' chain.
    pub(super) unsafe fn features(
        self,
        phd: vk::PhysicalDevice,
        features: &mut vk::PhysicalDeviceFeatures2<'_>,
    ) {
        match self {
            // SAFETY: The caller's requirements.
            Self::Core(instance) => unsafe {
                instance.get_physical_device_features2(phd, features)
            },
            // SAFETY: As above.
            Self::Extension(extension) => unsafe {
                extension.get_physical_device_features2(phd, features)
            },
        }
    }

    /// `vkGetPhysicalDeviceMemoryProperties2`.
    ///
    /// # Safety
    ///
    /// As for [`Self::properties`], with `properties`' chain.
    pub(super) unsafe fn memory_properties(
        self,
        phd: vk::PhysicalDevice,
        properties: &mut vk::PhysicalDeviceMemoryProperties2<'_>,
    ) {
        match self {
            // SAFETY: The caller's requirements.
            Self::Core(instance) => unsafe {
                instance.get_physical_device_memory_properties2(phd, properties)
            },
            // SAFETY: As above.
            Self::Extension(extension) => unsafe {
                extension.get_physical_device_memory_properties2(phd, properties)
            },
        }
    }

    /// `vkGetPhysicalDeviceFormatProperties2`.
    ///
    /// # Safety
    ///
    /// As for [`Self::properties`], with `properties`' chain.
    pub(super) unsafe fn format_properties(
        self,
        phd: vk::PhysicalDevice,
        format: vk::Format,
        properties: &mut vk::FormatProperties2<'_>,
    ) {
        match self {
            // SAFETY: The caller's requirements.
            Self::Core(instance) => unsafe {
                instance.get_physical_device_format_properties2(phd, format, properties)
            },
            // SAFETY: As above.
            Self::Extension(extension) => unsafe {
                extension.get_physical_device_format_properties2(phd, format, properties)
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{source, Source};
    use ash::vk;

    /// Vulkan 1.1's commands wherever the effective version is 1.1 or later, with or without the
    /// extension.
    #[test]
    fn an_effective_1_1_or_later_takes_the_core_commands() {
        for version in [
            vk::API_VERSION_1_1,
            vk::API_VERSION_1_2,
            vk::API_VERSION_1_3,
        ] {
            for extension in [false, true] {
                assert_eq!(
                    source(version, extension),
                    Some(Source::Core),
                    "version {version:#x}, extension {extension}"
                );
            }
        }
    }

    /// An effective 1.0 (a 1.0 instance, or a 1.0 physical device) takes the extension's commands
    /// where it is enabled, and none where it is not.
    #[test]
    fn an_effective_1_0_takes_the_extension_or_nothing() {
        assert_eq!(source(vk::API_VERSION_1_0, true), Some(Source::Extension));
        assert_eq!(source(vk::API_VERSION_1_0, false), None);
    }
}
