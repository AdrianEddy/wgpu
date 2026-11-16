//! Vulkan Surface and Swapchain implementation using native Vulkan surfaces.

use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec::Vec};
use core::any::Any;

use ash::{google, khr, vk};
use wgpu_sync::{Mutex, MutexGuard};

use crate::vulkan::{
    conv, map_host_device_oom_and_lost_err,
    semaphore_list::SemaphoreType,
    swapchain::{
        Surface, SurfaceTextureMetadata, Swapchain, SwapchainSubmissionSemaphoreGuard, WindowHandle,
    },
    DeviceShared, InstanceShared, PnextChain,
};

pub(crate) struct NativeSurface {
    raw: vk::SurfaceKHR,
    functor: khr::surface::Instance,
    instance: Arc<InstanceShared>,
    /// Built from the window's `HWND` (Windows only) to answer the display-HDR
    /// query; `None` for non-Win32 surfaces.
    #[cfg(windows)]
    hdr_source: Option<crate::auxil::dxgi::hdr::DxgiHdrSource>,
    /// A caller-provided `pNext` chain to attach to the [`vk::SwapchainCreateInfoKHR`]
    /// of the next swapchain created for this surface.
    ///
    /// Set only through
    /// [`Surface::set_next_swapchain_create_chain()`](crate::vulkan::Surface::set_next_swapchain_create_chain).
    next_swapchain_create_chain: Mutex<Option<PnextChain>>,
}

impl NativeSurface {
    pub fn from_vk_surface_khr(
        instance: &crate::vulkan::Instance,
        raw: vk::SurfaceKHR,
        hwnd: Option<WindowHandle>,
    ) -> Self {
        #[cfg(not(windows))]
        let _ = hwnd;
        let functor = khr::surface::Instance::new(&instance.shared.entry, &instance.shared.raw);
        Self {
            raw,
            functor,
            instance: Arc::clone(&instance.shared),
            #[cfg(windows)]
            hdr_source: hwnd.map(|wh| crate::auxil::dxgi::hdr::DxgiHdrSource::new(wh.0)),
            next_swapchain_create_chain: Mutex::new(None),
        }
    }

    pub fn as_raw(&self) -> vk::SurfaceKHR {
        self.raw
    }

    /// # Safety
    ///
    /// See [`Surface::set_next_swapchain_create_chain()`](crate::vulkan::Surface::set_next_swapchain_create_chain).
    pub unsafe fn set_next_swapchain_create_chain(&self, chain: *mut core::ffi::c_void) {
        *self.next_swapchain_create_chain.lock() =
            (!chain.is_null()).then(|| PnextChain::new(chain));
    }

    /// Whether a chain set by [`Self::set_next_swapchain_create_chain`] waits for a
    /// configuration to consume it.
    #[cfg(test)]
    pub(crate) fn has_pending_swapchain_create_chain(&self) -> bool {
        self.next_swapchain_create_chain.lock().is_some()
    }

    /// Whether the instance can query the present scaling that the surface supports: it has
    /// `VK_KHR_get_surface_capabilities2` and a surface maintenance extension. The swapchain
    /// maintenance extension of a device depends on the latter.
    fn queries_present_scaling(&self) -> bool {
        let extensions = &self.instance.extensions;
        extensions.contains(&khr::get_surface_capabilities2::NAME)
            && super::super::adapter::SWAPCHAIN_MAINTENANCE1
                .iter()
                .any(|(_, surface_extension)| extensions.contains(surface_extension))
    }

    /// What the surface supports of present scaling with `present_mode`
    /// (`VkSurfacePresentScalingCapabilitiesKHR`).
    ///
    /// # Safety
    ///
    /// - [`Self::queries_present_scaling`] must be true.
    /// - `present_mode` must be one that the surface lists for `physical_device`
    ///   (VUID-VkSurfacePresentModeKHR-presentMode-07780).
    unsafe fn present_scaling_capabilities(
        &self,
        physical_device: vk::PhysicalDevice,
        present_mode: vk::PresentModeKHR,
    ) -> Result<vk::SurfacePresentScalingCapabilitiesEXT<'static>, vk::Result> {
        let functor =
            khr::get_surface_capabilities2::Instance::new(&self.instance.entry, &self.instance.raw);
        let mut scaling = vk::SurfacePresentScalingCapabilitiesEXT::default();
        let mut caps = vk::SurfaceCapabilities2KHR::default().push_next(&mut scaling);
        let mut mode = vk::SurfacePresentModeEXT::default().present_mode(present_mode);
        let info = vk::PhysicalDeviceSurfaceInfo2KHR::default()
            .surface(self.raw)
            .push_next(&mut mode);
        profiling::scope!("vkGetPhysicalDeviceSurfaceCapabilities2KHR");
        // SAFETY: Our caller's contract.
        unsafe {
            functor.get_physical_device_surface_capabilities2(physical_device, &info, &mut caps)
        }?;
        // Last in the chain: its `pNext` is null.
        Ok(scaling)
    }

    /// The scaling modes the surface supports with every present mode of `present_modes` that
    /// wgpu configures, so that a configuration may combine any listed scaling with any listed
    /// present mode (VUID-VkSwapchainPresentScalingCreateInfoKHR-scalingBehavior-07770,
    /// -presentGravityX-07772 and -presentGravityY-07774).
    ///
    /// Only for an adapter whose devices enable `swapchainMaintenance1`
    /// ([`super::super::Adapter::opens_with_swapchain_maintenance1`]): its instance then has the
    /// surface maintenance extension and `VK_KHR_get_surface_capabilities2`.
    fn scaling_modes(
        &self,
        adapter: &crate::vulkan::Adapter,
        present_modes: &[vk::PresentModeKHR],
    ) -> Vec<wgt::SurfaceScalingMode> {
        let mut supported = Vec::new();
        for &present_mode in present_modes
            .iter()
            .filter(|&&mode| conv::map_vk_present_mode(mode).is_some())
        {
            // SAFETY: The instance has the extensions, and `present_mode` is one the surface
            // lists.
            match unsafe { self.present_scaling_capabilities(adapter.raw, present_mode) } {
                Ok(caps) => supported.push(conv::PresentScalingSupport::of(&caps)),
                Err(e) => {
                    log::error!("get_physical_device_surface_capabilities2: {e}");
                    return Vec::new();
                }
            }
        }
        conv::surface_scaling_modes(&supported)
    }

    /// Refuses, with [`crate::SurfaceError::UnsupportedScalingMode`], a configuration whose
    /// scaling mode, other than `Auto`, `device` cannot create this surface's swapchain with
    /// ([`present_scaling_refusal`]).
    fn check_present_scaling(
        &self,
        device: &crate::vulkan::Device,
        config: &crate::SurfaceConfiguration,
    ) -> Result<(), crate::SurfaceError> {
        let refusal = present_scaling_refusal(
            device.shared.swapchain_maintenance1,
            self.queries_present_scaling(),
            || {
                // SAFETY: `present_scaling_refusal` asks only where the instance can query. The
                // present mode of a valid configuration is one the surface lists.
                unsafe {
                    self.present_scaling_capabilities(
                        device.shared.physical_device,
                        conv::map_present_mode(config.present_mode),
                    )
                }
            },
            config.scaling_mode,
            config.extent,
        )
        .map_err(|error| match error {
            vk::Result::ERROR_SURFACE_LOST_KHR => crate::SurfaceError::Lost,
            other => crate::SurfaceError::Device(super::super::map_host_device_oom_err(other)),
        })?;
        match refusal {
            None => Ok(()),
            Some(refusal) => {
                log::warn!(
                    "the surface refuses scaling mode {:?}: {}",
                    config.scaling_mode,
                    refusal.reason()
                );
                Err(crate::SurfaceError::UnsupportedScalingMode)
            }
        }
    }
}

/// Why a swapchain cannot be created with the present scaling of a configuration's scaling mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PresentScalingRefusal {
    /// The device has no `swapchainMaintenance1`
    /// (VUID-VkSwapchainPresentScalingCreateInfoKHR-swapchainMaintenance1-10154).
    NoSwapchainMaintenance1,
    /// The instance cannot query the surface's support: it has no surface maintenance extension or
    /// no `VK_KHR_get_surface_capabilities2`.
    NoSurfaceQuery,
    /// The surface does not support the scaling with the configured present mode, with the gravity
    /// that the mode asks for (VUID-VkSwapchainPresentScalingCreateInfoKHR-scalingBehavior-07770,
    /// -presentGravityX-07772 and -presentGravityY-07774).
    NotWithPresentMode,
    /// The extent is outside the surface's range for scaled presentation
    /// (VUID-VkSwapchainCreateInfoKHR-pNext-07782), which the list of scaling modes cannot express.
    ExtentOutOfRange,
}

impl PresentScalingRefusal {
    fn reason(self) -> &'static str {
        match self {
            Self::NoSwapchainMaintenance1 => "the device has no swapchainMaintenance1",
            Self::NoSurfaceQuery => "the instance cannot query the surface's present scaling",
            Self::NotWithPresentMode => "not supported with the present mode configured",
            Self::ExtentOutOfRange => {
                "the extent is outside the surface's range for scaled presentation"
            }
        }
    }
}

/// Whether a device can create a swapchain with the present scaling of `mode`, other than `Auto`,
/// for a configuration of `extent`, and if not, why. The device comes first: the adapter lists
/// scaling modes for the devices it opens itself, and a device created from a raw device without
/// `swapchainMaintenance1` must not use them (its `supports_present_scaling` is false, so wgpu-core
/// asks it for `Auto` only). Then the instance, and only then `query`, the surface's support with
/// the configured present mode: the list holds for every present mode, but a caller of wgpu-hal
/// may not have read it, and the extent's range is not in it.
fn present_scaling_refusal(
    device_has_swapchain_maintenance1: bool,
    instance_queries_present_scaling: bool,
    query: impl FnOnce() -> Result<vk::SurfacePresentScalingCapabilitiesEXT<'static>, vk::Result>,
    mode: wgt::SurfaceScalingMode,
    extent: wgt::Extent3d,
) -> Result<Option<PresentScalingRefusal>, vk::Result> {
    if !device_has_swapchain_maintenance1 {
        return Ok(Some(PresentScalingRefusal::NoSwapchainMaintenance1));
    }
    if !instance_queries_present_scaling {
        return Ok(Some(PresentScalingRefusal::NoSurfaceQuery));
    }
    let caps = query()?;
    if !conv::surface_scaling_modes(&[conv::PresentScalingSupport::of(&caps)]).contains(&mode) {
        return Ok(Some(PresentScalingRefusal::NotWithPresentMode));
    }
    if !conv::extent_within_scaled_range(&caps, extent) {
        return Ok(Some(PresentScalingRefusal::ExtentOutOfRange));
    }
    Ok(None)
}

impl Drop for NativeSurface {
    fn drop(&mut self) {
        unsafe {
            self.functor.destroy_surface(self.raw, None);
        }
    }
}

impl Surface for NativeSurface {
    fn surface_capabilities(
        &self,
        adapter: &crate::vulkan::Adapter,
    ) -> Option<crate::SurfaceCapabilities> {
        if !adapter.private_caps.can_present {
            return None;
        }
        let queue_family_index = 0; //TODO
        {
            profiling::scope!("vkGetPhysicalDeviceSurfaceSupportKHR");
            match unsafe {
                self.functor.get_physical_device_surface_support(
                    adapter.raw,
                    queue_family_index,
                    self.raw,
                )
            } {
                Ok(true) => (),
                Ok(false) => return None,
                Err(e) => {
                    log::error!("get_physical_device_surface_support: {e}");
                    return None;
                }
            }
        }

        let caps = {
            profiling::scope!("vkGetPhysicalDeviceSurfaceCapabilitiesKHR");
            match unsafe {
                self.functor
                    .get_physical_device_surface_capabilities(adapter.raw, self.raw)
            } {
                Ok(caps) => caps,
                Err(e) => {
                    log::error!("get_physical_device_surface_capabilities: {e}");
                    return None;
                }
            }
        };

        // If image count is 0, the support number of images is unlimited.
        let max_image_count = if caps.max_image_count == 0 {
            !0
        } else {
            caps.max_image_count
        };

        // `0xFFFFFFFF` indicates that the extent depends on the created swapchain.
        let current_extent = if caps.current_extent.width != !0 && caps.current_extent.height != !0
        {
            Some(wgt::Extent3d {
                width: caps.current_extent.width,
                height: caps.current_extent.height,
                depth_or_array_layers: 1,
            })
        } else {
            None
        };

        let raw_present_modes = {
            profiling::scope!("vkGetPhysicalDeviceSurfacePresentModesKHR");
            match unsafe {
                self.functor
                    .get_physical_device_surface_present_modes(adapter.raw, self.raw)
            } {
                Ok(present_modes) => present_modes,
                Err(e) => {
                    log::error!("get_physical_device_surface_present_modes: {e}");
                    // Per definition of `SurfaceCapabilities`, there must be at least one present mode.
                    return None;
                }
            }
        };

        let raw_surface_formats = {
            profiling::scope!("vkGetPhysicalDeviceSurfaceFormatsKHR");
            match unsafe {
                self.functor
                    .get_physical_device_surface_formats(adapter.raw, self.raw)
            } {
                Ok(formats) => formats,
                Err(e) => {
                    log::error!("get_physical_device_surface_formats: {e}");
                    // Per definition of `SurfaceCapabilities`, there must be at least one present format.
                    return None;
                }
            }
        };

        let scaling_modes = if adapter.opens_with_swapchain_maintenance1().is_some() {
            self.scaling_modes(adapter, &raw_present_modes)
        } else {
            Vec::new()
        };

        // Group the driver's (format, color space) pairs into one entry per
        // format, preserving the driver's format order.
        let mut formats: Vec<wgt::SurfaceFormatCapabilities> = Vec::new();
        for (format, color_space) in raw_surface_formats
            .into_iter()
            .filter_map(conv::map_vk_surface_formats)
        {
            let color_spaces = color_space.to_color_spaces().unwrap();
            match formats.iter_mut().find(|fc| fc.format == format) {
                Some(fc) => fc.color_spaces |= color_spaces,
                None => formats.push(wgt::SurfaceFormatCapabilities {
                    format,
                    color_spaces,
                }),
            }
        }
        Some(crate::SurfaceCapabilities {
            formats,
            // TODO: Right now we're always truncating the swap chain
            // (presumably - we're actually setting the min image count which isn't necessarily the swap chain size)
            // Instead, we should use extensions when available to wait in present.
            // See https://github.com/gfx-rs/wgpu/issues/2869
            maximum_frame_latency: (caps.min_image_count - 1)..=(max_image_count - 1), // Note this can't underflow since both `min_image_count` is at least one and we already patched `max_image_count`.
            current_extent,
            usage: conv::map_vk_image_usage(caps.supported_usage_flags),
            present_modes: raw_present_modes
                .into_iter()
                .flat_map(conv::map_vk_present_mode)
                .collect(),
            composite_alpha_modes: conv::map_vk_composite_alpha(caps.supported_composite_alpha),
            scaling_modes,
            supports_background_color: false,
        })
    }

    unsafe fn create_swapchain(
        &self,
        device: &crate::vulkan::Device,
        config: &crate::SurfaceConfiguration,
        provided_old_swapchain: Option<Box<dyn Swapchain>>,
    ) -> Result<Box<dyn Swapchain>, crate::SurfaceError> {
        profiling::scope!("Device::create_swapchain");
        // The present scaling comes first. A configuration refused for its scaling mode leaves
        // the caller's chain set, for the next configuration: wgpu-core configures `Auto` again
        // at once without a scaling. Every other configuration consumes the chain
        // (`Surface::set_next_swapchain_create_chain()`).
        let (create_chain, scaling) = {
            let mut pending_chain = self.next_swapchain_create_chain.lock();
            // A caller's chain may give the swapchain its own present scaling. A structure type
            // appears once in a chain, so the configuration's mode then adds none.
            // SAFETY: The contract on `Surface::set_next_swapchain_create_chain()` keeps the
            // chain valid.
            let callers_scaling = pending_chain.as_ref().is_some_and(|chain| unsafe {
                chain.contains(vk::StructureType::SWAPCHAIN_PRESENT_SCALING_CREATE_INFO_EXT)
            });
            // `Auto` asks for nothing: no scaling structure, the platform's own placement.
            let scaling = match conv::map_surface_scaling_mode(config.scaling_mode) {
                Some(scaling) if !callers_scaling => {
                    match self.check_present_scaling(device, config) {
                        Ok(()) => Some(scaling),
                        Err(crate::SurfaceError::UnsupportedScalingMode) => {
                            return Err(crate::SurfaceError::UnsupportedScalingMode);
                        }
                        Err(other) => {
                            // Any other failure consumes the chain.
                            *pending_chain = None;
                            return Err(other);
                        }
                    }
                }
                _ => None,
            };
            (pending_chain.take(), scaling)
        };

        // A swapchain needs `VK_KHR_swapchain` on the device, and presentation to the surface from
        // the device's queue family (VUID-vkQueuePresentKHR-pSwapchains-01292), which a device
        // created elsewhere may lack: `surface_capabilities` asks of the adapter's family 0.
        if !device
            .shared
            .enabled_extensions
            .contains(&khr::swapchain::NAME)
        {
            return Err(crate::SurfaceError::Other(
                "the device was created without VK_KHR_swapchain",
            ));
        }
        match unsafe {
            self.functor.get_physical_device_surface_support(
                device.shared.physical_device,
                device.shared.family_index,
                self.raw,
            )
        } {
            Ok(true) => {}
            Ok(false) => {
                return Err(crate::SurfaceError::Other(
                    "the device's queue family cannot present to the surface",
                ))
            }
            Err(vk::Result::ERROR_SURFACE_LOST_KHR) => return Err(crate::SurfaceError::Lost),
            Err(other) => {
                return Err(crate::SurfaceError::Device(
                    super::super::map_host_device_oom_err(other),
                ))
            }
        }
        let functor = khr::swapchain::Device::new(&self.instance.raw, &device.shared.raw);

        let old_swapchain = provided_old_swapchain
            .as_ref()
            .map(|osc| osc.as_any().downcast_ref::<NativeSwapchain>().unwrap().raw)
            .unwrap_or(vk::SwapchainKHR::null());

        let color_space = conv::map_surface_color_space(config.color_space);

        let original_format = device.shared.private_caps.map_texture_format(config.format);
        let mut raw_flags = vk::SwapchainCreateFlagsKHR::empty();
        let mut raw_view_formats: Vec<vk::Format> = vec![];
        if !config.view_formats.is_empty() {
            raw_flags |= vk::SwapchainCreateFlagsKHR::MUTABLE_FORMAT;
            raw_view_formats = config
                .view_formats
                .iter()
                .map(|f| device.shared.private_caps.map_texture_format(*f))
                .collect();
            raw_view_formats.push(original_format);
        }

        let mut info = vk::SwapchainCreateInfoKHR::default()
            .flags(raw_flags)
            .surface(self.raw)
            .min_image_count(config.maximum_frame_latency + 1) // TODO: https://github.com/gfx-rs/wgpu/issues/2869
            .image_format(original_format)
            .image_color_space(color_space)
            .image_extent(vk::Extent2D {
                width: config.extent.width,
                height: config.extent.height,
            })
            .image_array_layers(config.extent.depth_or_array_layers)
            .image_usage(conv::map_texture_usage(config.usage))
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
            .composite_alpha(conv::map_composite_alpha_mode(config.composite_alpha_mode))
            .present_mode(conv::map_present_mode(config.present_mode))
            .clipped(true)
            .old_swapchain(old_swapchain);

        let mut format_list_info = vk::ImageFormatListCreateInfo::default();
        if !raw_view_formats.is_empty() {
            format_list_info = format_list_info.view_formats(&raw_view_formats);
            info = info.push_next(&mut format_list_info);
        }

        let mut scaling_info = vk::SwapchainPresentScalingCreateInfoEXT::default();
        if let Some((behavior, gravity)) = scaling {
            scaling_info = scaling_info
                .scaling_behavior(behavior)
                .present_gravity_x(gravity)
                .present_gravity_y(gravity);
            info = info.push_next(&mut scaling_info);
        }

        if let Some(chain) = create_chain {
            // SAFETY: The contract on `Surface::set_next_swapchain_create_chain()` keeps
            // the chain valid and unaliased until this swapchain creation returns.
            info.p_next = unsafe { chain.splice_into(info.p_next) };
        }

        let result = {
            profiling::scope!("vkCreateSwapchainKHR");
            unsafe { functor.create_swapchain(&info, None) }
        };

        let raw = match result {
            Ok(swapchain) => swapchain,
            Err(error) => {
                return Err(match error {
                    vk::Result::ERROR_SURFACE_LOST_KHR
                    | vk::Result::ERROR_INITIALIZATION_FAILED => crate::SurfaceError::Lost,
                    vk::Result::ERROR_NATIVE_WINDOW_IN_USE_KHR => {
                        crate::SurfaceError::Other("Native window is in use")
                    }
                    // We don't use VK_EXT_image_compression_control
                    // VK_ERROR_COMPRESSION_EXHAUSTED_EXT
                    other => map_host_device_oom_and_lost_err(other).into(),
                });
            }
        };

        let images = unsafe { functor.get_swapchain_images(raw) }
            .map_err(crate::vulkan::map_host_device_oom_err)?;

        // This fence is only used to throttle FIFO acquisition on Windows. It is very important to
        // avoid bad frame pacing when the Vulkan driver is using a DXGI swapchain. See
        // https://github.com/gfx-rs/wgpu/issues/8310 and
        // https://github.com/gfx-rs/wgpu/issues/8354 for more details. In other present modes it
        // only adds acquire latency, see https://github.com/gfx-rs/wgpu/issues/9559.
        let fence = if cfg!(target_os = "windows")
            && matches!(
                config.present_mode,
                wgt::PresentMode::Fifo | wgt::PresentMode::FifoRelaxed
            ) {
            let raw = unsafe {
                device
                    .shared
                    .raw
                    .create_fence(&vk::FenceCreateInfo::default(), None)
                    .map_err(crate::vulkan::map_host_device_oom_err)?
            };
            Some(raw)
        } else {
            None
        };

        // NOTE: It's important that we define the same number of acquire/present semaphores
        // as we will need to index into them with the image index.
        let acquire_semaphores = (0..images.len())
            .map(|i| {
                SwapchainAcquireSemaphore::new(&device.shared, i)
                    .map(Mutex::new)
                    .map(Arc::new)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let present_semaphores = (0..images.len())
            .map(|i| Arc::new(Mutex::new(SwapchainPresentSemaphores::new(i))))
            .collect::<Vec<_>>();

        Ok(Box::new(NativeSwapchain {
            raw,
            functor,
            device: Arc::clone(&device.shared),
            images,
            fence,
            config: config.clone(),
            acquire_semaphores,
            next_acquire_index: 0,
            present_semaphores,
            present_fences: device
                .shared
                .swapchain_maintenance1
                .then(PresentFences::default),
            discarded: None,
            acquired_suboptimal: false,
            next_present_time: None,
            next_present_chain: None,
        }))
    }

    fn supports_present_scaling(&self, device: &crate::vulkan::Device) -> bool {
        // A swapchain's present scaling needs `swapchainMaintenance1`
        // (VUID-VkSwapchainPresentScalingCreateInfoKHR-swapchainMaintenance1-10154).
        device.shared.swapchain_maintenance1
    }

    #[cfg(windows)]
    fn display_hdr_info(&self) -> Option<wgt::DisplayHdrInfo> {
        self.hdr_source.as_ref()?.display_hdr_info()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub(crate) struct NativeSwapchain {
    raw: vk::SwapchainKHR,
    functor: khr::swapchain::Device,
    device: Arc<DeviceShared>,
    images: Vec<vk::Image>,
    /// Fence used to wait on the acquired image.
    fence: Option<vk::Fence>,
    config: crate::SurfaceConfiguration,

    /// Semaphores used between image acquisition and the first submission
    /// that uses that image. This is indexed using [`next_acquire_index`].
    ///
    /// Because we need to provide this to [`vkAcquireNextImageKHR`], we haven't
    /// received the swapchain image index for the frame yet, so we cannot use
    /// that to index it.
    ///
    /// Before we pass this to [`vkAcquireNextImageKHR`], we ensure that we wait on
    /// the submission indicated by [`previously_used_submission_index`]. This ensures
    /// the semaphore is no longer in use before we use it.
    ///
    /// [`next_acquire_index`]: NativeSwapchain::next_acquire_index
    /// [`vkAcquireNextImageKHR`]: https://registry.khronos.org/vulkan/specs/1.3-extensions/html/vkspec.html#vkAcquireNextImageKHR
    /// [`previously_used_submission_index`]: SwapchainAcquireSemaphore::previously_used_submission_index
    acquire_semaphores: Vec<Arc<Mutex<SwapchainAcquireSemaphore>>>,
    /// The index of the next acquire semaphore to use.
    ///
    /// This is incremented each time we acquire a new image, and wraps around
    /// to 0 when it reaches the end of [`acquire_semaphores`].
    ///
    /// [`acquire_semaphores`]: NativeSwapchain::acquire_semaphores
    next_acquire_index: usize,

    /// Semaphore sets used between all submissions that write to an image and
    /// the presentation of that image.
    ///
    /// This is indexed by the swapchain image index returned by
    /// [`vkAcquireNextImageKHR`].
    ///
    /// We know it is safe to use these semaphores because use them
    /// _after_ the acquire semaphore. Because the acquire semaphore
    /// has been signaled, the previous presentation using that image
    /// is known-finished, so this semaphore is no longer in use.
    ///
    /// [`vkAcquireNextImageKHR`]: https://registry.khronos.org/vulkan/specs/1.3-extensions/html/vkspec.html#vkAcquireNextImageKHR
    present_semaphores: Vec<Arc<Mutex<SwapchainPresentSemaphores>>>,

    /// The fences that the presents to the swapchain signal, on which its teardown waits, where
    /// presents signal fences ([`DeviceShared::swapchain_maintenance1`]).
    present_fences: Option<PresentFences>,

    /// The texture last discarded, which the next [`acquire`] returns.
    ///
    /// Vulkan has no core way to give an acquired image back to the presentation engine without
    /// presenting it, so a discarded image stays acquired, and the next acquisition hands it out
    /// again instead of acquiring another image. Its semaphores go with it: the acquire semaphore,
    /// still to be waited for if no submission has used the image yet, and the present semaphores
    /// signaled by the submissions that have, which the presentation of the image waits for.
    ///
    /// [`acquire`]: Swapchain::acquire
    discarded: Option<crate::AcquiredSurfaceTexture<crate::api::Vulkan>>,

    /// Whether `vkAcquireNextImageKHR` reported the image it last acquired as suboptimal. Only
    /// one image is acquired at a time, so a discarded texture has that image, and is handed out
    /// again as suboptimal as it was.
    acquired_suboptimal: bool,

    /// The present timing information which will be set in the next call to [`present()`](crate::Queue::present()).
    ///
    /// # Safety
    ///
    /// This must only be set if the VK_GOOGLE_display_timing extension is enabled.
    next_present_time: Option<vk::PresentTimeGOOGLE>,

    /// A caller-provided `pNext` chain to attach to the [`vk::PresentInfoKHR`] of the next
    /// call to [`present()`](crate::Queue::present()).
    ///
    /// Set only through
    /// [`Surface::set_next_present_chain()`](crate::vulkan::Surface::set_next_present_chain).
    next_present_chain: Option<PnextChain>,
}

impl Drop for NativeSwapchain {
    fn drop(&mut self) {
        unsafe {
            self.functor.destroy_swapchain(self.raw, None);
        }
    }
}

/// `VK_ERROR_PRESENT_TIMING_QUEUE_FULL_EXT` (`VK_EXT_present_timing`), which `ash` 0.38 does not
/// define. wgpu-hal does not use the extension, but a caller's present chain may.
const ERROR_PRESENT_TIMING_QUEUE_FULL_EXT: vk::Result = vk::Result::from_raw(-1_000_208_000);

/// The fences that a swapchain's presents signal, with `swapchainMaintenance1`
/// (`VkSwapchainPresentFenceInfoKHR`). A present's fence signals once the present's queue
/// operations have completed and the presentation engine has taken its references, and before the
/// fence of any later present to the swapchain on the same queue. The swapchain's teardown waits
/// on them, and so for its presents alone, without the queue lock.
#[derive(Debug, Default)]
struct PresentFences {
    /// The fences of the presents enqueued, oldest first.
    pending: VecDeque<vk::Fence>,
    /// Unsignaled fences that no queue operation uses, for the next presents.
    free: Vec<vk::Fence>,
    /// The fences of presents that failed in a way that leaves unsaid whether they were
    /// enqueued: they may never signal.
    unknown: Vec<vk::Fence>,
    /// Whether a present went without a fence of the swapchain's: creating one failed, or the
    /// caller's present chain gave its own.
    unfenced: bool,
}

impl PresentFences {
    /// A fence for the next present: one of a present that has completed, reset, else a free
    /// one, else a new one. `None`, and the swapchain's teardown waits for the queue instead, if
    /// creating one failed.
    fn next(&mut self, device: &ash::Device) -> Option<vk::Fence> {
        // Fences signal in the order of their presents: the oldest unsignaled one ends the search.
        while let Some(&oldest) = self.pending.front() {
            // SAFETY: A fence of the device.
            if unsafe { device.get_fence_status(oldest) } != Ok(true) {
                break;
            }
            // SAFETY: Signaled, so no queue operation uses it.
            if unsafe { device.reset_fences(&[oldest]) }.is_err() {
                break;
            }
            self.pending.pop_front();
            self.free.push(oldest);
        }
        if let Some(fence) = self.free.pop() {
            return Some(fence);
        }
        // SAFETY: A valid create info.
        match unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) } {
            Ok(fence) => Some(fence),
            Err(error) => {
                log::warn!("Presenting without a fence, as creating one failed: {error}");
                self.unfenced = true;
                None
            }
        }
    }

    /// Files `fence` after the present that it was given to returned `result`.
    fn presented(&mut self, fence: vk::Fence, result: Result<bool, vk::Result>) {
        match result {
            // Enqueued: rejected by the presentation engine, the present's queue operations still
            // are. A lost device's fence waits return.
            Ok(_)
            | Err(
                vk::Result::ERROR_OUT_OF_DATE_KHR
                | vk::Result::ERROR_SURFACE_LOST_KHR
                | vk::Result::ERROR_FULL_SCREEN_EXCLUSIVE_MODE_LOST_EXT
                | ERROR_PRESENT_TIMING_QUEUE_FULL_EXT
                | vk::Result::ERROR_DEVICE_LOST,
            ) => self.pending.push_back(fence),
            // Not enqueued, and the fence unaffected.
            Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY) => {
                self.free.push(fence)
            }
            Err(_) => self.unknown.push(fence),
        }
    }

    /// Waits on the fences of the presents enqueued, without the queue lock. Whether that waited
    /// for every present to the swapchain: `false` if a present went without a fence of the
    /// swapchain's or with one of unknown fate, or the wait failed. The caller then waits for the
    /// queue to become idle.
    ///
    /// # Safety
    ///
    /// The fences are `device`'s.
    unsafe fn wait(&mut self, device: &ash::Device) -> bool {
        let waited = self.pending.is_empty()
            // SAFETY: Fences of the device, of presents enqueued.
            || match unsafe {
                device.wait_for_fences(self.pending.make_contiguous(), true, u64::MAX)
            } {
                // A lost device's fences are as good as signaled.
                Ok(()) | Err(vk::Result::ERROR_DEVICE_LOST) => true,
                Err(error) => {
                    log::warn!("Waiting on the presents' fences failed: {error}");
                    false
                }
            };
        waited && !self.unfenced && self.unknown.is_empty()
    }

    /// Destroys every fence.
    ///
    /// # Safety
    ///
    /// No queue operation uses them.
    unsafe fn destroy(&mut self, device: &ash::Device) {
        for fence in self
            .pending
            .drain(..)
            .chain(self.free.drain(..))
            .chain(self.unknown.drain(..))
        {
            // SAFETY: Our caller's contract.
            unsafe { device.destroy_fence(fence, None) };
        }
    }
}

/// Destroy the swapchain semaphores that are no longer referenced by a live
/// surface texture. A still-referenced semaphore is skipped, as the texture
/// may still submit work using it.
///
/// The vectors are drained.
fn destroy_swapchain_semaphores(
    acquire_semaphores: &mut Vec<Arc<Mutex<SwapchainAcquireSemaphore>>>,
    present_semaphores: &mut Vec<Arc<Mutex<SwapchainPresentSemaphores>>>,
    device: &crate::vulkan::Device,
) {
    for semaphore in acquire_semaphores.drain(..) {
        if let Some(mutex_removed) = Arc::into_inner(semaphore) {
            let semaphore_removed = mutex_removed.into_inner();
            unsafe { semaphore_removed.destroy(&device.shared.raw) };
        }
    }

    for semaphore in present_semaphores.drain(..) {
        if let Some(mutex_removed) = Arc::into_inner(semaphore) {
            let semaphore_removed = mutex_removed.into_inner();
            unsafe { semaphore_removed.destroy(&device.shared.raw) };
        }
    }
}

impl Swapchain for NativeSwapchain {
    unsafe fn release_resources(&mut self, device: &crate::vulkan::Device) {
        profiling::scope!("Swapchain::release_resources");
        // Every present was made on the swapchain's device's queue.
        debug_assert!(Arc::ptr_eq(&self.device, &device.shared));
        // The fences of the presents signal once their queue operations have completed: waiting
        // on them takes no queue lock, and waits for nothing else.
        let presents_done = self.present_fences.as_mut().is_some_and(|fences| {
            profiling::scope!("vkWaitForFences");
            unsafe { fences.wait(&self.device.raw) }
        });
        if !presents_done {
            profiling::scope!("vkQueueWaitIdle");
            // The queue's idle wait covers its presents' queue operations (Vulkan 1.4.324).
            let _ = self
                .device
                .with_queue_lock(|| unsafe {
                    self.device.raw.queue_wait_idle(self.device.raw_queue)
                })
                .map_err(map_host_device_oom_and_lost_err);
        }
        if let Some(fences) = self.present_fences.as_mut() {
            // SAFETY: Every present has completed, so no queue operation uses its fence.
            unsafe { fences.destroy(&self.device.raw) };
        }

        if let Some(fence) = self.fence {
            unsafe { device.shared.raw.destroy_fence(fence, None) }
        }

        // A discarded texture holds its semaphores, which would otherwise be skipped below.
        self.discarded = None;
        destroy_swapchain_semaphores(
            &mut self.acquire_semaphores,
            &mut self.present_semaphores,
            device,
        );
    }

    unsafe fn acquire(
        &mut self,
        timeout: Option<core::time::Duration>,
        fence: &crate::vulkan::Fence,
    ) -> Result<crate::AcquiredSurfaceTexture<crate::api::Vulkan>, crate::SurfaceError> {
        let mut timeout_ns = match timeout {
            Some(duration) => duration.as_nanos() as u64,
            None => u64::MAX,
        };

        // AcquireNextImageKHR on Android (prior to Android 11) doesn't support timeouts
        // and will also log verbose warnings if tying to use a timeout.
        //
        // Android 10 implementation for reference:
        // https://android.googlesource.com/platform/frameworks/native/+/refs/tags/android-mainline-10.0.0_r13/vulkan/libvulkan/swapchain.cpp#1426
        // Android 11 implementation for reference:
        // https://android.googlesource.com/platform/frameworks/native/+/refs/tags/android-mainline-11.0.0_r45/vulkan/libvulkan/swapchain.cpp#1438
        //
        // Android 11 corresponds to an SDK_INT/ro.build.version.sdk of 30
        if cfg!(target_os = "android") && self.device.instance.android_sdk_version < 30 {
            timeout_ns = u64::MAX;
        }

        // The image of a discarded texture is still acquired: hand it out again.
        if let Some(discarded) = self.discarded.take() {
            return self.reacquire_discarded(discarded, fence, timeout_ns);
        }

        let acquire_semaphore_arc = self.get_acquire_semaphore();
        // Nothing should be using this, so we don't block, but panic if we fail to lock.
        let acquire_semaphore_guard = acquire_semaphore_arc
            .try_lock()
            .expect("Failed to lock a SwapchainSemaphores.");

        // Wait for all commands writing to the previously acquired image to
        // complete.
        //
        // Almost all the steps in the usual acquire-draw-present flow are
        // asynchronous: they get something started on the presentation engine
        // or the GPU, but on the CPU, control returns immediately. Without some
        // sort of intervention, the CPU could crank out frames much faster than
        // the presentation engine can display them.
        //
        // This is the intervention: if any submissions drew on this image, and
        // thus waited for `locked_swapchain_semaphores.acquire`, wait for all
        // of them to finish, thus ensuring that it's okay to pass `acquire` to
        // `vkAcquireNextImageKHR` again.
        let completed = self.device.wait_for_fence(
            fence,
            acquire_semaphore_guard.previously_used_submission_index,
            timeout_ns,
        )?;
        if !completed {
            return Err(crate::SurfaceError::Timeout);
        }

        let acquire_fence = self.fence.unwrap_or_else(vk::Fence::null);

        // will block if no image is available
        let (index, suboptimal) = match unsafe {
            profiling::scope!("vkAcquireNextImageKHR");
            self.functor.acquire_next_image(
                self.raw,
                timeout_ns,
                acquire_semaphore_guard.acquire,
                acquire_fence,
            )
        } {
            // We treat `VK_SUBOPTIMAL_KHR` as `VK_SUCCESS` on Android.
            // See the comment in `Queue::present`.
            #[cfg(target_os = "android")]
            Ok((index, _)) => (index, false),
            #[cfg(not(target_os = "android"))]
            Ok(pair) => pair,
            Err(error) => {
                return match error {
                    vk::Result::TIMEOUT => Err(crate::SurfaceError::Timeout),
                    vk::Result::NOT_READY | vk::Result::ERROR_OUT_OF_DATE_KHR => {
                        Err(crate::SurfaceError::Outdated)
                    }
                    vk::Result::ERROR_SURFACE_LOST_KHR => Err(crate::SurfaceError::Lost),
                    // We don't use VK_EXT_full_screen_exclusive
                    // VK_ERROR_FULL_SCREEN_EXCLUSIVE_MODE_LOST_EXT
                    other => Err(map_host_device_oom_and_lost_err(other).into()),
                };
            }
        };

        if let Some(fence) = self.fence {
            unsafe {
                // The `wait_all` argument must be `true` to avoid crash on some Android devices. See https://github.com/gfx-rs/wgpu/pull/8769
                self.device
                    .raw
                    .wait_for_fences(&[fence], true, timeout_ns)
                    .map_err(map_host_device_oom_and_lost_err)?;

                self.device
                    .raw
                    .reset_fences(&[fence])
                    .map_err(map_host_device_oom_and_lost_err)?;
            }
        }

        drop(acquire_semaphore_guard);
        // We only advance the surface semaphores if we successfully acquired an image, otherwise
        // we should try to re-acquire using the same semaphores.
        self.advance_acquire_semaphore();
        self.acquired_suboptimal = suboptimal;

        let present_semaphore_arc = self.get_present_semaphores(index);

        // special case for Intel Vulkan returning bizarre values (ugh)
        if self.device.vendor_id == crate::auxil::db::intel::VENDOR && index > 0x100 {
            return Err(crate::SurfaceError::Outdated);
        }

        let identity = self.device.texture_identity_factory.next();

        let texture = crate::vulkan::SurfaceTexture {
            index,
            texture: crate::vulkan::Texture {
                raw: self.images[index as usize],
                drop_guard: None,
                memory: crate::vulkan::TextureMemory::External,
                format: self.config.format,
                copy_size: crate::CopyExtent {
                    width: self.config.extent.width,
                    height: self.config.extent.height,
                    depth: 1,
                },
                identity,
                present_transition: crate::vulkan::PresentTransition::PresentSrc,
            },
            metadata: Box::new(NativeSurfaceTextureMetadata {
                acquire_semaphores: acquire_semaphore_arc,
                present_semaphores: present_semaphore_arc,
            }),
        };
        Ok(crate::AcquiredSurfaceTexture {
            texture,
            suboptimal,
        })
    }

    unsafe fn discard_texture(
        &mut self,
        texture: crate::vulkan::SurfaceTexture,
    ) -> Result<(), crate::SurfaceError> {
        // Only one texture is acquired at a time, so the next acquisition comes before another
        // discard.
        debug_assert!(
            self.discarded.is_none(),
            "a texture was discarded twice without an acquisition in between"
        );
        self.discarded = Some(crate::AcquiredSurfaceTexture {
            texture,
            suboptimal: self.acquired_suboptimal,
        });
        Ok(())
    }

    unsafe fn present(
        &mut self,
        queue: &crate::vulkan::Queue,
        texture: crate::vulkan::SurfaceTexture,
        damage_rects: &[wgt::Rect<u32>],
    ) -> Result<(), crate::SurfaceError> {
        let metadata = texture
            .metadata
            .as_any()
            .downcast_ref::<NativeSurfaceTextureMetadata>()
            .unwrap();
        let mut acquire_semaphore = metadata.acquire_semaphores.lock();
        let mut present_semaphores = metadata.present_semaphores.lock();

        let wait_semaphores = present_semaphores.get_present_wait_semaphores();

        // Reset the acquire and present semaphores internal state
        // to be ready for the next frame.
        //
        // We do this before the actual call to present to ensure that
        // even if this method errors and early outs, we have reset
        // the state for next frame.
        acquire_semaphore.end_semaphore_usage();
        present_semaphores.end_semaphore_usage();

        drop(acquire_semaphore);

        let swapchains = [self.raw];
        let image_indices = [texture.index];
        let vk_info = vk::PresentInfoKHR::default()
            .swapchains(&swapchains)
            .image_indices(&image_indices)
            .wait_semaphores(&wait_semaphores);

        let mut display_timing;
        let present_times;
        let vk_info = if let Some(present_time) = self.next_present_time.take() {
            debug_assert!(
                self.device
                    .enabled_extensions
                    .contains(&google::display_timing::NAME),
                "`next_present_time` should only be set if VK_GOOGLE_display_timing is enabled"
            );
            present_times = [present_time];
            display_timing = vk::PresentTimesInfoGOOGLE::default().times(&present_times);
            // SAFETY: We know that VK_GOOGLE_display_timing is present because of the safety contract on `next_present_time`.
            vk_info.push_next(&mut display_timing)
        } else {
            vk_info
        };

        let rects: Vec<vk::RectLayerKHR>;
        let present_region;
        let mut present_regions;
        let mut vk_info =
            if self.device.private_caps.incremental_present && !damage_rects.is_empty() {
                rects = conv::map_damage_rects(damage_rects);
                present_region = vk::PresentRegionKHR::default().rectangles(&rects);
                present_regions = vk::PresentRegionsKHR::default()
                    .regions(core::slice::from_ref(&present_region));
                vk_info.push_next(&mut present_regions)
            } else {
                vk_info
            };

        let next_present_chain = self.next_present_chain.take();
        // The present signals a fence of the swapchain's, unless the caller's chain gives it one.
        // SAFETY: The contract on `Surface::set_next_present_chain()` keeps the chain valid.
        let present_fence = self.present_fences.as_mut().and_then(|fences| {
            let callers_fence = next_present_chain.as_ref().is_some_and(|chain| unsafe {
                chain.contains(vk::StructureType::SWAPCHAIN_PRESENT_FENCE_INFO_EXT)
            });
            if callers_fence {
                fences.unfenced = true;
                None
            } else {
                fences.next(&self.device.raw)
            }
        });
        let present_fences;
        let mut present_fence_info;
        if let Some(fence) = present_fence {
            present_fences = [fence];
            // `VkSwapchainPresentFenceInfoKHR`, whose structure type has the EXT one's value.
            present_fence_info =
                vk::SwapchainPresentFenceInfoEXT::default().fences(&present_fences);
            vk_info = vk_info.push_next(&mut present_fence_info);
        }

        if let Some(chain) = next_present_chain {
            // SAFETY: The contract on `Surface::set_next_present_chain()` keeps the chain
            // valid and unaliased until this present completes.
            vk_info.p_next = unsafe { chain.splice_into(vk_info.p_next) };
        }

        // Presented with the lock of the queue it is presented on, the swapchain's device's.
        debug_assert!(Arc::ptr_eq(&self.device, &queue.device));
        let suboptimal = {
            profiling::scope!("vkQueuePresentKHR");
            let presented = queue
                .device
                .with_queue_lock(|| unsafe { self.functor.queue_present(queue.raw, &vk_info) });
            if let (Some(fences), Some(fence)) = (self.present_fences.as_mut(), present_fence) {
                fences.presented(fence, presented);
            }
            presented.map_err(|error| {
                match error {
                    vk::Result::ERROR_OUT_OF_DATE_KHR => crate::SurfaceError::Outdated,
                    vk::Result::ERROR_SURFACE_LOST_KHR => crate::SurfaceError::Lost,
                    // We don't use VK_EXT_full_screen_exclusive
                    // VK_ERROR_FULL_SCREEN_EXCLUSIVE_MODE_LOST_EXT
                    _ => map_host_device_oom_and_lost_err(error).into(),
                }
            })?
        };
        if suboptimal {
            // We treat `VK_SUBOPTIMAL_KHR` as `VK_SUCCESS` on Android.
            // On Android 10+, libvulkan's `vkQueuePresentKHR` implementation returns `VK_SUBOPTIMAL_KHR` if not doing pre-rotation
            // (i.e `VkSwapchainCreateInfoKHR::preTransform` not being equal to the current device orientation).
            // This is always the case when the device orientation is anything other than the identity one, as we unconditionally use `VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR`.
            #[cfg(not(target_os = "android"))]
            log::debug!("Suboptimal present of frame {}", texture.index);
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl NativeSwapchain {
    pub(crate) fn as_raw(&self) -> vk::SwapchainKHR {
        self.raw
    }

    pub fn set_next_present_time(&mut self, present_timing: vk::PresentTimeGOOGLE) {
        if self
            .device
            .enabled_extensions
            .contains(&google::display_timing::NAME)
        {
            self.next_present_time = Some(present_timing);
        } else {
            panic!(
                "Tried to set display timing properties without the VK_GOOGLE_display_timing extension enabled."
            );
        }
    }

    /// # Safety
    ///
    /// See [`Surface::set_next_present_chain()`](crate::vulkan::Surface::set_next_present_chain).
    pub unsafe fn set_next_present_chain(&mut self, chain: *mut core::ffi::c_void) {
        self.next_present_chain = Some(PnextChain::new(chain));
    }

    /// Hands out `discarded` again, once the submissions that used its image have finished.
    ///
    /// Waiting for them throttles the CPU as acquiring another image would: a loop that renders
    /// to every texture and discards it would otherwise never wait. It also lets the present
    /// semaphores that those submissions signaled be destroyed. Only a presentation waits for
    /// them, so they would otherwise pile up, one per submission, until the image is presented.
    fn reacquire_discarded(
        &mut self,
        discarded: crate::AcquiredSurfaceTexture<crate::api::Vulkan>,
        fence: &crate::vulkan::Fence,
        timeout_ns: u64,
    ) -> Result<crate::AcquiredSurfaceTexture<crate::api::Vulkan>, crate::SurfaceError> {
        let metadata = discarded
            .texture
            .metadata
            .as_any()
            .downcast_ref::<NativeSurfaceTextureMetadata>()
            .unwrap();
        let last_use = metadata
            .acquire_semaphores
            .lock()
            .previously_used_submission_index;
        let completed = match self.device.wait_for_fence(fence, last_use, timeout_ns) {
            Ok(completed) => completed,
            Err(error) => {
                self.discarded = Some(discarded);
                return Err(error.into());
            }
        };
        if !completed {
            self.discarded = Some(discarded);
            return Err(crate::SurfaceError::Timeout);
        }
        // SAFETY: The submissions that signaled them have finished.
        unsafe {
            metadata
                .present_semaphores
                .lock()
                .destroy_signaled(&self.device.raw)
        };
        Ok(discarded)
    }

    /// Mark the current frame finished, advancing to the next acquire semaphore.
    fn advance_acquire_semaphore(&mut self) {
        let semaphore_count = self.acquire_semaphores.len();
        self.next_acquire_index = (self.next_acquire_index + 1) % semaphore_count;
    }

    /// Get the next acquire semaphore that should be used with this swapchain.
    fn get_acquire_semaphore(&self) -> Arc<Mutex<SwapchainAcquireSemaphore>> {
        self.acquire_semaphores[self.next_acquire_index].clone()
    }

    /// Get the set of present semaphores that should be used with the given image index.
    fn get_present_semaphores(&self, index: u32) -> Arc<Mutex<SwapchainPresentSemaphores>> {
        self.present_semaphores[index as usize].clone()
    }
}

/// Semaphore used to acquire a swapchain image.
#[derive(Debug)]
struct SwapchainAcquireSemaphore {
    /// A semaphore that is signaled when this image is safe for us to modify.
    ///
    /// When [`vkAcquireNextImageKHR`] returns the index of the next swapchain
    /// image that we should use, that image may actually still be in use by the
    /// presentation engine, and is not yet safe to modify. However, that
    /// function does accept a semaphore that it will signal when the image is
    /// indeed safe to begin messing with.
    ///
    /// This semaphore is:
    ///
    /// - waited for by the first queue submission to operate on this image
    ///   since it was acquired, and
    ///
    /// - signaled by [`vkAcquireNextImageKHR`] when the acquired image is ready
    ///   for us to use.
    ///
    /// [`vkAcquireNextImageKHR`]: https://registry.khronos.org/vulkan/specs/1.3-extensions/html/vkspec.html#vkAcquireNextImageKHR
    acquire: vk::Semaphore,

    /// True if the next command submission operating on this image should wait
    /// for [`acquire`].
    ///
    /// We must wait for `acquire` before drawing to this swapchain image, but
    /// because `wgpu-hal` queue submissions are always strongly ordered, only
    /// the first submission that works with a swapchain image actually needs to
    /// wait. We set this flag when this image is acquired, and clear it the
    /// first time it's passed to [`Queue::submit`] as a surface texture.
    ///
    /// Additionally, semaphores can only be waited on once, so we need to ensure
    /// that we only actually pass this semaphore to the first submission that
    /// uses that image.
    ///
    /// [`acquire`]: SwapchainAcquireSemaphore::acquire
    /// [`Queue::submit`]: crate::Queue::submit
    should_wait_for_acquire: bool,

    /// The fence value of the last command submission that wrote to this image.
    ///
    /// The next time we try to acquire this image, we'll block until
    /// this submission finishes, proving that [`acquire`] is ready to
    /// pass to `vkAcquireNextImageKHR` again.
    ///
    /// [`acquire`]: SwapchainAcquireSemaphore::acquire
    previously_used_submission_index: crate::FenceValue,
}

impl SwapchainAcquireSemaphore {
    fn new(device: &DeviceShared, index: usize) -> Result<Self, crate::DeviceError> {
        Ok(Self {
            acquire: device
                .new_binary_semaphore(&format!("SwapchainImageSemaphore: Index {index} acquire"))?,
            should_wait_for_acquire: true,
            previously_used_submission_index: 0,
        })
    }

    /// Sets the fence value which the next acquire will wait for. This prevents
    /// the semaphore from being used while the previous submission is still in flight.
    fn set_used_fence_value(&mut self, value: crate::FenceValue) {
        self.previously_used_submission_index = value;
    }

    /// Return the semaphore that commands drawing to this image should wait for, if any.
    ///
    /// This only returns `Some` once per acquisition; see
    /// [`SwapchainAcquireSemaphore::should_wait_for_acquire`] for details.
    fn get_acquire_wait_semaphore(&mut self) -> Option<vk::Semaphore> {
        if self.should_wait_for_acquire {
            self.should_wait_for_acquire = false;
            Some(self.acquire)
        } else {
            None
        }
    }

    /// Indicates the cpu-side usage of this semaphore has finished for the frame,
    /// so reset internal state to be ready for the next frame.
    fn end_semaphore_usage(&mut self) {
        // Reset the acquire semaphore, so that the next time we acquire this
        // image, we can wait for it again.
        self.should_wait_for_acquire = true;
    }

    unsafe fn destroy(&self, device: &ash::Device) {
        unsafe {
            device.destroy_semaphore(self.acquire, None);
        }
    }
}

#[derive(Debug)]
struct SwapchainPresentSemaphores {
    /// A pool of semaphores for ordering presentation after drawing.
    ///
    /// The first [`present_index`] semaphores in this vector are:
    ///
    /// - all waited on by the call to [`vkQueuePresentKHR`] that presents this
    ///   image, and
    ///
    /// - each signaled by some [`vkQueueSubmit`] queue submission that draws to
    ///   this image, when the submission finishes execution.
    ///
    /// This vector accumulates one semaphore per submission that writes to this
    /// image. This is awkward, but hard to avoid: [`vkQueuePresentKHR`]
    /// requires a semaphore to order it with respect to drawing commands, and
    /// we can't attach new completion semaphores to a command submission after
    /// it's been submitted. This means that, at submission time, we must create
    /// the semaphore we might need if the caller's next action is to enqueue a
    /// presentation of this image.
    ///
    /// An alternative strategy would be for presentation to enqueue an empty
    /// submit, ordered relative to other submits in the usual way, and
    /// signaling a single presentation semaphore. But we suspect that submits
    /// are usually expensive enough, and semaphores usually cheap enough, that
    /// performance-sensitive users will avoid making many submits, so that the
    /// cost of accumulated semaphores will usually be less than the cost of an
    /// additional submit.
    ///
    /// Only the first [`present_index`] semaphores in the vector are actually
    /// going to be signalled by submitted commands, and need to be waited for
    /// by the next present call. Any semaphores beyond that index were created
    /// for prior presents and are simply being retained for recycling.
    ///
    /// [`present_index`]: SwapchainPresentSemaphores::present_index
    /// [`vkQueuePresentKHR`]: https://registry.khronos.org/vulkan/specs/1.3-extensions/html/vkspec.html#vkQueuePresentKHR
    /// [`vkQueueSubmit`]: https://registry.khronos.org/vulkan/specs/1.3-extensions/html/vkspec.html#vkQueueSubmit
    present: Vec<vk::Semaphore>,

    /// The number of semaphores in [`present`] to be signalled for this submission.
    ///
    /// [`present`]: SwapchainPresentSemaphores::present
    present_index: usize,

    /// Which image this semaphore set is used for.
    frame_index: usize,
}

impl SwapchainPresentSemaphores {
    pub fn new(frame_index: usize) -> Self {
        Self {
            present: Vec::new(),
            present_index: 0,
            frame_index,
        }
    }

    /// Return the semaphore that the next submission that writes to this image should
    /// signal when it's done.
    ///
    /// See [`SwapchainPresentSemaphores::present`] for details.
    fn get_submit_signal_semaphore(
        &mut self,
        device: &DeviceShared,
    ) -> Result<vk::Semaphore, crate::DeviceError> {
        // Try to recycle a semaphore we created for a previous presentation.
        let sem = match self.present.get(self.present_index) {
            Some(sem) => *sem,
            None => {
                let sem = device.new_binary_semaphore(&format!(
                    "SwapchainImageSemaphore: Image {} present semaphore {}",
                    self.frame_index, self.present_index
                ))?;
                self.present.push(sem);
                sem
            }
        };

        self.present_index += 1;

        Ok(sem)
    }

    /// Indicates the cpu-side usage of this semaphore has finished for the frame,
    /// so reset internal state to be ready for the next frame.
    fn end_semaphore_usage(&mut self) {
        // Reset the index to 0, so that the next time we get a semaphore, we
        // start from the beginning of the list.
        self.present_index = 0;
    }

    /// Destroys the semaphores that the submissions since the last presentation signaled, so
    /// that the next presentation waits only for the submissions made from now on.
    ///
    /// # Safety
    ///
    /// The submissions that signaled them must have finished.
    unsafe fn destroy_signaled(&mut self, device: &ash::Device) {
        for sem in self.present.drain(..self.present_index) {
            unsafe { device.destroy_semaphore(sem, None) };
        }
        self.present_index = 0;
    }

    /// Return the semaphores that a presentation of this image should wait on.
    ///
    /// Return a slice of semaphores that the call to [`vkQueueSubmit`] that
    /// ends this image's acquisition should wait for. See
    /// [`SwapchainPresentSemaphores::present`] for details.
    ///
    /// Reset `self` to be ready for the next acquisition cycle.
    ///
    /// [`vkQueueSubmit`]: https://registry.khronos.org/vulkan/specs/1.3-extensions/html/vkspec.html#vkQueueSubmit
    fn get_present_wait_semaphores(&mut self) -> Vec<vk::Semaphore> {
        self.present[0..self.present_index].to_vec()
    }

    unsafe fn destroy(&self, device: &ash::Device) {
        unsafe {
            for sem in &self.present {
                device.destroy_semaphore(*sem, None);
            }
        }
    }
}

#[derive(Debug)]
struct NativeSurfaceTextureMetadata {
    acquire_semaphores: Arc<Mutex<SwapchainAcquireSemaphore>>,
    present_semaphores: Arc<Mutex<SwapchainPresentSemaphores>>,
}

impl SurfaceTextureMetadata for NativeSurfaceTextureMetadata {
    fn get_semaphore_guard(&self) -> Box<dyn SwapchainSubmissionSemaphoreGuard + '_> {
        Box::new(NativeSwapchainSubmissionSemaphoreGuard {
            acquire_semaphore_guard: self
                .acquire_semaphores
                .try_lock()
                .expect("Failed to lock surface acquire semaphore"),
            present_semaphores_guard: self
                .present_semaphores
                .try_lock()
                .expect("Failed to lock surface present semaphores"),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct NativeSwapchainSubmissionSemaphoreGuard<'a> {
    acquire_semaphore_guard: MutexGuard<'a, SwapchainAcquireSemaphore>,
    present_semaphores_guard: MutexGuard<'a, SwapchainPresentSemaphores>,
}

impl<'a> SwapchainSubmissionSemaphoreGuard for NativeSwapchainSubmissionSemaphoreGuard<'a> {
    fn set_used_fence_value(&mut self, value: u64) {
        self.acquire_semaphore_guard.set_used_fence_value(value);
    }

    fn get_acquire_wait_semaphore(&mut self) -> Option<SemaphoreType> {
        self.acquire_semaphore_guard
            .get_acquire_wait_semaphore()
            .map(SemaphoreType::Binary)
    }

    fn get_submit_signal_semaphore(
        &mut self,
        device: &DeviceShared,
    ) -> Result<SemaphoreType, crate::DeviceError> {
        self.present_semaphores_guard
            .get_submit_signal_semaphore(device)
            .map(SemaphoreType::Binary)
    }
}

#[cfg(test)]
mod tests {
    use super::{present_scaling_refusal, PresentScalingRefusal};
    use ash::vk;
    use wgt::SurfaceScalingMode as Mode;

    const EXTENT: wgt::Extent3d = wgt::Extent3d {
        width: 400,
        height: 300,
        depth_or_array_layers: 1,
    };

    /// A surface's support with one present mode: every scaling and gravity, scaled extents from
    /// 100x100 to 800x600.
    fn all() -> vk::SurfacePresentScalingCapabilitiesEXT<'static> {
        let gravity = vk::PresentGravityFlagsEXT::MIN | vk::PresentGravityFlagsEXT::CENTERED;
        vk::SurfacePresentScalingCapabilitiesEXT {
            supported_present_scaling: vk::PresentScalingFlagsEXT::STRETCH
                | vk::PresentScalingFlagsEXT::ASPECT_RATIO_STRETCH
                | vk::PresentScalingFlagsEXT::ONE_TO_ONE,
            supported_present_gravity_x: gravity,
            supported_present_gravity_y: gravity,
            min_scaled_image_extent: vk::Extent2D {
                width: 100,
                height: 100,
            },
            max_scaled_image_extent: vk::Extent2D {
                width: 800,
                height: 600,
            },
            ..Default::default()
        }
    }

    fn refusal(
        device: bool,
        instance: bool,
        caps: vk::SurfacePresentScalingCapabilitiesEXT<'static>,
        mode: Mode,
        extent: wgt::Extent3d,
    ) -> Option<PresentScalingRefusal> {
        present_scaling_refusal(device, instance, || Ok(caps), mode, extent).unwrap()
    }

    /// A device without `swapchainMaintenance1` is refused each mode before the surface is asked,
    /// whatever the instance can do; an instance that cannot query is refused before it asks.
    #[test]
    fn the_device_then_the_instance_refuse_before_the_surface_is_asked() {
        for mode in [Mode::Stretch, Mode::AspectRatio, Mode::OneToOne] {
            for instance in [false, true] {
                assert_eq!(
                    present_scaling_refusal(
                        false,
                        instance,
                        || panic!("asked the surface"),
                        mode,
                        EXTENT
                    ),
                    Ok(Some(PresentScalingRefusal::NoSwapchainMaintenance1))
                );
            }
            assert_eq!(
                present_scaling_refusal(true, false, || panic!("asked the surface"), mode, EXTENT),
                Ok(Some(PresentScalingRefusal::NoSurfaceQuery))
            );
            assert_eq!(refusal(true, true, all(), mode, EXTENT), None);
        }
    }

    /// The configured present mode's support decides: a scaling it lacks, or the gravity that the
    /// mode asks for on either axis, refuses the mode.
    #[test]
    fn a_mode_the_present_mode_lacks_with_its_gravity_is_refused() {
        let stretch_only = vk::SurfacePresentScalingCapabilitiesEXT {
            supported_present_scaling: vk::PresentScalingFlagsEXT::STRETCH,
            ..all()
        };
        assert_eq!(
            refusal(true, true, stretch_only, Mode::Stretch, EXTENT),
            None
        );
        for mode in [Mode::AspectRatio, Mode::OneToOne] {
            assert_eq!(
                refusal(true, true, stretch_only, mode, EXTENT),
                Some(PresentScalingRefusal::NotWithPresentMode)
            );
        }
        // One-to-one needs `MIN` on both axes, the aspect-ratio stretch `CENTERED`.
        let centered_x = vk::SurfacePresentScalingCapabilitiesEXT {
            supported_present_gravity_x: vk::PresentGravityFlagsEXT::CENTERED,
            ..all()
        };
        assert_eq!(
            refusal(true, true, centered_x, Mode::OneToOne, EXTENT),
            Some(PresentScalingRefusal::NotWithPresentMode)
        );
        assert_eq!(
            refusal(true, true, centered_x, Mode::AspectRatio, EXTENT),
            None
        );
        let min_y = vk::SurfacePresentScalingCapabilitiesEXT {
            supported_present_gravity_y: vk::PresentGravityFlagsEXT::MIN,
            ..all()
        };
        assert_eq!(
            refusal(true, true, min_y, Mode::AspectRatio, EXTENT),
            Some(PresentScalingRefusal::NotWithPresentMode)
        );
        assert_eq!(refusal(true, true, min_y, Mode::OneToOne, EXTENT), None);
    }

    /// An extent outside the range for scaled presentation refuses a supported mode; its ends are
    /// inside.
    #[test]
    fn an_extent_outside_the_scaled_range_is_refused() {
        let extent = |width, height| wgt::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        for (width, height, inside) in [
            (100, 100, true),
            (800, 600, true),
            (99, 300, false),
            (400, 99, false),
            (801, 300, false),
            (400, 601, false),
        ] {
            assert_eq!(
                refusal(true, true, all(), Mode::Stretch, extent(width, height)),
                (!inside).then_some(PresentScalingRefusal::ExtentOutOfRange),
                "{width}x{height}"
            );
        }
    }

    /// A query that fails is an error, not a refusal.
    #[test]
    fn a_failed_query_is_an_error() {
        assert_eq!(
            present_scaling_refusal(
                true,
                true,
                || Err(vk::Result::ERROR_SURFACE_LOST_KHR),
                Mode::Stretch,
                EXTENT
            ),
            Err(vk::Result::ERROR_SURFACE_LOST_KHR)
        );
    }
}
