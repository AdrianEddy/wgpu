/*! Presentation.

## Lifecycle

Whenever a submission detects the use of any surface texture, it adds it to the device
tracker for the duration of the submission (temporarily, while recording).
It's added with `UNINITIALIZED` state and transitioned into `empty()` state.
When this texture is presented, we remove it from the device tracker as well as
extract it from the hub.
!*/

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::mem::{self, ManuallyDrop};

#[cfg(feature = "trace")]
use crate::device::trace::{Action, IntoTrace};
use crate::{
    conv,
    device::{
        callbacks::CallbackScope, queue::Queue, Device, DeviceError, MissingDownlevelFlags,
        WaitIdleError,
    },
    hal_label,
    instance::Surface,
    resource::{self, Labeled},
    weak_vec::WeakVec,
};

use thiserror::Error;
use wgt::{
    error::{ErrorType, WebGpuError},
    SurfaceStatus as Status,
};

const FRAME_TIMEOUT_MS: u32 = 1000;

#[derive(Debug)]
pub(crate) struct Presentation {
    pub(crate) device: Arc<Device>,
    pub(crate) config: wgt::SurfaceConfiguration<Vec<wgt::TextureFormat>>,
    pub(crate) acquired_texture: Option<Arc<resource::Texture>>,
    /// The textures acquired with this configuration, which the application can keep
    /// referencing after they are presented or discarded. See [`Self::destroy_texture_views`].
    pub(crate) textures: WeakVec<resource::Texture>,
}

impl Presentation {
    pub(crate) fn new(
        device: Arc<Device>,
        config: wgt::SurfaceConfiguration<Vec<wgt::TextureFormat>>,
    ) -> Self {
        Self {
            device,
            config,
            acquired_texture: None,
            textures: WeakVec::new(),
        }
    }

    /// Destroys the HAL views of the textures acquired with this configuration that are still
    /// referenced, before the HAL surface is reconfigured or unconfigured.
    ///
    /// The HAL requires them destroyed by then (see `hal::Surface::configure`;
    /// <https://github.com/gfx-rs/wgpu/issues/4105>). Presenting, discarding or releasing a
    /// texture destroys it, but its clear view, and the views of its
    /// [`TextureView`](resource::TextureView)s, live as long as those do, and the application
    /// can keep them alive: `SurfaceTexture`s, texture and view handles, bind groups.
    ///
    /// Nothing observable changes for the application: the textures are destroyed already, so
    /// their views fail validation wherever they are used.
    ///
    /// No texture may be acquired, and the GPU must be done with all of them.
    pub(crate) fn destroy_texture_views(&mut self) {
        let textures = mem::take(&mut self.textures)
            .into_iter()
            .filter_map(|texture| texture.upgrade())
            .collect::<Vec<_>>();
        if textures.is_empty() {
            return;
        }
        let mut snatch_guard = self.device.snatchable_lock.write();
        for texture in &textures {
            texture.destroy_surface_texture_views(&mut snatch_guard);
        }
        // Released before the last references to the textures can be dropped.
        drop(snatch_guard);
    }
}

#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum SurfaceError {
    #[error("Surface is invalid")]
    Invalid,
    #[error("Surface is not configured for presentation")]
    NotConfigured,
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error("Surface image is already acquired")]
    AlreadyAcquired,
    #[error("No surface image is currently acquired to present")]
    NothingToPresent,
    #[error("Texture has been destroyed")]
    TextureDestroyed,
}

impl WebGpuError for SurfaceError {
    fn webgpu_error_type(&self) -> ErrorType {
        match self {
            Self::Device(e) => e.webgpu_error_type(),
            Self::Invalid
            | Self::NotConfigured
            | Self::AlreadyAcquired
            | Self::NothingToPresent
            | Self::TextureDestroyed => ErrorType::Validation,
        }
    }
}

#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum ConfigureSurfaceError {
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error("Invalid surface")]
    InvalidSurface,
    #[error("The view format {0:?} is not compatible with texture format {1:?}, only changing srgb-ness is allowed.")]
    InvalidViewFormat(wgt::TextureFormat, wgt::TextureFormat),
    #[error(transparent)]
    MissingDownlevelFlags(#[from] MissingDownlevelFlags),
    #[error("The `SurfaceOutput` returned by `get_current_texture` must be dropped before re-configuring via `configure` or  retrieving a new texture via `get_current_texture`.")]
    PreviousOutputExists,
    #[error("Failed to wait for GPU to come idle before reconfiguring the Surface")]
    GpuWaitTimeout,
    #[error("Both `Surface` width and height must be non-zero. Wait to recreate the `Surface` until the window has non-zero area.")]
    ZeroArea,
    #[error("`Surface` width and height must be within the maximum supported texture size. Requested was ({width}, {height}), maximum extent for either dimension is {max_texture_dimension_2d}.")]
    TooLarge {
        width: u32,
        height: u32,
        max_texture_dimension_2d: u32,
    },
    #[error("Surface does not support the adapter's queue family")]
    UnsupportedQueueFamily,
    #[error("Requested format {requested:?} is not in list of supported formats: {available:?}")]
    UnsupportedFormat {
        requested: wgt::TextureFormat,
        available: Vec<wgt::TextureFormat>,
    },
    #[error("Requested color space {requested:?} is not in the list of color spaces supported for format {format:?}: {available:?}")]
    UnsupportedColorSpace {
        requested: wgt::SurfaceColorSpace,
        format: wgt::TextureFormat,
        available: wgt::SurfaceColorSpaces,
    },
    #[error("Requested present mode {requested:?} is not in the list of supported present modes: {available:?}")]
    UnsupportedPresentMode {
        requested: wgt::PresentMode,
        available: Vec<wgt::PresentMode>,
    },
    #[error("Requested alpha mode {requested:?} is not in the list of supported alpha modes: {available:?}")]
    UnsupportedAlphaMode {
        requested: wgt::CompositeAlphaMode,
        available: Vec<wgt::CompositeAlphaMode>,
    },
    #[error("Requested usage {requested:?} is not in the list of supported usages: {available:?}")]
    UnsupportedUsage {
        requested: wgt::TextureUses,
        available: wgt::TextureUses,
    },
}

impl From<WaitIdleError> for ConfigureSurfaceError {
    fn from(e: WaitIdleError) -> Self {
        match e {
            WaitIdleError::Device(d) => ConfigureSurfaceError::Device(d),
            WaitIdleError::WrongSubmissionIndex(..) => unreachable!(),
            WaitIdleError::Timeout => ConfigureSurfaceError::GpuWaitTimeout,
        }
    }
}

impl WebGpuError for ConfigureSurfaceError {
    fn webgpu_error_type(&self) -> ErrorType {
        match self {
            Self::Device(e) => e.webgpu_error_type(),
            Self::MissingDownlevelFlags(e) => e.webgpu_error_type(),
            Self::InvalidSurface
            | Self::InvalidViewFormat(..)
            | Self::PreviousOutputExists
            | Self::GpuWaitTimeout
            | Self::ZeroArea
            | Self::TooLarge { .. }
            | Self::UnsupportedQueueFamily
            | Self::UnsupportedFormat { .. }
            | Self::UnsupportedColorSpace { .. }
            | Self::UnsupportedPresentMode { .. }
            | Self::UnsupportedAlphaMode { .. }
            | Self::UnsupportedUsage { .. } => ErrorType::Validation,
        }
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct SurfaceOutput<T = Arc<resource::Texture>> {
    pub status: Status,
    pub texture: Option<T>,
}

impl Surface {
    pub fn get_current_texture(self: &Arc<Self>) -> Result<SurfaceOutput, SurfaceError> {
        // A lost device is often detected here: its callback is delivered when this returns.
        let _scope = CallbackScope::new(&self.pending_callbacks);
        let output = self.get_current_texture_inner();
        #[cfg(feature = "trace")]
        if let Some(present) = self.presentation.lock().as_ref() {
            if let Some(ref mut trace) = *present.device.trace.lock() {
                if let Some(texture) = present.acquired_texture.as_ref() {
                    trace.add(Action::GetSurfaceTexture {
                        id: texture.to_trace(),
                        parent: self.to_trace(),
                    });
                }
            }
        }
        output
    }

    pub(crate) fn get_current_texture_inner(&self) -> Result<SurfaceOutput, SurfaceError> {
        profiling::scope!("Surface::get_current_texture");

        // Held until the acquired texture is recorded, so that the surface cannot be
        // reconfigured, nor another texture acquired, in between.
        let mut presentation = self.presentation.lock();
        let Some(present) = presentation.as_mut() else {
            return Err(SurfaceError::NotConfigured);
        };
        present.device.check_is_valid()?;
        // Checked before acquiring: a texture acquired from the HAL must be presented or
        // discarded, or the swapchain does not get it back.
        if present.acquired_texture.is_some() {
            return Err(SurfaceError::AlreadyAcquired);
        }
        let device = Arc::clone(&present.device);

        let suf = self.raw(device.backend()).unwrap();
        let ast = match unsafe {
            suf.acquire_texture(
                Some(core::time::Duration::from_millis(FRAME_TIMEOUT_MS as u64)),
                device.fence.as_ref(),
            )
        } {
            Ok(ast) => ast,
            Err(err) => {
                let status = match err {
                    hal::SurfaceError::Timeout => Status::Timeout,
                    hal::SurfaceError::Occluded => Status::Occluded,
                    hal::SurfaceError::Lost => Status::Lost,
                    hal::SurfaceError::Device(err) => {
                        return Err(device.handle_hal_error(err).into());
                    }
                    hal::SurfaceError::Outdated => Status::Outdated,
                    hal::SurfaceError::Other(msg) => {
                        log::error!("acquire error: {msg}");
                        Status::Lost
                    }
                };
                return Ok(SurfaceOutput {
                    status,
                    texture: None,
                });
            }
        };

        let config = &present.config;
        let texture_desc = wgt::TextureDescriptor {
            label: hal_label(
                Some(alloc::borrow::Cow::Borrowed("<Surface Texture>")),
                device.instance_flags,
            ),
            size: wgt::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            sample_count: 1,
            mip_level_count: 1,
            format: config.format,
            dimension: wgt::TextureDimension::D2,
            texture_binding_view_dimension: None,
            usage: config.usage,
            view_formats: config.view_formats.clone(),
        };
        let format_features = wgt::TextureFormatFeatures {
            allowed_usages: wgt::TextureUsages::RENDER_ATTACHMENT,
            flags: wgt::TextureFormatFeatureFlags::MULTISAMPLE_X4
                | wgt::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE,
        };
        let hal_usage =
            conv::map_texture_usage(config.usage, config.format.into(), format_features.flags);
        let clear_view_desc = hal::TextureViewDescriptor {
            label: hal_label(
                Some("(wgpu internal) clear surface texture view"),
                device.instance_flags,
            ),
            format: config.format,
            dimension: wgt::TextureViewDimension::D2,
            usage: wgt::TextureUses::COLOR_TARGET,
            range: wgt::ImageSubresourceRange::default(),
            swizzle: wgt::TextureComponentSwizzle::default(),
        };
        let clear_view = match unsafe {
            device
                .raw()
                .create_texture_view(ast.texture.as_ref().borrow(), &clear_view_desc)
        } {
            Ok(clear_view) => clear_view,
            Err(err) => {
                // Give the texture back, so that the swapchain can hand it out again.
                unsafe { suf.discard_texture(ast.texture) };
                return Err(device.handle_hal_error(err).into());
            }
        };

        let status = if ast.suboptimal {
            Status::Suboptimal
        } else {
            Status::Good
        };
        let texture = Arc::new(resource::Texture::new(
            &device,
            resource::TextureInner::Surface { raw: ast.texture },
            hal_usage,
            &texture_desc,
            format_features,
            resource::TextureClearMode::Surface {
                clear_view: ManuallyDrop::new(clear_view),
            },
            true,
            None,
        ));

        device
            .trackers
            .lock()
            .textures
            .insert_single(&texture, wgt::TextureUses::UNINITIALIZED);

        present.acquired_texture = Some(Arc::clone(&texture));
        present.textures.push(Arc::downgrade(&texture));

        Ok(SurfaceOutput {
            status,
            texture: Some(texture),
        })
    }

    pub fn present(self: &Arc<Self>) -> Result<Status, SurfaceError> {
        self.present_with_damage(&[])
    }

    pub fn present_with_damage(
        self: &Arc<Self>,
        damage_rects: &[wgt::Rect<u32>],
    ) -> Result<Status, SurfaceError> {
        // A lost device is often detected here: its callback is delivered when this returns.
        let _scope = CallbackScope::new(&self.pending_callbacks);
        #[cfg(feature = "trace")]
        if let Some(present) = self.presentation.lock().as_ref() {
            if let Some(ref mut trace) = *present.device.trace.lock() {
                trace.add(Action::Present(self.to_trace()));
            }
        }
        self.present_inner(damage_rects)
    }

    pub(crate) fn present_inner(
        &self,
        damage_rects: &[wgt::Rect<u32>],
    ) -> Result<Status, SurfaceError> {
        profiling::scope!("Surface::present");

        let device = match self.presentation.lock().as_ref() {
            Some(present) => Arc::clone(&present.device),
            None => return Err(SurfaceError::NotConfigured),
        };

        device.check_is_valid()?;
        let queue = device
            .get_queue()
            .ok_or(SurfaceError::Device(DeviceError::Lost))?;

        queue.present_with_damage(self, damage_rects)
    }
}

impl Surface {
    /// Gives `texture`, acquired from this surface on `device`, back to the HAL without
    /// presenting it. Returns `false` if the texture was destroyed.
    ///
    /// The caller must have taken `texture` out of the surface's locked [`Presentation`].
    pub(crate) fn discard_raw(&self, device: &Device, texture: &resource::Texture) -> bool {
        let Ok(state) = texture.state() else {
            return false;
        };
        let mut snatch_guard = device.snatchable_lock.write();
        let inner = state.inner.snatch(&mut snatch_guard);
        drop(snatch_guard);

        match inner {
            None => false,
            Some(resource::TextureInner::Surface { raw }) => {
                let raw_surface = self.raw(device.backend()).unwrap();
                unsafe { raw_surface.discard_texture(raw) };
                true
            }
            Some(resource::TextureInner::Native { .. }) => unreachable!(),
        }
    }
}

impl Queue {
    pub fn present(&self, surface: &Surface) -> Result<Status, SurfaceError> {
        self.present_with_damage(surface, &[])
    }

    pub fn present_with_damage(
        &self,
        surface: &Surface,
        damage_rects: &[wgt::Rect<u32>],
    ) -> Result<Status, SurfaceError> {
        profiling::scope!("Queue::present");

        let device = &self.device;

        // Held until the HAL has presented the texture, so that the surface cannot be
        // reconfigured, nor another texture acquired, in between.
        let mut presentation = surface.presentation.lock();
        let present = match presentation.as_mut() {
            Some(present) => present,
            None => return Err(SurfaceError::NotConfigured),
        };

        // Check the surface is configured for this device.
        if !Arc::ptr_eq(&present.device, device) {
            return Err(SurfaceError::Device(DeviceError::DeviceMismatch(Box::new(
                crate::device::DeviceMismatch {
                    res: self.error_ident(),
                    res_device: device.error_ident(),
                    target: None,
                    target_device: present.device.error_ident(),
                },
            ))));
        }

        let texture = present
            .acquired_texture
            .take()
            .ok_or(SurfaceError::NothingToPresent)?;

        // hal requires damage rects to be non-empty and within the surface.
        let surface_rect = wgt::Rect {
            x: 0,
            y: 0,
            w: present.config.width,
            h: present.config.height,
        };
        let damage_rects = damage_rects
            .iter()
            .filter_map(|rect| rect.intersect(&surface_rect))
            .collect::<Vec<_>>();

        // If the texture was never rendered to, clear it and transition to
        // PRESENT state before presenting.
        // Fixes <https://github.com/gfx-rs/wgpu/issues/6748>
        if let Err(error) = self.prepare_surface_texture_for_present(&texture) {
            // The texture is no longer acquired: give it back to the HAL, which would otherwise
            // never hand it out again.
            surface.discard_raw(device, &texture);
            return Err(error);
        }

        let inner = texture.state().ok().and_then(|state| {
            let mut snatch_guard = device.snatchable_lock.write();
            state.inner.snatch(&mut snatch_guard)
        });

        let result = match inner {
            None => return Err(SurfaceError::TextureDestroyed),
            Some(resource::TextureInner::Surface { raw }) => {
                let raw_surface = surface.raw(device.backend()).unwrap();
                let raw_queue = self.raw();
                // [`wgpu_hal::Queue::present`] requires the queue to be synchronized with submit calls and
                // other present calls. Locking command indices prevents submits which must increment the
                // submission index, and by `write`ing prevents other present calls.
                let _command_indices = device.command_indices.write();
                unsafe { raw_queue.present(raw_surface, raw, &damage_rects) }
            }
            Some(resource::TextureInner::Native { .. }) => unreachable!(),
        };

        match result {
            Ok(()) => Ok(Status::Good),
            Err(err) => match err {
                hal::SurfaceError::Timeout => Ok(Status::Timeout),
                hal::SurfaceError::Occluded => Ok(Status::Occluded),
                hal::SurfaceError::Lost => Ok(Status::Lost),
                hal::SurfaceError::Device(err) => {
                    Err(SurfaceError::from(device.handle_hal_error(err)))
                }
                hal::SurfaceError::Outdated => Ok(Status::Outdated),
                hal::SurfaceError::Other(msg) => {
                    log::error!("present error: {msg}");
                    Err(SurfaceError::Invalid)
                }
            },
        }
    }
}

impl Surface {
    pub fn discard(self: &Arc<Self>) -> Result<(), SurfaceError> {
        let _scope = CallbackScope::new(&self.pending_callbacks);
        #[cfg(feature = "trace")]
        if let Some(present) = self.presentation.lock().as_ref() {
            if let Some(ref mut trace) = *present.device.trace.lock() {
                trace.add(Action::DiscardSurfaceTexture(self.to_trace()));
            }
        }
        self.discard_inner()
    }

    pub(crate) fn discard_inner(&self) -> Result<(), SurfaceError> {
        profiling::scope!("Surface::discard");

        let mut presentation = self.presentation.lock();
        let present = match presentation.as_mut() {
            Some(present) => present,
            None => return Err(SurfaceError::NotConfigured),
        };

        let device = &present.device;

        device.check_is_valid()?;

        let texture = present
            .acquired_texture
            .take()
            .ok_or(SurfaceError::NothingToPresent)?;

        let mut exclusive_snatch_guard = device.snatchable_lock.write();
        let inner = texture
            .state()
            .ok()
            .and_then(|state| state.inner.snatch(&mut exclusive_snatch_guard));
        drop(exclusive_snatch_guard);

        match inner {
            None => return Err(SurfaceError::TextureDestroyed),
            Some(resource::TextureInner::Surface { raw }) => {
                let raw_surface = self.raw(device.backend()).unwrap();
                unsafe { raw_surface.discard_texture(raw) };
            }
            _ => unreachable!(),
        }

        Ok(())
    }

    pub fn release(self: &Arc<Self>) -> Result<(), SurfaceError> {
        #[cfg(feature = "trace")]
        if let Some(present) = self.presentation.lock().as_ref() {
            if let Some(ref mut trace) = *present.device.trace.lock() {
                trace.add(Action::ReleaseSurfaceTexture(self.to_trace()));
            }
        }
        self.release_inner()
    }

    /// Like `discard`, destroys the acquired texture, but skips the HAL
    /// `discard_texture` call. Safe to call during unwinding.
    pub(crate) fn release_inner(&self) -> Result<(), SurfaceError> {
        profiling::scope!("Surface::release");

        let mut presentation = self.presentation.lock();
        let Some(present) = presentation.as_mut() else {
            return Err(SurfaceError::NotConfigured);
        };

        let texture = present
            .acquired_texture
            .take()
            .ok_or(SurfaceError::NothingToPresent)?;
        // Destroy the texture, as presenting and discarding do: its raw texture is dropped here
        // even if the application still references the texture. On Vulkan, that drops the
        // texture's `Arc<SwapchainAcquireSemaphore>`, which the swapchain's teardown checks.
        //
        // This runs while the thread unwinds, which may hold the snatch lock itself, through an
        // `as_hal` guard, so it never waits for the lock. If the lock is held, configuring,
        // unconfiguring or dropping the surface drops the raw texture instead, in
        // `Texture::destroy_surface_texture_views`.
        if let Ok(state) = texture.state() {
            if let Some(mut snatch_guard) = present.device.snatchable_lock.try_write() {
                drop(state.inner.snatch(&mut snatch_guard));
            }
        }

        Ok(())
    }
}
