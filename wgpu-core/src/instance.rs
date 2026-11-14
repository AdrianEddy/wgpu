use alloc::{borrow::ToOwned as _, boxed::Box, string::String, sync::Arc, vec, vec::Vec};
use core::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

use hashbrown::HashMap;
use thiserror::Error;

use crate::{
    api_log, api_log_debug,
    device::{
        callbacks::{CallbackScope, PendingCallbacks},
        queue::Queue,
        resource::Device,
        DeviceDescriptor, DeviceError, UserClosures, WaitIdleError,
    },
    id::markers,
    limits::{self, check_limits, FailedLimit},
    lock::{rank, Mutex, MutexGuard},
    present::{ConfigureSurfaceError, Presentation},
    resource::ResourceType,
    resource_log,
    timestamp_normalization::TimestampNormalizerInitError,
    weak_vec::WeakVec,
    SubmissionIndex, DOWNLEVEL_WARNING_MESSAGE,
};

use wgt::{Backend, Backends, InstanceFlags, PowerPreference};

#[derive(Debug)]
pub(crate) struct InstanceDevices(Mutex<WeakVec<Device>>);

impl Default for InstanceDevices {
    fn default() -> Self {
        Self::new()
    }
}

impl InstanceDevices {
    pub(crate) fn new() -> Self {
        Self(Mutex::new(rank::INSTANCE_DEVICES, WeakVec::new()))
    }

    pub(crate) fn push(&self, device: &Arc<Device>) {
        self.0.lock().push(Arc::downgrade(device));
    }

    /// Poll all devices stored in this instance, and queue their callbacks on `callbacks`.
    ///
    /// If `force_wait` is true, block until all buffer mappings are done.
    ///
    /// Return `all_queue_empty` indicating whether there are more queue
    /// submissions still in flight.
    fn poll_all_devices(
        &self,
        force_wait: bool,
        callbacks: &PendingCallbacks,
    ) -> Result<bool, WaitIdleError> {
        // Poll the devices with the list unlocked: polling can lose a device, and this can hold
        // the last reference to one.
        let devices = self
            .0
            .lock()
            .iter()
            .filter_map(|device| device.upgrade())
            .collect::<Vec<_>>();

        let mut all_queue_empty = true;
        for device in devices {
            let poll_type = if force_wait {
                // TODO(#8286): Should expose timeout to poll_all.
                wgt::PollType::wait_indefinitely()
            } else {
                wgt::PollType::Poll
            };

            let (closures, result) = device.poll_and_return_closures(poll_type);

            let is_queue_empty = matches!(result, Ok(wgt::PollStatus::QueueEmpty));

            all_queue_empty &= is_queue_empty;

            callbacks.defer(closures);
        }

        Ok(all_queue_empty)
    }
}

#[derive(Default)]
pub struct Instance {
    _name: String,

    /// List of instances per `wgpu-hal` backend.
    ///
    /// The ordering in this list implies prioritization and needs to be preserved.
    instance_per_backend: Vec<(Backend, Box<dyn hal::DynInstance>)>,

    /// The backends that were requested by the user.
    requested_backends: Backends,

    /// The backends that we could have attempted to obtain from `wgpu-hal` —
    /// those for which support is compiled in, currently.
    ///
    /// The union of this and `requested_backends` is the set of backends that would be used,
    /// independent of whether accessing the drivers/hardware for them succeeds.
    /// To obtain the set of backends actually in use by this instance, check
    /// `instance_per_backend` instead.
    supported_backends: Backends,

    pub flags: InstanceFlags,

    /// Non-lifetimed [`raw_window_handle::DisplayHandle`], for keepalive and validation purposes in
    /// [`Self::create_surface()`].
    ///
    /// When used with `winit`, callers are expected to pass its `OwnedDisplayHandle` (created from
    /// the `EventLoop`) here.
    display: Option<Box<dyn wgt::WgpuHasDisplayHandle>>,

    /// Keeps track of all devices created from this instance, so that they can be polled.
    devices: InstanceDevices,

    /// The callbacks of this instance's devices, waiting to be delivered. Its surfaces share it.
    pub(crate) pending_callbacks: Arc<PendingCallbacks>,
}

impl Instance {
    pub fn new(
        name: &str,
        instance_desc: wgt::InstanceDescriptor,
        telemetry: Option<hal::Telemetry>,
    ) -> Arc<Self> {
        let mut this = InstanceBuilder::new(name, instance_desc, telemetry);

        #[cfg(all(vulkan, not(target_os = "netbsd")))]
        this.try_add_hal(hal::api::Vulkan);
        #[cfg(metal)]
        this.try_add_hal(hal::api::Metal);
        #[cfg(dx12)]
        this.try_add_hal(hal::api::Dx12);
        #[cfg(gles)]
        this.try_add_hal(hal::api::Gles);
        #[cfg(feature = "noop")]
        this.try_add_hal(hal::api::Noop);

        this.build()
    }

    pub fn from_hal_instance<A: hal::Api>(
        name: String,
        hal_instance: <A as hal::Api>::Instance,
    ) -> Arc<Self> {
        Arc::new(Self {
            _name: name,
            instance_per_backend: vec![(A::VARIANT, Box::new(hal_instance))],
            requested_backends: A::VARIANT.into(),
            supported_backends: A::VARIANT.into(),
            flags: InstanceFlags::default(),
            display: None, // TODO: Extract display from HAL instance if available?
            devices: InstanceDevices::new(),
            pending_callbacks: Arc::default(),
        })
    }

    pub fn raw(&self, backend: Backend) -> Option<&dyn hal::DynInstance> {
        self.instance_per_backend
            .iter()
            .find_map(|(instance_backend, instance)| {
                (*instance_backend == backend).then(|| instance.as_ref())
            })
    }

    /// # Safety
    ///
    /// - The raw instance handle returned must not be manually destroyed.
    pub unsafe fn as_hal<A: hal::Api>(&self) -> Option<&A::Instance> {
        self.raw(A::VARIANT).map(|instance| {
            instance
                .as_any()
                .downcast_ref()
                // This should be impossible. It would mean that backend instance and enum type are mismatching.
                .expect("Stored instance is not of the correct type")
        })
    }

    /// Creates a new surface targeting the given display/window handles.
    ///
    /// Internally attempts to create hal surfaces for all enabled backends.
    ///
    /// Fails only if creation for surfaces for all enabled backends fails in which case
    /// the error for each enabled backend is listed.
    /// Vice versa, if creation for any backend succeeds, success is returned.
    /// Surface creation errors are logged to the debug log in any case.
    ///
    /// # Safety
    ///
    /// - `display_handle` must be a valid object to create a surface upon,
    ///   falls back to the instance display handle otherwise.
    /// - `window_handle` must remain valid as long as the returned
    ///   [`Surface`] is being used.
    pub unsafe fn create_surface(
        &self,
        display_handle: Option<raw_window_handle::RawDisplayHandle>,
        window_handle: raw_window_handle::RawWindowHandle,
    ) -> Result<Arc<Surface>, CreateSurfaceError> {
        profiling::scope!("Instance::create_surface");

        let instance_display_handle = self.display.as_ref().map(|d| {
            d.display_handle()
                .expect("Implementation did not provide a DisplayHandle")
                .as_raw()
        });
        let display_handle = match (instance_display_handle, display_handle) {
            (Some(a), Some(b)) => {
                if a != b {
                    return Err(CreateSurfaceError::MismatchingDisplayHandle);
                }
                a
            }
            (Some(hnd), None) => hnd,
            (None, Some(hnd)) => hnd,
            (None, None) => return Err(CreateSurfaceError::MissingDisplayHandle),
        };

        let mut errors = HashMap::default();
        let mut surface_per_backend = HashMap::default();

        for (backend, instance) in &self.instance_per_backend {
            match unsafe {
                instance
                    .as_ref()
                    .create_surface(display_handle, window_handle)
            } {
                Ok(raw) => {
                    surface_per_backend.insert(*backend, raw);
                }
                Err(err) => {
                    log::debug!(
                        "Instance::create_surface: failed to create surface for {backend:?}: {err:?}"
                    );
                    errors.insert(*backend, err);
                }
            }
        }

        if surface_per_backend.is_empty() {
            Err(CreateSurfaceError::FailedToCreateSurfaceForAnyBackend(
                errors,
            ))
        } else {
            let surface = Arc::new(Surface {
                presentation: Mutex::new(rank::SURFACE_PRESENTATION, None),
                pending_callbacks: Arc::clone(&self.pending_callbacks),
                surface_per_backend,
            });

            Ok(surface)
        }
    }

    /// Creates a new surface from the given drm configuration.
    ///
    /// # Safety
    ///
    /// - All parameters must point to valid DRM values.
    ///
    /// # Platform Support
    ///
    /// This function requires the `"drm"` feature. It is only available on
    /// non-apple Unix-like platforms (Linux, FreeBSD) and currently only works
    /// with the Vulkan backend.
    #[cfg(drm)]
    #[cfg_attr(not(vulkan), expect(unused_variables, unused_mut))]
    pub unsafe fn create_surface_from_drm(
        &self,
        fd: i32,
        plane: u32,
        connector_id: u32,
        width: u32,
        height: u32,
        refresh_rate: u32,
    ) -> Result<Arc<Surface>, CreateSurfaceError> {
        profiling::scope!("Instance::create_surface_from_drm");

        let mut errors = HashMap::default();
        let mut surface_per_backend: HashMap<Backend, Box<dyn hal::DynSurface>> =
            HashMap::default();

        #[cfg(vulkan)]
        {
            let instance = unsafe { self.as_hal::<hal::api::Vulkan>() }
                .ok_or(CreateSurfaceError::BackendNotEnabled(Backend::Vulkan))?;

            // Safety must be upheld by the caller
            match unsafe {
                instance.create_surface_from_drm(
                    fd,
                    plane,
                    connector_id,
                    width,
                    height,
                    refresh_rate,
                )
            } {
                Ok(surface) => {
                    surface_per_backend.insert(Backend::Vulkan, Box::new(surface));
                }
                Err(err) => {
                    errors.insert(Backend::Vulkan, err);
                }
            }
        }

        if surface_per_backend.is_empty() {
            Err(CreateSurfaceError::FailedToCreateSurfaceForAnyBackend(
                errors,
            ))
        } else {
            let surface = Arc::new(Surface {
                presentation: Mutex::new(rank::SURFACE_PRESENTATION, None),
                pending_callbacks: Arc::clone(&self.pending_callbacks),
                surface_per_backend,
            });

            Ok(surface)
        }
    }

    /// # Safety
    ///
    /// `layer` must be a valid pointer.
    #[cfg(metal)]
    pub unsafe fn create_surface_metal(
        &self,
        layer: *mut core::ffi::c_void,
    ) -> Result<Arc<Surface>, CreateSurfaceError> {
        profiling::scope!("Instance::create_surface_metal");

        let instance = unsafe { self.as_hal::<hal::api::Metal>() }
            .ok_or(CreateSurfaceError::BackendNotEnabled(Backend::Metal))?;

        let layer = layer.cast();
        // SAFETY: We do this cast and deref. (rather than using `metal` to get the
        // object we want) to avoid direct coupling on the `metal` crate.
        //
        // To wit, this pointer…
        //
        // - …is properly aligned.
        // - …is dereferenceable to a `MetalLayerRef` as an invariant of the `metal`
        //   field.
        // - …points to an _initialized_ `MetalLayerRef`.
        // - …is only ever aliased via an immutable reference that lives within this
        //   lexical scope.
        let layer = unsafe { &*layer };
        let raw_surface: Box<dyn hal::DynSurface> =
            Box::new(instance.create_surface_from_layer(layer));

        let surface = Arc::new(Surface {
            presentation: Mutex::new(rank::SURFACE_PRESENTATION, None),
            pending_callbacks: Arc::clone(&self.pending_callbacks),
            surface_per_backend: core::iter::once((Backend::Metal, raw_surface)).collect(),
        });

        Ok(surface)
    }

    #[cfg(dx12)]
    fn create_surface_dx12(
        &self,
        create_surface_func: impl FnOnce(&hal::dx12::Instance) -> hal::dx12::Surface,
    ) -> Result<Arc<Surface>, CreateSurfaceError> {
        let instance = unsafe { self.as_hal::<hal::api::Dx12>() }
            .ok_or(CreateSurfaceError::BackendNotEnabled(Backend::Dx12))?;
        let surface: Box<dyn hal::DynSurface> = Box::new(create_surface_func(instance));

        let surface = Arc::new(Surface {
            presentation: Mutex::new(rank::SURFACE_PRESENTATION, None),
            pending_callbacks: Arc::clone(&self.pending_callbacks),
            surface_per_backend: core::iter::once((Backend::Dx12, surface)).collect(),
        });

        Ok(surface)
    }

    #[cfg(dx12)]
    /// # Safety
    ///
    /// The visual must be valid and able to be used to make a swapchain with.
    pub unsafe fn create_surface_from_visual(
        &self,
        visual: *mut core::ffi::c_void,
    ) -> Result<Arc<Surface>, CreateSurfaceError> {
        profiling::scope!("Instance::instance_create_surface_from_visual");
        self.create_surface_dx12(|inst| unsafe { inst.create_surface_from_visual(visual) })
    }

    #[cfg(dx12)]
    /// # Safety
    ///
    /// The surface_handle must be valid and able to be used to make a swapchain with.
    pub unsafe fn create_surface_from_surface_handle(
        &self,
        surface_handle: *mut core::ffi::c_void,
    ) -> Result<Arc<Surface>, CreateSurfaceError> {
        profiling::scope!("Instance::instance_create_surface_from_surface_handle");
        self.create_surface_dx12(|inst| unsafe {
            inst.create_surface_from_surface_handle(surface_handle)
        })
    }

    #[cfg(dx12)]
    /// # Safety
    ///
    /// The swap_chain_panel must be valid and able to be used to make a swapchain with.
    pub unsafe fn create_surface_from_swap_chain_panel(
        &self,
        swap_chain_panel: *mut core::ffi::c_void,
    ) -> Result<Arc<Surface>, CreateSurfaceError> {
        profiling::scope!("Instance::instance_create_surface_from_swap_chain_panel");
        self.create_surface_dx12(|inst| unsafe {
            inst.create_surface_from_swap_chain_panel(swap_chain_panel)
        })
    }

    fn adapter_allowed(&self, raw: &hal::DynExposedAdapter) -> bool {
        adapter_allowed(
            self.flags,
            &raw.info,
            &raw.capabilities.limits,
            raw.features,
        )
    }

    pub fn enumerate_adapters(
        self: &Arc<Self>,
        backends: Backends,
        apply_limit_buckets: bool,
    ) -> Vec<Arc<Adapter>> {
        profiling::scope!("Instance::enumerate_adapters");
        api_log!("Instance::enumerate_adapters");

        self.request_adapters(
            &wgt::RequestAdapterOptions {
                power_preference: PowerPreference::None,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets,
            },
            backends,
        )
        .unwrap_or_default()
    }

    /// Returns a list of adapters that match the given request options and are compatible with the specified backends.
    ///
    /// On success at least one compatible adapter will be returned in the resulting vector.
    /// Every adapter of `backends` is exposed to find them.
    pub fn request_adapters(
        self: &Arc<Self>,
        desc: &wgt::RequestAdapterOptions<&Surface>,
        backends: Backends,
    ) -> Result<Vec<Arc<Adapter>>, wgt::RequestAdapterError> {
        Ok(self
            .find_adapters(desc, backends, Wanted::All)?
            .into_iter()
            .map(|adapter| Adapter::new(adapter, self.clone()))
            .collect())
    }

    /// Returns the adapter of `backends` that best matches the given request options.
    ///
    /// A backend that can select its best adapter without exposing the others
    /// (`hal::Instance::request_adapter`; DX12 does, creating devices only for the adapters
    /// it tries up to its selection) offers only that one. It may follow a native preference
    /// order (DX12's is DXGI's, which a per-app GPU preference in Windows' graphics settings
    /// can supersede), so this is not always the first adapter of [`Self::request_adapters`],
    /// nor always of its device type. With other backends, the selection is ranked with their
    /// adapters by device type, Vulkan's first among equals, so a Vulkan adapter of the type
    /// that the power preference ranks first is returned, and a per-app GPU preference has no
    /// effect. A backend that cannot select, that declines, or whose selection does not suit
    /// `desc` has all its adapters exposed.
    ///
    /// The backends are asked in order, until the adapters found include one that no
    /// later backend's adapter can rank before: any adapter for `PowerPreference::None`
    /// and for `force_fallback_adapter`, else an integrated GPU for `LowPower` and a
    /// discrete one for `HighPerformance`. That adapter is the one that asking every
    /// backend would return. A backend that is asked still exposes its selection when
    /// another backend's adapter is returned: DX12 creates a device for it.
    pub fn request_adapter(
        self: &Arc<Self>,
        desc: &wgt::RequestAdapterOptions<&Surface>,
        backends: Backends,
    ) -> Result<Arc<Adapter>, wgt::RequestAdapterError> {
        profiling::scope!("Instance::request_adapter");
        api_log!("Instance::request_adapter");

        let adapter = self
            .find_adapters(desc, backends, Wanted::Best)?
            .into_iter()
            .next()
            .expect(
                "find_adapters should always return at least one compatible adapter on success",
            );
        let adapter = Adapter::new(adapter, self.clone());
        api_log_debug!("Request adapter result {:?}", adapter.get_info());
        Ok(adapter)
    }

    /// The adapters of `backends` that suit `desc`, ranked by `desc.power_preference`. On
    /// success there is at least one.
    ///
    /// With [`Wanted::Best`], each backend first offers its best adapter alone
    /// (`hal::Instance::request_adapter`). If it offers none, or that adapter does not
    /// suit `desc`, the backend's adapters are all exposed, as with [`Wanted::All`], so
    /// that the selection and the reasons of a [`wgt::RequestAdapterError::NotFound`] are
    /// those of a full enumeration. The backends after one whose adapters, with those of
    /// the backends before it, include an adapter that no later adapter can outrank
    /// ([`ranks_first_whatever_follows`]) are not asked: the first adapter is the same as
    /// with every backend asked, and the error cannot happen.
    fn find_adapters(
        &self,
        desc: &wgt::RequestAdapterOptions<&Surface>,
        backends: Backends,
        wanted: Wanted,
    ) -> Result<Vec<hal::DynExposedAdapter>, wgt::RequestAdapterError> {
        let mut adapters = Vec::new();
        let mut unsuitable = UnsuitableBackends::none();

        for &(backend, ref instance) in self
            .instance_per_backend
            .iter()
            .filter(|&&(backend, _)| backends.contains(Backends::from(backend)))
        {
            if wanted == Wanted::Best && ranks_first_whatever_follows(&adapters, desc) {
                log::debug!(
                    "Backend `{backend:?}` and the backends after it are not asked: \
                    none of their adapters can rank before an adapter already found"
                );
                break;
            }

            profiling::scope!("enumerating", &*alloc::format!("{backend:?}"));

            let compatible_hal_surface = desc
                .compatible_surface
                .and_then(|surface| surface.raw(backend));

            if wanted == Wanted::Best {
                let best = unsafe {
                    instance.request_adapter(
                        desc.power_preference,
                        desc.force_fallback_adapter,
                        compatible_hal_surface,
                    )
                };
                if let Some(best) = best {
                    // Why one adapter does not suit `desc` says nothing about the backend's
                    // other adapters: the enumeration below records the backend's reasons.
                    let suitable = self.suitable_adapters(
                        backend,
                        vec![best],
                        desc,
                        &mut UnsuitableBackends::none(),
                    );
                    if let Some(best) = suitable.first() {
                        log::debug!(
                            "Backend `{backend:?}` selected adapter without full enumeration: {:?}",
                            best.info
                        );
                        adapters.extend(suitable);
                        continue;
                    }
                }
            }

            let backend_adapters = unsafe { instance.enumerate_adapters(compatible_hal_surface) };
            adapters.extend(self.suitable_adapters(
                backend,
                backend_adapters,
                desc,
                &mut unsuitable,
            ));
        }

        rank_adapters(&mut adapters, desc.power_preference);

        // `request_adapter` can be a bit of a black box.
        // Shine some light on its decision in debug log.
        if adapters.is_empty() {
            log::debug!("Request adapter didn't find compatible adapters.");
            Err(wgt::RequestAdapterError::NotFound {
                supported_backends: self.supported_backends,
                requested_backends: self.requested_backends,
                active_backends: self.active_backends(),
                no_fallback_backends: unsuitable.no_fallback,
                no_adapter_backends: unsuitable.no_adapter,
                incompatible_surface_backends: unsuitable.incompatible_surface,
            })
        } else {
            log::debug!(
                "Found {} compatible adapters. Sorted by preference:",
                adapters.len()
            );
            for adapter in &adapters {
                log::debug!("* {:?}", adapter.info);
            }
            Ok(adapters)
        }
    }

    /// `backend_adapters`, the adapters that `backend` exposed, less those that do not
    /// suit `desc`, with their features and limits filtered as this instance requires.
    /// Records in `unsuitable` that `backend` exposed no adapter, or none for
    /// `force_fallback_adapter`, and that `desc.compatible_surface` rejected one of its
    /// adapters (even if others remain). Adapters that `adapter_allowed` or the limit
    /// buckets drop record nothing.
    fn suitable_adapters(
        &self,
        backend: Backend,
        mut backend_adapters: Vec<hal::DynExposedAdapter>,
        desc: &wgt::RequestAdapterOptions<&Surface>,
        unsuitable: &mut UnsuitableBackends,
    ) -> Vec<hal::DynExposedAdapter> {
        if backend_adapters.is_empty() {
            log::debug!("enabled backend `{backend:?}` has no adapters");
            unsuitable.no_adapter |= Backends::from(backend);
            // by returning, we avoid setting the further error bits below
            return Vec::new();
        }

        if desc.force_fallback_adapter {
            log::debug!("Filtering `{backend:?}` for `force_fallback_adapter`");
            backend_adapters.retain(|exposed| {
                let keep = exposed.info.device_type == wgt::DeviceType::Cpu;
                if !keep {
                    log::debug!("* Eliminating adapter `{}`", exposed.info.name);
                }
                keep
            });
            if backend_adapters.is_empty() {
                log::debug!("* Backend `{backend:?}` has no fallback adapters");
                unsuitable.no_fallback |= Backends::from(backend);
                return Vec::new();
            }
        }

        if let Some(surface) = desc.compatible_surface {
            backend_adapters.retain(|exposed| {
                let capabilities = surface.get_capabilities_with_raw(exposed);
                if let Err(err) = capabilities {
                    log::debug!(
                        "Adapter {:?} not compatible with surface: {}",
                        exposed.info,
                        err
                    );
                    unsuitable.incompatible_surface |= Backends::from(backend);
                    false
                } else {
                    true
                }
            });
            if backend_adapters.is_empty() {
                unsuitable.incompatible_surface |= Backends::from(backend);
                return Vec::new();
            }
        }

        let backend_adapters = backend_adapters
            .into_iter()
            .map(|mut raw| {
                self.adjust_limits_for_indirect_validation(&mut raw.capabilities.limits);
                raw
            })
            .map(|mut raw| {
                filter_features_and_limits(
                    self.flags,
                    &mut raw.features,
                    &mut raw.capabilities.limits,
                );
                raw
            })
            .filter(|raw| self.adapter_allowed(raw));

        if desc.apply_limit_buckets {
            backend_adapters
                .filter_map(limits::apply_limit_buckets)
                .collect()
        } else {
            backend_adapters.collect()
        }
    }

    /// This is similar to wgpu-hal's `adjust_raw_limits` but tailored to
    /// wgpu-core's constraints.
    fn adjust_limits_for_indirect_validation(&self, limits: &mut wgt::Limits) {
        // Indirect draw validation can't support u64 offsets,
        // lower max buffer and binding size to fit in an u32.
        if self.flags.contains(InstanceFlags::VALIDATION_INDIRECT_CALL) {
            limits.max_buffer_size = limits.max_buffer_size.min(u32::MAX as u64);
            limits.max_uniform_buffer_binding_size =
                limits.max_uniform_buffer_binding_size.min(u32::MAX as u64);
            limits.max_storage_buffer_binding_size = limits
                .max_storage_buffer_binding_size
                .min(u32::MAX as u64 & !(wgt::STORAGE_BINDING_SIZE_ALIGNMENT as u64 - 1));
        }
    }

    fn active_backends(&self) -> Backends {
        self.instance_per_backend
            .iter()
            .map(|&(backend, _)| Backends::from(backend))
            .collect()
    }

    /// Create an adapter from a HAL adapter.
    ///
    /// The HAL adapter may be obtained e.g. by calling `enumerate_adapters` on
    /// the HAL directly.
    ///
    /// If [limit bucketing][lt] is desired, [`crate::limits::apply_limit_buckets`]
    /// should be called with the HAL adapter before calling this function.
    ///
    /// # Safety
    ///
    /// `hal_adapter` must be created from this global internal instance handle.
    ///
    /// [lt]: crate::limits#Limit-bucketing
    pub unsafe fn create_adapter_from_hal(
        self: &Arc<Self>,
        hal_adapter: hal::DynExposedAdapter,
    ) -> Arc<Adapter> {
        profiling::scope!("Instance::create_adapter_from_hal");

        let adapter = Adapter::new(hal_adapter, self.clone());

        resource_log!("Created Adapter {:?}", Arc::as_ptr(&adapter));
        adapter
    }

    /// Poll all devices on all backends.
    ///
    /// This is the implementation of `wgpu::Instance::poll_all`.
    ///
    /// Return `all_queue_empty` indicating whether there are more queue
    /// submissions still in flight.
    pub fn poll_all_devices(&self, force_wait: bool) -> Result<bool, WaitIdleError> {
        api_log!("poll_all_devices");
        let _scope = CallbackScope::new(&self.pending_callbacks);
        self.devices
            .poll_all_devices(force_wait, &self.pending_callbacks)
    }
}

pub struct InstanceBuilder {
    instance: Instance,
    instance_desc: wgt::InstanceDescriptor,
    telemetry: Option<hal::Telemetry>,
}

impl InstanceBuilder {
    pub fn new(
        name: &str,
        mut desc: wgt::InstanceDescriptor,
        telemetry: Option<hal::Telemetry>,
    ) -> Self {
        Self {
            instance: Instance {
                _name: name.to_owned(),
                instance_per_backend: Vec::new(),
                requested_backends: desc.backends,
                supported_backends: Backends::empty(),
                flags: desc.flags,
                // HACK: We must take ownership of the field here, without being able to pass it into
                // try_add_hal(). Remove it from the mutable descriptor instead, while try_add_hal()
                // borrows the handle from `this.display` instead.
                display: desc.display.take(),
                devices: InstanceDevices::new(),
                pending_callbacks: Arc::default(),
            },
            instance_desc: desc,
            telemetry,
        }
    }

    pub fn try_add_hal<A: hal::Api>(&mut self, api: A) {
        use hal::Instance as _;
        self.try_add_custom_hal(api, |hal_desc| unsafe { A::Instance::init(hal_desc) });
    }

    pub fn try_add_custom_hal<A: hal::Api>(
        &mut self,
        _: A,
        f: impl FnOnce(&hal::InstanceDescriptor) -> Result<A::Instance, hal::InstanceError>,
    ) {
        if self.instance.supported_backends.contains(A::VARIANT.into()) {
            return;
        }

        // Whether or not the backend was requested, and whether or not it succeeds,
        // note that we *could* try it.
        self.instance.supported_backends |= A::VARIANT.into();

        if !self.instance_desc.backends.contains(A::VARIANT.into()) {
            log::trace!("Instance::new: backend {:?} not requested", A::VARIANT);
            return;
        }

        // If this was Some, it was moved into self
        assert!(self.instance_desc.display.is_none());

        let hal_desc = hal::InstanceDescriptor {
            name: "wgpu",
            flags: self.instance.flags,
            memory_budget_thresholds: self.instance_desc.memory_budget_thresholds,
            backend_options: self.instance_desc.backend_options.clone(),
            telemetry: self.telemetry,
            // Pass a borrow, the core instance here keeps the owned handle alive already
            // WARNING: Using self here, not instance_desc!
            display: self.instance.display.as_ref().map(|hdh| {
                hdh.display_handle()
                    .expect("Implementation did not provide a DisplayHandle")
            }),
        };

        match f(&hal_desc) {
            Ok(instance) => {
                log::debug!("Instance::new: created {:?} backend", A::VARIANT);
                self.instance
                    .instance_per_backend
                    .push((A::VARIANT, Box::new(instance)));
            }
            Err(err) => {
                log::debug!(
                    "Instance::new: failed to create {:?} backend: {:?}",
                    A::VARIANT,
                    err
                );
            }
        }
    }

    pub fn build(self) -> Arc<Instance> {
        Arc::new(self.instance)
    }
}

pub struct Surface {
    pub(crate) presentation: Mutex<Option<Presentation>>,
    pub surface_per_backend: HashMap<Backend, Box<dyn hal::DynSurface>>,
    /// The pending callbacks of the instance that created the surface, so that it can open a
    /// [`CallbackScope`] before it locks `presentation`.
    pub(crate) pending_callbacks: Arc<PendingCallbacks>,
}

impl ResourceType for Surface {
    const TYPE: &'static str = "Surface";
}
impl crate::storage::StorageItem for Surface {
    type Marker = markers::Surface;
}

impl Surface {
    pub fn get_capabilities(
        &self,
        adapter: &Adapter,
    ) -> Result<wgt::SurfaceCapabilities, GetSurfaceSupportError> {
        profiling::scope!("Surface::get_capabilities");
        let mut hal_caps = self.get_hal_capabilities(adapter)?;

        hal_caps
            .formats
            .sort_by_key(|fc| !fc.format.has_srgb_suffix());

        let usages = crate::conv::map_texture_usage_from_hal(hal_caps.usage);

        // `SurfaceCapabilities::formats` lists only the formats a
        // color-space-unaware application can configure via
        // `SurfaceColorSpace::Auto`, i.e. those for which `Auto` resolves to a
        // concrete color space. (The full `format_capabilities` still reports
        // every color space, including HDR ones, for explicit opt-in.)
        Ok(wgt::SurfaceCapabilities {
            formats: hal_caps
                .formats
                .iter()
                .filter(|fc| {
                    crate::device::surface_config::resolve_auto_color_space(
                        fc.format,
                        fc.color_spaces,
                    )
                    .is_some()
                })
                .map(|fc| fc.format)
                .collect(),
            format_capabilities: hal_caps.formats,
            present_modes: hal_caps.present_modes,
            alpha_modes: hal_caps.composite_alpha_modes,
            usages,
        })
    }

    pub fn get_hal_capabilities(
        &self,
        adapter: &Adapter,
    ) -> Result<hal::SurfaceCapabilities, GetSurfaceSupportError> {
        self.get_capabilities_with_raw(&adapter.raw)
    }

    pub fn get_capabilities_with_raw(
        &self,
        adapter: &hal::DynExposedAdapter,
    ) -> Result<hal::SurfaceCapabilities, GetSurfaceSupportError> {
        let backend = adapter.backend();
        let suf = self
            .raw(backend)
            .ok_or(GetSurfaceSupportError::NotSupportedByBackend(backend))?;
        profiling::scope!("surface_capabilities");
        let caps = unsafe { adapter.adapter.surface_capabilities(suf) }
            .ok_or(GetSurfaceSupportError::FailedToRetrieveSurfaceCapabilitiesForAdapter)?;
        Ok(caps)
    }

    /// Returns the HDR / luminance characteristics of the display backing this
    /// surface on `adapter`.
    ///
    /// Falls back to [`wgt::DisplayHdrInfo::default`] (all fields `None`) when the
    /// surface is not on `adapter`'s backend or the backend reports nothing.
    pub fn display_hdr_info(&self, adapter: &Adapter) -> wgt::DisplayHdrInfo {
        profiling::scope!("Surface::display_hdr_info");
        self.display_hdr_info_with_raw(&adapter.raw)
    }

    pub fn display_hdr_info_with_raw(
        &self,
        adapter: &hal::DynExposedAdapter,
    ) -> wgt::DisplayHdrInfo {
        let backend = adapter.backend();
        let Some(suf) = self.raw(backend) else {
            return wgt::DisplayHdrInfo::default();
        };
        profiling::scope!("surface_display_hdr_info");
        unsafe { adapter.adapter.surface_display_hdr_info(suf) }.unwrap_or_default()
    }

    pub fn raw(&self, backend: Backend) -> Option<&dyn hal::DynSurface> {
        self.surface_per_backend
            .get(&backend)
            .map(|surface| surface.as_ref())
    }

    /// Discards the acquired texture of the current configuration, destroys its other textures,
    /// waits for all work on its device to finish, then takes the configuration and destroys the
    /// views of its textures.
    ///
    /// The raw surface is still configured, so the caller must unconfigure it next, with the
    /// returned configuration's device.
    ///
    /// If the wait times out, the GPU may still use the surface's textures: the configuration is
    /// left in place, without its acquired texture, and what the wait gave up on is returned. A
    /// backend can time out even without a timeout: GL waits for at most `i32::MAX` nanoseconds.
    fn take_presentation<'a>(
        &self,
        presentation: &mut MutexGuard<'a, Option<Presentation>>,
    ) -> (Result<Option<Presentation>, WaitGaveUp>, UserClosures) {
        let Some(present) = presentation.as_mut() else {
            return (Ok(None), UserClosures::default());
        };
        // Given back to the HAL, so that the HAL surface has no texture acquired, whether the
        // configuration is taken or stays. Destroyed before the wait, they can no longer be used
        // by work submitted during the wait, whose views would be destroyed under it. The other
        // textures of the configuration were presented or discarded, which destroyed them.
        let acquired = [
            present.acquired_texture.take(),
            present.released_texture.take(),
        ];
        for texture in acquired.into_iter().flatten() {
            self.discard_raw(&present.device, &texture);
        }

        // Wait for all work that uses the surface's textures to finish. The wait is for this
        // submission or a later one.
        let waited_for = present
            .device
            .last_successful_submission_index
            .load(Ordering::Acquire);
        let snatch_guard = present.device.snatchable_lock.read();
        let (user_callbacks, result) = present
            .device
            .maintain(wgt::PollType::wait_indefinitely(), snatch_guard);
        let waited = match result {
            Ok(_) => Ok(()),
            // A lost device runs no more work, so cleaning up can go on.
            Err(WaitIdleError::Device(_)) => Ok(()),
            // On wasm, you cannot actually successfully wait for the surface. However WebGL does
            // not actually require you do this, so ignoring the failure is totally fine. See
            // https://github.com/gfx-rs/wgpu/issues/7363
            Err(WaitIdleError::Timeout) if cfg!(target_family = "wasm") => Ok(()),
            Err(WaitIdleError::Timeout) => Err(WaitGaveUp { waited_for }),
            Err(WaitIdleError::WrongSubmissionIndex(_, _)) => {
                unreachable!("no submission index was provided")
            }
        };
        let taken = waited.map(|()| {
            presentation.take().map(|mut present| {
                // The HAL surface is unconfigured next, which requires the views of its textures
                // destroyed, including those of textures that the application still references.
                present.destroy_texture_views();
                present
            })
        });
        (taken, user_callbacks)
    }

    /// Unconfigures the raw surface from `device`, once the device's submissions completed (see
    /// [`Self::teardown_waited`]). `gave_up_at` is the submission that [`Self::take_presentation`]'s
    /// wait gave up on, if it did.
    ///
    /// # Safety
    ///
    /// `device` must have been returned by [`Self::take_presentation`], and the
    /// surface must not have been configured since.
    unsafe fn unconfigure_raw(&self, device: &Device, gave_up_at: Option<SubmissionIndex>) {
        if let Some(surface) = self.raw(device.backend()) {
            let (_command_indices, waited) = device.lock_after_submissions(gave_up_at);
            let _ = Self::teardown_waited(device, waited);
            unsafe { surface.unconfigure(device.raw()) };
        }
    }

    /// The outcome of the wait for `device`'s submissions before the HAL tears a configuration
    /// down. A wait that gave up (GL, WebGL) is logged. A failed wait loses the device. The
    /// teardown goes on either way: the HAL must release the configuration.
    fn teardown_waited(
        device: &Device,
        waited: Result<bool, hal::DeviceError>,
    ) -> Result<(), DeviceError> {
        match waited {
            Ok(true) => Ok(()),
            Ok(false) => {
                log::warn!(
                    "Gave up waiting for the GPU before tearing down a surface configuration"
                );
                Ok(())
            }
            Err(error) => {
                log::warn!(
                    "Failed to wait for the GPU before tearing down a surface configuration: {error}"
                );
                Err(device.handle_hal_error(error))
            }
        }
    }

    /// Unconfigures the surface. Returns the device it was configured for, which the caller must
    /// release once the surface is unlocked: it may be the last reference to the device.
    fn unconfigure_inner<'a>(
        &self,
        presentation: &mut MutexGuard<'a, Option<Presentation>>,
    ) -> (Option<Arc<Device>>, UserClosures) {
        let (taken, user_callbacks) = self.take_presentation(presentation);
        let mut gave_up_at = None;
        let present = taken.unwrap_or_else(|gave_up| {
            // Unconfiguring cannot fail, and dropping a surface must release it.
            log::error!(
                "Failed to wait for the GPU before unconfiguring the surface: {}",
                WaitIdleError::Timeout
            );
            gave_up_at = Some(gave_up.waited_for);
            presentation.take().map(|mut present| {
                present.destroy_texture_views();
                present
            })
        });
        let device = present.map(|present| present.device);
        if let Some(device) = &device {
            unsafe { self.unconfigure_raw(device, gave_up_at) };
        }
        (device, user_callbacks)
    }

    pub fn unconfigure(self: &Arc<Self>) {
        profiling::scope!("Surface::unconfigure");
        let _scope = CallbackScope::new(&self.pending_callbacks);
        let (device, user_callbacks) = self.unconfigure_inner(&mut self.presentation.lock());
        drop(device);
        self.pending_callbacks.defer(user_callbacks);
    }

    pub fn configure(
        self: &Arc<Self>,
        device: &Arc<Device>,
        config: &wgt::SurfaceConfiguration<Vec<wgt::TextureFormat>>,
    ) -> Result<(), ConfigureSurfaceError> {
        use ConfigureSurfaceError as E;
        profiling::scope!("Surface::configure");
        let _scope = CallbackScope::new(&self.pending_callbacks);

        #[cfg(feature = "trace")]
        if let Some(ref mut trace) = *device.trace.lock() {
            use crate::device::trace::{Action, IntoTrace};

            trace.add(Action::ConfigureSurface(self.to_trace(), config.clone()));
        }

        log::debug!("configuring surface with {config:?}");

        let caps = self
            .get_hal_capabilities(&device.adapter)
            .map_err(|_| E::UnsupportedQueueFamily)?;

        let mut hal_view_formats = Vec::new();
        for format in config.view_formats.iter() {
            if *format == config.format {
                continue;
            }
            if !caps.formats.iter().any(|fc| fc.format == config.format) {
                return Err(E::UnsupportedFormat {
                    requested: config.format,
                    available: caps.texture_formats().collect(),
                });
            }
            if config.format.remove_srgb_suffix() != format.remove_srgb_suffix() {
                return Err(E::InvalidViewFormat(*format, config.format));
            }
            hal_view_formats.push(*format);
        }

        if !hal_view_formats.is_empty() {
            device.require_downlevel_flags(wgt::DownlevelFlags::SURFACE_VIEW_FORMATS)?;
        }

        let maximum_frame_latency = config.desired_maximum_frame_latency.clamp(
            *caps.maximum_frame_latency.start(),
            *caps.maximum_frame_latency.end(),
        );
        let mut hal_config = hal::SurfaceConfiguration {
            maximum_frame_latency,
            present_mode: config.present_mode,
            composite_alpha_mode: config.alpha_mode,
            format: config.format,
            color_space: config.color_space,
            extent: wgt::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            usage: crate::conv::map_texture_usage(
                config.usage,
                hal::FormatAspects::COLOR,
                wgt::TextureFormatFeatureFlags::STORAGE_READ_ONLY
                    | wgt::TextureFormatFeatureFlags::STORAGE_WRITE_ONLY
                    | wgt::TextureFormatFeatureFlags::STORAGE_READ_WRITE,
            ),
            view_formats: hal_view_formats,
        };

        crate::device::surface_config::validate_surface_configuration(
            &mut hal_config,
            &caps,
            device.limits.max_texture_dimension_2d,
        )?;

        device.check_is_valid()?;

        let user_callbacks;
        // Released once the surface is unlocked: it may be the last reference to the device.
        let previous_device;
        let result = 'configure: {
            // we keep presentation locked for the entire duration of the configure call,
            // so that no change can happen in the middle of it.
            let mut presentation = self.presentation.lock();

            let taken;
            (taken, user_callbacks) = self.take_presentation(&mut presentation);
            let previous = match taken {
                Ok(previous) => previous.map(|previous| previous.device),
                // The surface stays configured, without the texture it had acquired.
                Err(WaitGaveUp { .. }) => {
                    previous_device = None;
                    break 'configure Err(E::GpuWaitTimeout);
                }
            };
            previous_device = previous;

            let surface_raw = self.raw(device.backend()).unwrap();
            // `command_indices` serializes these calls with other queue
            // operations. For the same device, hold it across both calls.
            let result = {
                let same_device = match &previous_device {
                    Some(previous_device) if Arc::ptr_eq(previous_device, device) => true,
                    Some(previous_device) => {
                        unsafe { self.unconfigure_raw(previous_device, None) };
                        false
                    }
                    None => false,
                };
                let _command_indices = if same_device {
                    let (command_indices, waited) = device.lock_after_submissions(None);
                    let waited = Self::teardown_waited(device, waited);
                    unsafe { surface_raw.unconfigure(device.raw()) };
                    if let Err(error) = waited {
                        break 'configure Err(E::Device(error));
                    }
                    command_indices
                } else {
                    device.command_indices.write()
                };
                unsafe { surface_raw.configure(device.raw(), &hal_config) }
            };
            match result {
                Ok(()) => {
                    *presentation = Some(Presentation::new(Arc::clone(device), config.clone()));
                    Ok(())
                }
                Err(error) => Err(match error {
                    hal::SurfaceError::Outdated
                    | hal::SurfaceError::Lost
                    | hal::SurfaceError::Occluded
                    | hal::SurfaceError::Timeout => E::InvalidSurface,
                    hal::SurfaceError::Device(error) => E::Device(device.handle_hal_error(error)),
                    hal::SurfaceError::Other(message) => {
                        log::error!("surface configuration failed: {message}");
                        E::InvalidSurface
                    }
                }),
            }
        };
        drop(previous_device);
        self.pending_callbacks.defer(user_callbacks);

        result
    }
}

impl Drop for Surface {
    #[allow(trivial_casts)]
    fn drop(&mut self) {
        profiling::scope!("Surface::drop");

        api_log!("Surface::drop {:?}", self as *const _);
        let _scope = CallbackScope::new(&self.pending_callbacks);
        let (device, user_closures) = self.unconfigure_inner(&mut self.presentation.lock());
        drop(device);
        self.pending_callbacks.defer(user_closures);
    }
}

/// [`Surface::take_presentation`]'s wait for the GPU gave up, as GL's waits do after about two
/// seconds: the device's submissions up to `waited_for`, at least, may be incomplete.
struct WaitGaveUp {
    waited_for: SubmissionIndex,
}

pub struct Adapter {
    pub(crate) raw: hal::DynExposedAdapter,
    pub(crate) instance: Arc<Instance>,
    consumed: AtomicBool,
}

impl Adapter {
    pub(crate) fn new(raw: hal::DynExposedAdapter, instance: Arc<Instance>) -> Arc<Self> {
        Arc::new(Self {
            raw,
            instance,
            consumed: AtomicBool::new(false),
        })
    }

    /// Returns the backend this adapter is using.
    pub fn backend(&self) -> Backend {
        self.raw.backend()
    }

    pub fn is_surface_supported(&self, surface: &Surface) -> bool {
        // If get_capabilities returns Err, then the API does not advertise support for the surface.
        //
        // This could occur if the user is running their app on Wayland but Vulkan does not support
        // VK_KHR_wayland_surface.
        surface.get_hal_capabilities(self).is_ok()
    }

    pub fn get_info(&self) -> wgt::AdapterInfo {
        self.raw.info.clone()
    }

    pub fn features(&self) -> wgt::Features {
        self.raw.features
    }

    pub fn limits(&self) -> wgt::Limits {
        self.raw.capabilities.limits.clone()
    }

    pub fn downlevel_capabilities(&self) -> wgt::DownlevelCapabilities {
        self.raw.capabilities.downlevel.clone()
    }

    pub fn get_presentation_timestamp(&self) -> wgt::PresentationTimestamp {
        unsafe { self.raw.adapter.get_presentation_timestamp() }
    }

    pub fn cooperative_matrix_properties(&self) -> Vec<wgt::CooperativeMatrixProperties> {
        self.raw.capabilities.cooperative_matrix_properties.clone()
    }

    pub fn ray_tracing_invocation_reorder_mode(&self) -> wgt::RayTracingInvocationReorderMode {
        self.raw.capabilities.ray_tracing_invocation_reorder_mode
    }

    pub fn get_texture_format_features(
        &self,
        format: wgt::TextureFormat,
    ) -> wgt::TextureFormatFeatures {
        use hal::TextureFormatCapabilities as Tfc;

        let mut caps = unsafe { self.raw.adapter.texture_format_capabilities(format) };

        // Storage bindings of multi-planar textures are not supported.
        if format.is_multi_planar_format() {
            caps -= Tfc::STORAGE_READ_ONLY
                | Tfc::STORAGE_WRITE_ONLY
                | Tfc::STORAGE_READ_WRITE
                | Tfc::STORAGE_ATOMIC;
        }

        let mut allowed_usages = wgt::TextureUsages::empty();

        allowed_usages.set(wgt::TextureUsages::COPY_SRC, caps.contains(Tfc::COPY_SRC));
        allowed_usages.set(wgt::TextureUsages::COPY_DST, caps.contains(Tfc::COPY_DST));
        allowed_usages.set(
            wgt::TextureUsages::TEXTURE_BINDING,
            caps.contains(Tfc::SAMPLED),
        );
        allowed_usages.set(
            wgt::TextureUsages::STORAGE_BINDING,
            caps.intersects(
                Tfc::STORAGE_WRITE_ONLY
                    | Tfc::STORAGE_READ_ONLY
                    | Tfc::STORAGE_READ_WRITE
                    | Tfc::STORAGE_ATOMIC,
            ),
        );
        allowed_usages.set(
            wgt::TextureUsages::RENDER_ATTACHMENT | wgt::TextureUsages::TRANSIENT_ATTACHMENT,
            caps.intersects(Tfc::COLOR_ATTACHMENT | Tfc::DEPTH_STENCIL_ATTACHMENT),
        );
        allowed_usages.set(
            wgt::TextureUsages::STORAGE_ATOMIC,
            caps.contains(Tfc::STORAGE_ATOMIC),
        );

        let mut flags = wgt::TextureFormatFeatureFlags::empty();
        flags.set(
            wgt::TextureFormatFeatureFlags::STORAGE_READ_ONLY,
            caps.contains(Tfc::STORAGE_READ_ONLY),
        );
        flags.set(
            wgt::TextureFormatFeatureFlags::STORAGE_WRITE_ONLY,
            caps.contains(Tfc::STORAGE_WRITE_ONLY),
        );
        flags.set(
            wgt::TextureFormatFeatureFlags::STORAGE_READ_WRITE,
            caps.contains(Tfc::STORAGE_READ_WRITE),
        );

        flags.set(
            wgt::TextureFormatFeatureFlags::STORAGE_ATOMIC,
            caps.contains(Tfc::STORAGE_ATOMIC),
        );

        flags.set(
            wgt::TextureFormatFeatureFlags::FILTERABLE,
            caps.contains(Tfc::SAMPLED_LINEAR),
        );

        flags.set(
            wgt::TextureFormatFeatureFlags::BLENDABLE,
            caps.contains(Tfc::COLOR_ATTACHMENT_BLEND),
        );

        flags.set(
            wgt::TextureFormatFeatureFlags::MULTISAMPLE_X2,
            caps.contains(Tfc::MULTISAMPLE_X2),
        );
        flags.set(
            wgt::TextureFormatFeatureFlags::MULTISAMPLE_X4,
            caps.contains(Tfc::MULTISAMPLE_X4),
        );
        flags.set(
            wgt::TextureFormatFeatureFlags::MULTISAMPLE_X8,
            caps.contains(Tfc::MULTISAMPLE_X8),
        );
        flags.set(
            wgt::TextureFormatFeatureFlags::MULTISAMPLE_X16,
            caps.contains(Tfc::MULTISAMPLE_X16),
        );

        flags.set(
            wgt::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE,
            caps.contains(Tfc::MULTISAMPLE_RESOLVE),
        );

        wgt::TextureFormatFeatures {
            allowed_usages,
            flags,
        }
    }

    /// # Safety
    ///
    /// - `hal_device` must be created from this adapter.
    /// - `desc` must be a subset of `hal_device` features and limits.
    pub unsafe fn create_device_and_queue_from_hal(
        self: &Arc<Self>,
        hal_device: hal::DynOpenDevice,
        desc: &DeviceDescriptor,
    ) -> Result<(Arc<Device>, Arc<Queue>), RequestDeviceError> {
        profiling::scope!("Adapter::create_device_and_queue_from_hal");
        api_log!("Adapter::create_device_and_queue_from_hal");

        if self
            .instance
            .flags
            .contains(InstanceFlags::STRICT_WEBGPU_COMPLIANCE)
            && self.consumed.load(Ordering::SeqCst)
        {
            return Err(RequestDeviceError::AdapterConsumed);
        }

        let default_queue_desc = desc.default_queue.clone();

        let device = Device::new(hal_device.device, self, desc, self.instance.flags)?;
        let device = Arc::new(device);

        let queue = Queue::new(
            device.clone(),
            hal_device.queue,
            default_queue_desc,
            self.instance.flags,
        )?;
        let queue = Arc::new(queue);

        device.set_queue(&queue);
        device.late_init_resources_with_queue()?;

        resource_log!("Created Device {:?}", Arc::as_ptr(&device));
        resource_log!("Created Queue {:?}", Arc::as_ptr(&queue));

        self.instance.devices.push(&device);

        self.consumed.store(true, Ordering::SeqCst);

        Ok((device, queue))
    }

    /// Validate a device descriptor.
    ///
    /// This validates the provided device descriptor as if it were passed to
    /// [`Self::request_device`]. If [`InstanceFlags::STRICT_WEBGPU_COMPLIANCE`] is active,
    /// the requested extensions in the descriptor will be filtered to remove `wgpu`
    /// extensions, except for those that are included in [`limits::EXEMPT_FEATURES`].
    ///
    /// This may be useful when it is necessary to obtain the device itself from a raw hal
    /// API, but the rest of the `request_device` validation is still desired.
    pub fn validate_device_descriptor(
        &self,
        desc: &mut DeviceDescriptor,
    ) -> Result<(), RequestDeviceError> {
        filter_features_and_limits(
            self.instance.flags,
            &mut desc.required_features,
            &mut desc.required_limits,
        );

        // Verify all features were exposed by the adapter
        if !self.raw.features.contains(desc.required_features) {
            return Err(RequestDeviceError::UnsupportedFeature(
                desc.required_features - self.raw.features,
            ));
        }

        // Check if experimental features are permitted to be enabled.
        if desc
            .required_features
            .intersects(wgt::Features::all_experimental_mask())
            && !desc.experimental_features.is_enabled()
        {
            return Err(RequestDeviceError::ExperimentalFeaturesNotEnabled(
                desc.required_features
                    .intersection(wgt::Features::all_experimental_mask()),
            ));
        }

        let caps = &self.raw.capabilities;
        if Backends::PRIMARY.contains(Backends::from(self.backend()))
            && !caps.downlevel.is_webgpu_compliant()
        {
            let missing_flags = wgt::DownlevelFlags::compliant() - caps.downlevel.flags;
            log::warn!("Missing downlevel flags: {missing_flags:?}\n{DOWNLEVEL_WARNING_MESSAGE}");
            log::warn!("{:#?}", caps.downlevel);
        }

        // Verify feature preconditions
        if desc
            .required_features
            .contains(wgt::Features::MAPPABLE_PRIMARY_BUFFERS)
            && self.raw.info.device_type == wgt::DeviceType::DiscreteGpu
        {
            log::warn!(
                "Feature MAPPABLE_PRIMARY_BUFFERS enabled on a discrete gpu. \
                        This is a massive performance footgun and likely not what you wanted"
            );
        }

        if let Some(failed) = check_limits(&desc.required_limits, &caps.limits).pop() {
            return Err(RequestDeviceError::LimitsExceeded(failed));
        }

        normalize_max_resource_per_shader_stage_limits(&mut desc.required_limits);

        Ok(())
    }

    pub fn request_device(
        self: &Arc<Self>,
        desc: &DeviceDescriptor,
    ) -> Result<(Arc<Device>, Arc<Queue>), RequestDeviceError> {
        profiling::scope!("Adapter::request_device");
        api_log!("Adapter::request_device");

        let mut desc = desc.clone();
        self.validate_device_descriptor(&mut desc)?;

        let open = unsafe {
            self.raw.adapter.open(
                desc.required_features,
                &desc.required_limits,
                &desc.memory_hints,
            )
        }
        .map_err(DeviceError::from_hal)?;

        unsafe { self.create_device_and_queue_from_hal(open, &desc) }
    }
}

impl Drop for Adapter {
    #[allow(trivial_casts)]
    fn drop(&mut self) {
        profiling::scope!("Adapter::drop");
        api_log!("Adapter::drop {:?}", self as *const _);
    }
}

crate::impl_resource_type!(Adapter);
crate::impl_storage_item!(Adapter);

#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum GetSurfaceSupportError {
    #[error("Surface is not supported for the specified backend {0}")]
    NotSupportedByBackend(Backend),
    #[error("Failed to retrieve surface capabilities for the specified adapter.")]
    FailedToRetrieveSurfaceCapabilitiesForAdapter,
}

#[derive(Clone, Debug, Error)]
/// Error when requesting a device from the adapter
#[non_exhaustive]
pub enum RequestDeviceError {
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error(transparent)]
    LimitsExceeded(#[from] FailedLimit),
    #[error("Failed to initialize Timestamp Normalizer")]
    TimestampNormalizerInitFailed(#[from] TimestampNormalizerInitError),
    #[error("Unsupported features were requested: {0}")]
    UnsupportedFeature(wgt::Features),
    #[error(
        "Some experimental features, {0}, were requested, but experimental features are not enabled"
    )]
    ExperimentalFeaturesNotEnabled(wgt::Features),
    #[error("The adapter has already been consumed.")]
    AdapterConsumed,
}

#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum CreateSurfaceError {
    #[error("The backend {0} was not enabled on the instance.")]
    BackendNotEnabled(Backend),
    #[error("Failed to create surface for any enabled backend: {0:?}")]
    FailedToCreateSurfaceForAnyBackend(HashMap<Backend, hal::InstanceError>),
    #[error("The display handle used to create this Instance does not match the one used to create a surface on it")]
    MismatchingDisplayHandle,
    #[error(
        "No `DisplayHandle` is available to create this surface with.  When creating a surface with `create_surface()` \
        you must specify a display handle in `InstanceDescriptor::display`.  \
        Rarely, if you need to create surfaces from different `DisplayHandle`s (ex. different Wayland or X11 connections), \
        you must use `create_surface_unsafe()`."
    )]
    MissingDisplayHandle,
}

/// This function checks that the adapter obeys WebGPU's adapter capability
/// guarantees. Most of the limits are adjusted in wgpu-hal's
/// `adjust_raw_limits` fn. So we only check the remaining properties here.
/// See <https://gpuweb.github.io/gpuweb/#adapter-capability-guarantees>.
fn adapter_allowed(
    flags: InstanceFlags,
    info: &impl fmt::Debug,
    limits: &wgt::Limits,
    features: wgt::Features,
) -> bool {
    // Check "All alignment-class limits must be powers of 2."
    //
    // Even if the application has not requested strict WebGPU compliance,
    // non-power-of-two alignment limits are nonsensical, so don't attempt
    // to use such a device.
    let min_uniform_buffer_offset_alignment = limits.min_uniform_buffer_offset_alignment;
    if !min_uniform_buffer_offset_alignment.is_power_of_two() {
        log::error!(
            "Adapter {:?} min_uniform_buffer_offset_alignment limit is not a power of 2: {:?}",
            info,
            min_uniform_buffer_offset_alignment
        );
        return false;
    }
    let min_storage_buffer_offset_alignment = limits.min_storage_buffer_offset_alignment;
    if !min_storage_buffer_offset_alignment.is_power_of_two() {
        log::error!(
            "Adapter {:?} min_storage_buffer_offset_alignment limit is not a power of 2: {:?}",
            info,
            min_storage_buffer_offset_alignment
        );
        return false;
    }

    // Following checks are only enabled if `STRICT_WEBGPU_COMPLIANCE` is set.
    if !flags.contains(InstanceFlags::STRICT_WEBGPU_COMPLIANCE) {
        return true;
    }

    if !features.contains(wgt::Features::CORE_FEATURES_AND_LIMITS) {
        log::debug!(
            "Adapter {:?} is not WebGPU compliant because it does not support \
            CORE_FEATURES_AND_LIMITS",
            info
        );
        return false;
    }

    true
}

/// How many of a backend's adapters [`Instance::find_adapters`] needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wanted {
    /// Every adapter that suits the request, as [`Instance::request_adapters`] returns.
    All,
    /// The best adapter of each backend, which the backend may select without exposing its
    /// other adapters, as for [`Instance::request_adapter`].
    Best,
}

/// The backends that have no adapter for a request, by reason, for
/// [`wgt::RequestAdapterError::NotFound`].
#[derive(Debug)]
struct UnsuitableBackends {
    no_adapter: Backends,
    no_fallback: Backends,
    incompatible_surface: Backends,
}

impl UnsuitableBackends {
    /// No backend, for no reason yet.
    fn none() -> Self {
        Self {
            no_adapter: Backends::empty(),
            no_fallback: Backends::empty(),
            incompatible_surface: Backends::empty(),
        }
    }
}

/// Sorts `adapters` by how well their device type matches `power_preference`
/// ([`device_type_order`]), keeping the order of adapters of the same rank.
/// `PowerPreference::None` keeps the order.
fn rank_adapters(adapters: &mut [hal::DynExposedAdapter], power_preference: PowerPreference) {
    let prefer_integrated_gpu = match power_preference {
        PowerPreference::LowPower => true,
        PowerPreference::HighPerformance => false,
        PowerPreference::None => return,
    };
    adapters
        .sort_by_key(|adapter| device_type_order(adapter.info.device_type, prefer_integrated_gpu));
}

/// The rank of an adapter of `device_type` in [`rank_adapters`], the lowest first:
/// [`BEST_DEVICE_TYPE_ORDER`] at best.
fn device_type_order(device_type: wgt::DeviceType, prefer_integrated_gpu: bool) -> u8 {
    // Since devices of type "Other" might really be "Unknown" and come
    // from APIs like OpenGL that don't specify device type, Prefer more
    // Specific types over Other.
    //
    // This means that backends which do provide accurate device types
    // will be preferred if their device type indicates an actual
    // hardware GPU (integrated or discrete).
    match device_type {
        wgt::DeviceType::DiscreteGpu if prefer_integrated_gpu => 2,
        wgt::DeviceType::IntegratedGpu if prefer_integrated_gpu => 1,
        wgt::DeviceType::DiscreteGpu => 1,
        wgt::DeviceType::IntegratedGpu => 2,
        wgt::DeviceType::Other => 3,
        wgt::DeviceType::VirtualGpu => 4,
        wgt::DeviceType::Cpu => 5,
    }
}

/// The lowest [`device_type_order`]: an integrated GPU's when one is preferred, else a
/// discrete GPU's.
const BEST_DEVICE_TYPE_ORDER: u8 = 1;

/// Whether the adapter that [`rank_adapters`] ranks first among `adapters` stays first
/// whatever adapters that suit `desc` are appended to them.
///
/// The sort is stable, so an adapter stays before the later adapters of its rank. It
/// stays first, then, if no adapter that suits `desc` ranks better than the best of
/// `adapters`: if `adapters` holds any adapter, for [`PowerPreference::None`], which keeps
/// the order, and for `force_fallback_adapter`, which only `Cpu` adapters suit
/// ([`Instance::suitable_adapters`]); else if it holds one of [`BEST_DEVICE_TYPE_ORDER`].
fn ranks_first_whatever_follows(
    adapters: &[hal::DynExposedAdapter],
    desc: &wgt::RequestAdapterOptions<&Surface>,
) -> bool {
    let prefer_integrated_gpu = match desc.power_preference {
        _ if desc.force_fallback_adapter => return !adapters.is_empty(),
        PowerPreference::None => return !adapters.is_empty(),
        PowerPreference::LowPower => true,
        PowerPreference::HighPerformance => false,
    };
    adapters.iter().any(|adapter| {
        device_type_order(adapter.info.device_type, prefer_integrated_gpu) == BEST_DEVICE_TYPE_ORDER
    })
}

fn filter_features_and_limits(
    flags: InstanceFlags,
    features: &mut wgt::Features,
    limits: &mut wgt::Limits,
) {
    if flags.contains(InstanceFlags::STRICT_WEBGPU_COMPLIANCE) {
        *features &= wgt::Features::all_webgpu_mask() | limits::EXEMPT_FEATURES;
        limits.zero_native_only();
    }
}

fn normalize_max_resource_per_shader_stage_limits(limits: &mut wgt::Limits) {
    // The next steps are from <https://www.w3.org/TR/webgpu/#a-new-device>.

    // > 7. Set `limits.maxStorageBuffersPerShaderStage` to
    // >    `max(limits.maxStorageBuffersPerShaderStage, limits.maxStorageBuffersInVertexStage,
    // >    limits.maxStorageBuffersInFragmentStage)`.

    limits.max_storage_buffers_per_shader_stage = [
        limits.max_storage_buffers_per_shader_stage,
        limits.max_storage_buffers_in_vertex_stage,
        limits.max_storage_buffers_in_fragment_stage,
    ]
    .into_iter()
    .max()
    .unwrap();

    // > 8. Set `limits.maxStorageTexturesPerShaderStage` to
    // >    `max(limits.maxStorageTexturesPerShaderStage, limits.maxStorageTexturesInVertexStage,
    // >    limits.maxStorageTexturesInFragmentStage)`.

    limits.max_storage_textures_per_shader_stage = [
        limits.max_storage_textures_per_shader_stage,
        limits.max_storage_textures_in_vertex_stage,
        limits.max_storage_textures_in_fragment_stage,
    ]
    .into_iter()
    .max()
    .unwrap();

    // > 9. If features contains "core-features-and-limits":
    //
    // NOTE: We don't implement compat (yet?), so we do this unconditionally. See also:
    // <https://github.com/gfx-rs/wgpu/issues/8124>

    // >   1. Set `limits.maxStorageBuffersInVertexStage` and
    // >      `limits.maxStorageBuffersInFragmentStage` to
    // >      `limits.maxStorageBuffersPerShaderStage`.
    limits.max_storage_buffers_in_vertex_stage = limits.max_storage_buffers_per_shader_stage;
    limits.max_storage_buffers_in_fragment_stage = limits.max_storage_buffers_per_shader_stage;

    // >   2. Set `limits.maxStorageTexturesInVertexStage` and
    // >      `limits.maxStorageTexturesInFragmentStage` to
    // >      `limits.maxStorageTexturesPerShaderStage`.
    limits.max_storage_textures_in_vertex_stage = limits.max_storage_textures_per_shader_stage;
    limits.max_storage_textures_in_fragment_stage = limits.max_storage_textures_per_shader_stage;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_power_of_two_uniform_alignment_always_rejected() {
        let limits = wgt::Limits {
            min_uniform_buffer_offset_alignment: 3,
            ..wgt::Limits::defaults()
        };
        assert!(!adapter_allowed(
            InstanceFlags::empty(),
            &"",
            &limits,
            wgt::Features::CORE_FEATURES_AND_LIMITS
        ));
        assert!(!adapter_allowed(
            InstanceFlags::STRICT_WEBGPU_COMPLIANCE,
            &"",
            &limits,
            wgt::Features::CORE_FEATURES_AND_LIMITS
        ));
    }

    #[test]
    fn non_power_of_two_storage_alignment_always_rejected() {
        let limits = wgt::Limits {
            min_storage_buffer_offset_alignment: 96,
            ..wgt::Limits::defaults()
        };
        assert!(!adapter_allowed(
            InstanceFlags::empty(),
            &"",
            &limits,
            wgt::Features::CORE_FEATURES_AND_LIMITS
        ));
        assert!(!adapter_allowed(
            InstanceFlags::STRICT_WEBGPU_COMPLIANCE,
            &"",
            &limits,
            wgt::Features::CORE_FEATURES_AND_LIMITS
        ));
    }

    #[test]
    fn low_limits_allowed_without_strict_compliance() {
        let limits = wgt::Limits {
            max_texture_dimension_1d: 1,
            ..wgt::Limits::defaults()
        };
        assert!(adapter_allowed(
            InstanceFlags::empty(),
            &"",
            &limits,
            wgt::Features::empty()
        ));
    }

    #[test]
    fn low_limits_rejected_with_strict_compliance() {
        let limits = wgt::Limits {
            max_texture_dimension_1d: 1,
            ..wgt::Limits::defaults()
        };
        assert!(!adapter_allowed(
            InstanceFlags::STRICT_WEBGPU_COMPLIANCE,
            &"",
            &limits,
            wgt::Features::empty()
        ));
    }

    #[test]
    fn missing_core_features_and_limits_rejected_with_strict_compliance() {
        assert!(!adapter_allowed(
            InstanceFlags::STRICT_WEBGPU_COMPLIANCE,
            &"",
            &wgt::Limits::defaults(),
            wgt::Features::empty()
        ));
    }

    #[test]
    fn fully_compliant_adapter_always_allowed() {
        assert!(adapter_allowed(
            InstanceFlags::STRICT_WEBGPU_COMPLIANCE,
            &"",
            &wgt::Limits::defaults(),
            wgt::Features::CORE_FEATURES_AND_LIMITS
        ));
    }

    mod storage_resource_limits {
        use super::*;

        #[track_caller]
        fn assert_normalized_eq(non_normalized: &wgt::Limits, expected: &wgt::Limits) {
            let mut normalized = non_normalized.clone();
            normalize_max_resource_per_shader_stage_limits(&mut normalized);
            assert_eq!(&normalized, expected);
        }

        #[test]
        fn normalization_is_idempotent() {
            let original = wgt::Limits {
                max_storage_buffers_in_vertex_stage: 16,
                max_storage_textures_in_vertex_stage: 9,
                ..wgt::Limits::defaults()
            };

            let mut first_normalization = original.clone();
            normalize_max_resource_per_shader_stage_limits(&mut first_normalization);
            assert_ne!(original, first_normalization);

            let mut second_normalization = first_normalization.clone();
            normalize_max_resource_per_shader_stage_limits(&mut second_normalization);
            assert_eq!(first_normalization, second_normalization);
        }

        #[test]
        fn limits_presets_already_normalized() {
            [
                wgt::Limits::defaults(),
                wgt::Limits::downlevel_defaults(),
                wgt::Limits::downlevel_webgl2_defaults(),
                wgt::Limits::unlimited(),
            ]
            .iter()
            .for_each(|l| assert_normalized_eq(l, l))
        }

        #[test]
        fn in_stage_raises_per_shader_stage() {
            assert_normalized_eq(
                &wgt::Limits {
                    max_storage_buffers_per_shader_stage: 8,
                    max_storage_buffers_in_vertex_stage: 16,
                    max_storage_buffers_in_fragment_stage: 16,
                    ..wgt::Limits::defaults()
                },
                &wgt::Limits {
                    max_storage_buffers_per_shader_stage: 16,
                    max_storage_buffers_in_vertex_stage: 16,
                    max_storage_buffers_in_fragment_stage: 16,
                    ..wgt::Limits::defaults()
                },
            );

            assert_normalized_eq(
                &wgt::Limits {
                    max_storage_textures_per_shader_stage: 8,
                    max_storage_textures_in_vertex_stage: 9,
                    max_storage_textures_in_fragment_stage: 9,
                    ..wgt::Limits::defaults()
                },
                &wgt::Limits {
                    max_storage_textures_per_shader_stage: 9,
                    max_storage_textures_in_vertex_stage: 9,
                    max_storage_textures_in_fragment_stage: 9,
                    ..wgt::Limits::defaults()
                },
            );
        }

        #[test]
        fn per_shader_stage_raises_in_stage() {
            assert_normalized_eq(
                &wgt::Limits {
                    max_storage_buffers_per_shader_stage: 16,
                    max_storage_buffers_in_vertex_stage: 4,
                    max_storage_buffers_in_fragment_stage: 4,
                    max_storage_textures_per_shader_stage: 8,
                    max_storage_textures_in_vertex_stage: 4,
                    max_storage_textures_in_fragment_stage: 4,
                    ..wgt::Limits::defaults()
                },
                &wgt::Limits {
                    max_storage_buffers_per_shader_stage: 16,
                    max_storage_buffers_in_vertex_stage: 16,
                    max_storage_buffers_in_fragment_stage: 16,
                    max_storage_textures_per_shader_stage: 8,
                    max_storage_textures_in_vertex_stage: 8,
                    max_storage_textures_in_fragment_stage: 8,
                    ..wgt::Limits::defaults()
                },
            );
        }

        #[test]
        fn lowering_per_shader_stage_noop() {
            assert_normalized_eq(
                &wgt::Limits {
                    max_storage_buffers_per_shader_stage: 1,
                    max_storage_textures_per_shader_stage: 1,
                    ..wgt::Limits::defaults()
                },
                &wgt::Limits::defaults(),
            );
        }
    }

    #[test]
    fn downlevel_default_limits_less_than_default_limits() {
        let res = check_limits(&wgt::Limits::downlevel_defaults(), &wgt::Limits::default());
        assert!(
            res.is_empty(),
            "Downlevel limits are greater than default limits",
        )
    }

    /// `request_adapter` asks the backends in order, and only until the adapters found
    /// include one that no later backend's adapter can rank before. Each backend here
    /// offers a noop adapter of a chosen device type, or none, under a real backend's
    /// name, and counts how many times it is asked for adapters.
    mod backends_asked {
        use core::sync::atomic::AtomicUsize;

        use super::*;

        /// What a backend offers: no adapter, or one of `device_type`, which a limit
        /// bucket takes if `bucketed` (else `apply_limit_buckets` drops it).
        #[derive(Clone, Copy, Debug)]
        enum Offer {
            Nothing,
            Adapter {
                device_type: wgt::DeviceType,
                bucketed: bool,
            },
        }

        const DEVICE_TYPES: [wgt::DeviceType; 5] = [
            wgt::DeviceType::Other,
            wgt::DeviceType::IntegratedGpu,
            wgt::DeviceType::DiscreteGpu,
            wgt::DeviceType::VirtualGpu,
            wgt::DeviceType::Cpu,
        ];

        /// The backends' names, in `Instance::new`'s order.
        const BACKENDS: [Backend; 3] = [Backend::Vulkan, Backend::Dx12, Backend::Gl];

        /// Whatever three backends can offer.
        fn every_offer() -> Vec<[Offer; 3]> {
            let mut offers = alloc::vec![Offer::Nothing];
            for device_type in DEVICE_TYPES {
                for bucketed in [true, false] {
                    offers.push(Offer::Adapter {
                        device_type,
                        bucketed,
                    });
                }
            }
            let mut offered = Vec::new();
            for &first in &offers {
                for &second in &offers {
                    for &third in &offers {
                        offered.push([first, second, third]);
                    }
                }
            }
            offered
        }

        /// A backend named `backend` that offers the adapter of `noop`, if any.
        struct TestBackend {
            noop: Option<hal::noop::Context>,
            backend: Backend,
            /// Whether `request_adapter` selects the adapter, as DX12 does, or declines,
            /// as a backend that is enumerated does.
            lazily: bool,
            /// How many times `enumerate_adapters` and `request_adapter` were called.
            queries: Arc<AtomicUsize>,
        }

        impl TestBackend {
            fn expose(&self) -> Option<hal::DynExposedAdapter> {
                let noop = self.noop.as_ref()?;
                let mut adapter =
                    unsafe { hal::DynInstance::enumerate_adapters(noop, None) }.pop()?;
                adapter.info.name = alloc::format!("{:?}", self.backend);
                Some(adapter)
            }
        }

        impl hal::DynResource for TestBackend {
            fn as_any(&self) -> &dyn core::any::Any {
                self
            }

            fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
                self
            }
        }

        impl hal::DynInstance for TestBackend {
            unsafe fn create_surface(
                &self,
                _display_handle: raw_window_handle::RawDisplayHandle,
                _window_handle: raw_window_handle::RawWindowHandle,
            ) -> Result<Box<dyn hal::DynSurface>, hal::InstanceError> {
                unreachable!("the requests have no surface")
            }

            unsafe fn enumerate_adapters(
                &self,
                _surface_hint: Option<&dyn hal::DynSurface>,
            ) -> Vec<hal::DynExposedAdapter> {
                self.queries.fetch_add(1, Ordering::Relaxed);
                self.expose().into_iter().collect()
            }

            unsafe fn request_adapter(
                &self,
                _power_preference: PowerPreference,
                force_fallback_adapter: bool,
                _surface_hint: Option<&dyn hal::DynSurface>,
            ) -> Option<hal::DynExposedAdapter> {
                self.queries.fetch_add(1, Ordering::Relaxed);
                if !self.lazily {
                    return None;
                }
                self.expose().filter(|adapter| {
                    !force_fallback_adapter || adapter.info.device_type == wgt::DeviceType::Cpu
                })
            }
        }

        fn noop_instance(device_type: wgt::DeviceType, bucketed: bool) -> hal::noop::Context {
            let noop = wgt::NoopBackendOptions {
                enable: true,
                device_type: Some(device_type),
                features: Some(wgt::Features::CORE_FEATURES_AND_LIMITS),
                // No limit bucket takes an adapter below WebGPU's minimum limits.
                limits: (!bucketed).then(|| wgt::Limits {
                    max_texture_dimension_2d: 1024,
                    ..wgt::Limits::default()
                }),
                ..Default::default()
            };
            let desc = hal::InstanceDescriptor {
                name: "test",
                flags: InstanceFlags::empty(),
                memory_budget_thresholds: wgt::MemoryBudgetThresholds::default(),
                backend_options: wgt::BackendOptions {
                    noop,
                    ..Default::default()
                },
                telemetry: None,
                display: None,
            };
            unsafe { <hal::noop::Context as hal::Instance>::init(&desc) }.unwrap()
        }

        /// An instance of backends that offer `offered`, in order, and their query counts.
        fn instance(offered: [Offer; 3], lazily: bool) -> (Arc<Instance>, [Arc<AtomicUsize>; 3]) {
            let queries = [(); 3].map(|()| Arc::new(AtomicUsize::new(0)));
            let instance_per_backend = BACKENDS
                .into_iter()
                .zip(offered)
                .zip(&queries)
                .map(|((backend, offer), queries)| {
                    let noop = match offer {
                        Offer::Nothing => None,
                        Offer::Adapter {
                            device_type,
                            bucketed,
                        } => Some(noop_instance(device_type, bucketed)),
                    };
                    let instance: Box<dyn hal::DynInstance> = Box::new(TestBackend {
                        noop,
                        backend,
                        lazily,
                        queries: queries.clone(),
                    });
                    (backend, instance)
                })
                .collect();
            let instance = Instance {
                instance_per_backend,
                ..Default::default()
            };
            (Arc::new(instance), queries)
        }

        /// The name of the adapter that `request_adapter` returns, or its error, and how
        /// many times it asked each backend for adapters.
        fn request_adapter(
            offered: [Offer; 3],
            lazily: bool,
            desc: &wgt::RequestAdapterOptions<&Surface>,
        ) -> (Result<String, wgt::RequestAdapterError>, [usize; 3]) {
            let (instance, queries) = instance(offered, lazily);
            let selected = instance
                .request_adapter(desc, Backends::all())
                .map(|adapter| adapter.get_info().name);
            (
                selected,
                queries.map(|queries| queries.load(Ordering::Relaxed)),
            )
        }

        /// The name of the adapter that `request_adapters` ranks first, or its error: what
        /// `request_adapter` returns if it asks every backend, each of which offers one
        /// adapter at most here.
        fn ranked_first(
            offered: [Offer; 3],
            desc: &wgt::RequestAdapterOptions<&Surface>,
        ) -> Result<String, wgt::RequestAdapterError> {
            let (instance, queries) = instance(offered, true);
            let ranked = instance.request_adapters(desc, Backends::all());
            assert_eq!(
                queries.map(|queries| queries.load(Ordering::Relaxed)),
                [1; 3],
                "{offered:?}: `request_adapters` enumerates every backend"
            );
            ranked.map(|adapters| adapters[0].get_info().name)
        }

        /// Checks a request of `desc` to backends that offer `offered`:
        /// - `request_adapter` returns the adapter that `request_adapters` ranks first, or
        ///   the same error;
        /// - it asks the backends in order, and asks one after the first only if, given what
        ///   the backends before it offer, an adapter of it could be the one returned.
        ///
        /// Returns whether it asked fewer backends than all.
        fn check(
            offered: [Offer; 3],
            lazily: bool,
            desc: &wgt::RequestAdapterOptions<&Surface>,
        ) -> bool {
            let context = alloc::format!(
                "{offered:?}, {:?}, force_fallback_adapter: {}, apply_limit_buckets: {}, \
                lazily: {lazily}",
                desc.power_preference,
                desc.force_fallback_adapter,
                desc.apply_limit_buckets,
            );
            let (selected, queries) = request_adapter(offered, lazily, desc);
            assert_eq!(selected, ranked_first(offered, desc), "{context}");

            let asked = queries.iter().take_while(|&&queries| queries > 0).count();
            assert!(
                asked > 0 && queries[asked..].iter().all(|&queries| queries == 0),
                "{context}: asked out of order, {queries:?}"
            );
            for later in 1..asked {
                let name = alloc::format!("{:?}", BACKENDS[later]);
                let can_be_returned = DEVICE_TYPES.into_iter().any(|device_type| {
                    let mut offered = offered;
                    offered[later] = Offer::Adapter {
                        device_type,
                        bucketed: true,
                    };
                    ranked_first(offered, desc).is_ok_and(|first| first == name)
                });
                assert!(
                    can_be_returned,
                    "{context}: {name} was asked, but none of its adapters can be returned"
                );
            }
            asked < BACKENDS.len()
        }

        /// [`check`] for every preference, with and without `force_fallback_adapter` and
        /// `apply_limit_buckets`, with backends that select lazily or decline, and for
        /// whatever three backends offer.
        #[test]
        fn asked_until_no_later_backend_can_outrank() {
            let every_offer = every_offer();
            let (mut requests, mut stopped) = (0, 0);
            for power_preference in [
                PowerPreference::None,
                PowerPreference::LowPower,
                PowerPreference::HighPerformance,
            ] {
                for force_fallback_adapter in [false, true] {
                    for apply_limit_buckets in [false, true] {
                        let desc = wgt::RequestAdapterOptions {
                            power_preference,
                            force_fallback_adapter,
                            compatible_surface: None,
                            apply_limit_buckets,
                        };
                        for lazily in [false, true] {
                            for &offered in &every_offer {
                                requests += 1;
                                stopped += usize::from(check(offered, lazily, &desc));
                            }
                        }
                    }
                }
            }
            assert!(
                0 < stopped && stopped < requests,
                "{stopped} of {requests} requests asked fewer backends than all"
            );
        }
    }
}
