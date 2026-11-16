#![allow(unused_variables)]

#[cfg(feature = "noop_test_hooks")]
use alloc::boxed::Box;
use alloc::{string::String, sync::Arc, vec, vec::Vec};
use core::{ptr, sync::atomic::Ordering, time::Duration};

use wgpu_sync::atomic::AtomicU64;

use crate::TlasInstance;

mod buffer;
pub use buffer::Buffer;
mod command;
pub use command::CommandBuffer;
mod lock;
mod surface;
#[cfg(test)]
mod tests;
#[cfg(feature = "noop_test_hooks")]
pub use surface::ImageWrite;
pub use surface::{Surface, SurfaceCall, SurfaceTexture, SurfaceViolation};

#[derive(Clone, Debug)]
pub struct Api;

#[derive(Debug)]
pub struct Context {
    options: Arc<wgt::NoopBackendOptions>,
    /// The test hooks of an instance. `None` for an adapter, a device or a queue.
    #[cfg(feature = "noop_test_hooks")]
    instance: Option<InstanceHooks>,
    /// The test hooks of a device and its queue, which share them. `None` for an instance or an
    /// adapter.
    #[cfg(feature = "noop_test_hooks")]
    device: Option<Arc<DeviceHooks>>,
    /// For a device and its queue, the features the device was opened with; for an instance or
    /// an adapter, those the adapter offers.
    features: wgt::Features,
}

impl Context {
    /// An instance.
    fn new_instance(options: &Arc<wgt::NoopBackendOptions>) -> Self {
        Self {
            features: Self::offered(options),
            options: Arc::clone(options),
            #[cfg(feature = "noop_test_hooks")]
            instance: Some(InstanceHooks::default()),
            #[cfg(feature = "noop_test_hooks")]
            device: None,
        }
    }

    /// The features the adapter offers.
    fn offered(options: &wgt::NoopBackendOptions) -> wgt::Features {
        options.features.unwrap_or(wgt::Features::all())
    }

    /// An adapter.
    fn new_adapter(options: &Arc<wgt::NoopBackendOptions>) -> Self {
        Self {
            features: Self::offered(options),
            options: Arc::clone(options),
            #[cfg(feature = "noop_test_hooks")]
            instance: None,
            #[cfg(feature = "noop_test_hooks")]
            device: None,
        }
    }

    /// A device and its queue.
    fn new_device(
        options: &Arc<wgt::NoopBackendOptions>,
        features: wgt::Features,
    ) -> crate::OpenDevice<Api> {
        #[cfg(feature = "noop_test_hooks")]
        let hooks = Arc::new(DeviceHooks::default());
        let new = || Self {
            features,
            options: Arc::clone(options),
            #[cfg(feature = "noop_test_hooks")]
            instance: None,
            #[cfg(feature = "noop_test_hooks")]
            device: Some(Arc::clone(&hooks)),
        };
        crate::OpenDevice {
            device: new(),
            queue: new(),
        }
    }

    /// The instance's only adapter, as [`crate::Instance::enumerate_adapters`] exposes it.
    fn expose_adapter(&self) -> crate::ExposedAdapter<Api> {
        let subgroup_min_size = self
            .options
            .subgroup_min_size
            .unwrap_or(wgt::MINIMUM_SUBGROUP_MIN_SIZE);
        let subgroup_max_size = self
            .options
            .subgroup_max_size
            .unwrap_or(wgt::MAXIMUM_SUBGROUP_MAX_SIZE);
        let features = Self::offered(&self.options);
        let limits = self
            .options
            .limits
            .clone()
            .unwrap_or(CAPABILITIES.limits.clone());

        let default_info = adapter_info();
        let info = wgt::AdapterInfo {
            device_type: self.options.device_type.unwrap_or(default_info.device_type),
            subgroup_min_size,
            subgroup_max_size,
            ..default_info
        };

        let capabilities = crate::Capabilities {
            limits,
            ..CAPABILITIES
        };

        crate::ExposedAdapter {
            adapter: Context::new_adapter(&self.options),
            info,
            features,
            capabilities,
        }
    }
}

/// The test hooks of a noop instance.
#[cfg(feature = "noop_test_hooks")]
#[derive(Debug, Default)]
struct InstanceHooks {
    /// Whether [`crate::Instance::request_adapter`] selects the adapter (see
    /// [`Context::select_adapter_lazily`]).
    select_lazily: wgpu_sync::atomic::AtomicBool,
    /// How many times [`crate::Instance::enumerate_adapters`] ran.
    enumerations: wgpu_sync::atomic::AtomicUsize,
    /// How many times [`crate::Instance::request_adapter`] ran.
    requests: wgpu_sync::atomic::AtomicUsize,
    /// How many of those requests had a surface hint.
    hinted_requests: wgpu_sync::atomic::AtomicUsize,
}

/// How many times a noop instance was asked for adapters, as [`Context::adapter_queries`]
/// reports it.
#[cfg(feature = "noop_test_hooks")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdapterQueries {
    /// Calls of [`crate::Instance::enumerate_adapters`], which exposes every adapter.
    pub enumerations: usize,
    /// Calls of [`crate::Instance::request_adapter`], which exposes at most the one it selects.
    pub requests: usize,
}

#[cfg(feature = "noop_test_hooks")]
impl Context {
    fn instance_hooks(&self) -> &InstanceHooks {
        self.instance
            .as_ref()
            .expect("only a noop instance has instance test hooks")
    }

    /// Makes [`crate::Instance::request_adapter`] of this instance select its adapter, the one
    /// that [`crate::Instance::enumerate_adapters`] exposes, as a backend that selects its
    /// adapter without exposing the others does. Unless `force_fallback_adapter` is requested and
    /// the adapter is not a [`wgt::DeviceType::Cpu`] one: then it selects none.
    ///
    /// By default it declines, as the trait's default does: with one adapter, nothing is spared.
    ///
    /// # Panics
    ///
    /// If `self` is not an instance.
    pub fn select_adapter_lazily(&self, lazily: bool) {
        self.instance_hooks()
            .select_lazily
            .store(lazily, Ordering::Relaxed);
    }

    /// How many times this instance was asked for adapters so far.
    ///
    /// # Panics
    ///
    /// If `self` is not an instance.
    pub fn adapter_queries(&self) -> AdapterQueries {
        let hooks = self.instance_hooks();
        AdapterQueries {
            enumerations: hooks.enumerations.load(Ordering::Relaxed),
            requests: hooks.requests.load(Ordering::Relaxed),
        }
    }

    /// How many of this instance's [`crate::Instance::request_adapter`] calls so far had a
    /// surface hint.
    ///
    /// # Panics
    ///
    /// If `self` is not an instance.
    pub fn hinted_adapter_requests(&self) -> usize {
        self.instance_hooks()
            .hinted_requests
            .load(Ordering::Relaxed)
    }
}

/// An operation of a noop device or queue that [`Context::fail_next`] can make fail.
#[cfg(feature = "noop_test_hooks")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeviceOperation {
    /// [`crate::Queue::submit`].
    Submit,
    /// [`crate::Device::wait`].
    Wait,
    /// [`crate::Device::map_buffer`].
    MapBuffer,
    /// [`crate::Device::check_if_oom`].
    CheckIfOom,
    /// [`crate::CommandEncoder::end_encoding`], of an encoder of the device.
    EndEncoding,
}

/// A requirement of [`crate::Queue`] that a caller broke, as [`Context::take_violations`] reports
/// it.
#[cfg(feature = "noop_test_hooks")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeviceViolation {
    /// [`crate::Queue::wait_for_idle`] while a submission to the queue had not completed.
    WaitForIdleWithSubmissionsPending,
}

/// The test hooks of a noop device and its queue.
#[cfg(feature = "noop_test_hooks")]
#[derive(Default)]
struct DeviceHooks {
    failures: lock::SpinLock<Vec<(DeviceOperation, crate::DeviceError)>>,
    during_submit: lock::SpinLock<Vec<Box<dyn FnOnce() + Send>>>,
    during_begin_encoding: lock::SpinLock<Vec<Box<dyn FnOnce() + Send>>>,
    during_wait: lock::SpinLock<Vec<Box<dyn FnOnce() + Send>>>,
    /// Whether the GPU is stalled (see [`Context::stall_gpu`]).
    stalled: wgpu_sync::atomic::AtomicBool,
    /// The fence value of the last submission made while the GPU is stalled, or 0: it has not
    /// completed, as the GPU has not resumed since.
    pending: AtomicU64,
    /// The highest value that a wait with no timeout gave up on while the GPU is stalled, or 0.
    gave_up_at: AtomicU64,
    /// The highest value that a wait with no timeout failed on while the GPU is stalled, or 0.
    failed_at: AtomicU64,
    violations: lock::SpinLock<Vec<DeviceViolation>>,
}

#[cfg(feature = "noop_test_hooks")]
impl core::fmt::Debug for DeviceHooks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DeviceHooks").finish_non_exhaustive()
    }
}

#[cfg(feature = "noop_test_hooks")]
impl DeviceHooks {
    /// Takes the error that the next `operation` must fail with, if any.
    fn take_failure(&self, operation: DeviceOperation) -> Option<crate::DeviceError> {
        let mut failures = self.failures.lock();
        let index = failures
            .iter()
            .position(|&(failing, _)| failing == operation)?;
        Some(failures.remove(index).1)
    }

    /// Runs the hook of the next submission, if any.
    fn run_during_submit(&self) {
        Self::run_next(&self.during_submit);
    }

    /// Runs the hook of the next `begin_encoding`, if any.
    fn run_during_begin_encoding(&self) {
        Self::run_next(&self.during_begin_encoding);
    }

    /// Runs the hook of the next [`crate::Device::wait`], if any.
    fn run_during_wait(&self) {
        Self::run_next(&self.during_wait);
    }

    /// Whether a submission to the queue has not completed that no wait gave up on: a wait with
    /// no timeout that gives up covers the submissions it waited for (see
    /// [`crate::Queue::wait_for_idle`]). With `failed`, so does one that failed (see
    /// [`crate::Surface::unconfigure`]).
    fn submissions_pending(&self, failed: bool) -> bool {
        let mut covered = self.gave_up_at.load(Ordering::Acquire);
        if failed {
            covered = covered.max(self.failed_at.load(Ordering::Acquire));
        }
        self.pending.load(Ordering::Acquire) > covered
    }

    fn violation(&self, violation: DeviceViolation) {
        self.violations.lock().push(violation);
    }

    /// Runs the first of `hooks`, if any, with `hooks` unlocked.
    fn run_next(hooks: &lock::SpinLock<Vec<Box<dyn FnOnce() + Send>>>) {
        let hook = {
            let mut hooks = hooks.lock();
            (!hooks.is_empty()).then(|| hooks.remove(0))
        };
        if let Some(hook) = hook {
            hook();
        }
    }
}

#[cfg(feature = "noop_test_hooks")]
impl Context {
    fn hooks(&self) -> &DeviceHooks {
        self.device
            .as_deref()
            .expect("only a noop device or queue has device test hooks")
    }

    /// Makes the next `operation` on this device or its queue fail with `error`. Failures of the
    /// same operation apply one per operation, in the order they were added.
    ///
    /// # Panics
    ///
    /// If `self` is an instance or an adapter.
    pub fn fail_next(&self, operation: DeviceOperation, error: crate::DeviceError) {
        self.hooks().failures.lock().push((operation, error));
    }

    /// Runs `f` inside the next submission to this device's queue, on the submitting thread.
    /// Hooks run one per submission, in the order they were added.
    ///
    /// # Panics
    ///
    /// If `self` is an instance or an adapter.
    pub fn run_during_next_submit(&self, f: impl FnOnce() + Send + 'static) {
        self.hooks().during_submit.lock().push(Box::new(f));
    }

    /// Runs `f` inside the next [`crate::CommandEncoder::begin_encoding`] of an encoder of this
    /// device, on the encoding thread. Hooks run one per call, in the order they were added.
    ///
    /// # Panics
    ///
    /// If `self` is an instance or an adapter.
    pub fn run_during_next_begin_encoding(&self, f: impl FnOnce() + Send + 'static) {
        self.hooks().during_begin_encoding.lock().push(Box::new(f));
    }

    /// Runs `f` inside the next [`crate::Device::wait`] of this device, on the waiting thread,
    /// before the wait checks the fence. Hooks run one per wait, in the order they were added.
    ///
    /// # Panics
    ///
    /// If `self` is an instance or an adapter.
    pub fn run_during_next_wait(&self, f: impl FnOnce() + Send + 'static) {
        self.hooks().during_wait.lock().push(Box::new(f));
    }

    /// Returns the violations of the HAL's requirements that this device and its queue found
    /// since the last call, and forgets them. A surface reports its own
    /// ([`Surface::take_violations`]).
    ///
    /// # Panics
    ///
    /// If `self` is an instance or an adapter.
    pub fn take_violations(&self) -> Vec<DeviceViolation> {
        core::mem::take(&mut *self.hooks().violations.lock())
    }

    /// Stalls this device's GPU: the submissions made from now on run, but their fence values
    /// are reached only once [`Self::resume_gpu`] is called. Meanwhile, waiting for them times
    /// out, as on a backend whose waits have a bound, such as GL's.
    ///
    /// # Panics
    ///
    /// If `self` is an instance or an adapter.
    pub fn stall_gpu(&self) {
        self.hooks().stalled.store(true, Ordering::Release);
    }

    /// Resumes this device's GPU after [`Self::stall_gpu`]: its fences reach the values of the
    /// submissions made meanwhile.
    ///
    /// # Panics
    ///
    /// If `self` is an instance or an adapter.
    pub fn resume_gpu(&self) {
        let hooks = self.hooks();
        hooks.stalled.store(false, Ordering::Release);
        hooks.pending.store(0, Ordering::Release);
        hooks.gave_up_at.store(0, Ordering::Release);
        hooks.failed_at.store(0, Ordering::Release);
    }
}
#[derive(Debug)]
pub struct Encoder;
#[derive(Debug)]
pub struct Resource;

/// A texture of the noop backend. A texture of a surface's swapchain has the image.
#[derive(Debug)]
pub struct Texture {
    image: Option<Arc<surface::Image>>,
}

/// A texture view of the noop backend. A view of a surface's texture has the image.
#[derive(Debug)]
pub struct TextureView {
    image: Option<Arc<surface::Image>>,
}

#[derive(Debug)]
pub struct Fence {
    value: AtomicU64,
    /// The value of the last submission made while the GPU was stalled (see
    /// [`Context::stall_gpu`]), which the fence reaches once the GPU resumes.
    #[cfg(feature = "noop_test_hooks")]
    stalled_value: AtomicU64,
}

impl Fence {
    /// The value of the last submission that the GPU has finished.
    fn completed(&self, #[cfg(feature = "noop_test_hooks")] hooks: &DeviceHooks) -> u64 {
        #[cfg(feature = "noop_test_hooks")]
        if !hooks.stalled.load(Ordering::Acquire) {
            self.value
                .fetch_max(self.stalled_value.load(Ordering::Acquire), Ordering::AcqRel);
        }
        self.value.load(Ordering::Acquire)
    }
}

type DeviceResult<T> = Result<T, crate::DeviceError>;

impl crate::Api for Api {
    const VARIANT: wgt::Backend = wgt::Backend::Noop;

    type Instance = Context;
    type Surface = Surface;
    type Adapter = Context;
    type Device = Context;

    type Queue = Context;
    type CommandEncoder = CommandBuffer;
    type CommandBuffer = CommandBuffer;

    type Buffer = Buffer;
    type Texture = Texture;
    type SurfaceTexture = SurfaceTexture;
    type TextureView = TextureView;
    type Sampler = Resource;
    type QuerySet = Resource;
    type Fence = Fence;
    type AccelerationStructure = Resource;
    type PipelineCache = Resource;

    type BindGroupLayout = Resource;
    type BindGroup = Resource;
    type PipelineLayout = Resource;
    type ShaderModule = Resource;
    type RenderPipeline = Resource;
    type RayTracingPipeline = Resource;
    type ComputePipeline = Resource;
}

crate::impl_dyn_resource!(
    Buffer,
    CommandBuffer,
    Context,
    Fence,
    Resource,
    Surface,
    SurfaceTexture,
    Texture,
    TextureView
);

impl crate::DynAccelerationStructure for Resource {}
impl crate::DynBindGroup for Resource {}
impl crate::DynBindGroupLayout for Resource {}
impl crate::DynBuffer for Buffer {}
impl crate::DynCommandBuffer for CommandBuffer {}
impl crate::DynComputePipeline for Resource {}
impl crate::DynFence for Fence {}
impl crate::DynPipelineCache for Resource {}
impl crate::DynPipelineLayout for Resource {}
impl crate::DynQuerySet for Resource {}
impl crate::DynRenderPipeline for Resource {}
impl crate::DynRayTracingPipeline for Resource {}
impl crate::DynSampler for Resource {}
impl crate::DynShaderModule for Resource {}
impl crate::DynSurfaceTexture for SurfaceTexture {}
impl crate::DynTexture for Texture {}
impl crate::DynTextureView for TextureView {}

impl core::borrow::Borrow<dyn crate::DynTexture> for Texture {
    fn borrow(&self) -> &dyn crate::DynTexture {
        self
    }
}

impl crate::Instance for Context {
    type A = Api;

    unsafe fn init(desc: &crate::InstanceDescriptor<'_>) -> Result<Self, crate::InstanceError> {
        let options = Arc::new(desc.backend_options.noop.clone());
        if options.enable {
            Ok(Context::new_instance(&options))
        } else {
            Err(crate::InstanceError::new(String::from(
                "noop backend disabled because NoopBackendOptions::enable is false",
            )))
        }
    }
    unsafe fn create_surface(
        &self,
        _display_handle: raw_window_handle::RawDisplayHandle,
        _window_handle: raw_window_handle::RawWindowHandle,
    ) -> Result<Surface, crate::InstanceError> {
        Ok(Surface::new())
    }
    unsafe fn enumerate_adapters(
        &self,
        _surface_hint: Option<&Surface>,
    ) -> Vec<crate::ExposedAdapter<Api>> {
        #[cfg(feature = "noop_test_hooks")]
        if let Some(hooks) = &self.instance {
            hooks.enumerations.fetch_add(1, Ordering::Relaxed);
        }
        vec![self.expose_adapter()]
    }

    /// Declines, as the default does, unless a test enabled [`Context::select_adapter_lazily`]
    /// on this instance.
    #[cfg(feature = "noop_test_hooks")]
    unsafe fn request_adapter(
        &self,
        _power_preference: wgt::PowerPreference,
        force_fallback_adapter: bool,
        surface_hint: Option<&Surface>,
    ) -> Option<crate::ExposedAdapter<Api>> {
        let Some(hooks) = &self.instance else {
            return None;
        };
        hooks.requests.fetch_add(1, Ordering::Relaxed);
        if surface_hint.is_some() {
            hooks.hinted_requests.fetch_add(1, Ordering::Relaxed);
        }
        if !hooks.select_lazily.load(Ordering::Relaxed) {
            return None;
        }
        let adapter = self.expose_adapter();
        (!force_fallback_adapter || adapter.info.device_type == wgt::DeviceType::Cpu)
            .then_some(adapter)
    }
}

/// Returns the adapter info for the noop backend with the default [`wgt::NoopBackendOptions`].
///
/// This is used in the test harness to construct info about
/// the noop backend adapter without actually initializing wgpu.
pub fn adapter_info() -> wgt::AdapterInfo {
    wgt::AdapterInfo {
        name: String::from("noop wgpu backend"),
        driver: String::from("wgpu"),
        ..wgt::AdapterInfo::new(wgt::DeviceType::Cpu, wgt::Backend::Noop)
    }
}

/// The capabilities of the noop backend.
///
/// This is used in the test harness to construct capabilities
/// of the noop backend without actually initializing wgpu.
pub const CAPABILITIES: crate::Capabilities = {
    crate::Capabilities {
        limits: wgt::Limits::unlimited(),
        alignments: crate::Alignments {
            // All maximally permissive
            buffer_copy_offset: wgt::BufferSize::MIN,
            buffer_copy_pitch: wgt::BufferSize::MIN,
            uniform_bounds_check_alignment: wgt::BufferSize::MIN,
            raw_tlas_instance_size: 0,
            ray_tracing_scratch_buffer_alignment: 1,
            ray_tracing_pipeline_group_data_size: 1,
            ray_tracing_pipeline_group_data_alignment: 1,
            ray_tracing_pipeline_data_offset_alignment: 1,
        },
        downlevel: wgt::DownlevelCapabilities {
            flags: wgt::DownlevelFlags::all(),
            limits: wgt::DownlevelLimits {},
            shader_model: wgt::ShaderModel::Sm5,
        },
        cooperative_matrix_properties: Vec::new(),
    }
};

impl crate::Adapter for Context {
    type A = Api;

    unsafe fn open(
        &self,
        features: wgt::Features,
        _limits: &wgt::Limits,
        _memory_hints: &wgt::MemoryHints,
    ) -> DeviceResult<crate::OpenDevice<Api>> {
        Ok(Context::new_device(&self.options, features))
    }
    unsafe fn texture_format_capabilities(
        &self,
        format: wgt::TextureFormat,
    ) -> crate::TextureFormatCapabilities {
        crate::TextureFormatCapabilities::empty()
    }

    unsafe fn surface_capabilities(&self, surface: &Surface) -> Option<crate::SurfaceCapabilities> {
        #[cfg(feature = "noop_test_hooks")]
        if surface.refuses_capabilities() {
            return None;
        }
        Some(crate::SurfaceCapabilities {
            formats: [
                wgt::TextureFormat::Bgra8Unorm,
                wgt::TextureFormat::Bgra8UnormSrgb,
                wgt::TextureFormat::Rgba8Unorm,
                wgt::TextureFormat::Rgba8UnormSrgb,
            ]
            .into_iter()
            .map(|format| wgt::SurfaceFormatCapabilities {
                format,
                color_spaces: wgt::SurfaceColorSpaces::SRGB,
            })
            .collect(),
            maximum_frame_latency: 1..=3,
            current_extent: None,
            usage: wgt::TextureUses::COLOR_TARGET
                | wgt::TextureUses::RESOURCE
                | wgt::TextureUses::COPY_SRC
                | wgt::TextureUses::COPY_DST,
            present_modes: vec![
                wgt::PresentMode::Fifo,
                wgt::PresentMode::Mailbox,
                wgt::PresentMode::Immediate,
            ],
            composite_alpha_modes: vec![wgt::CompositeAlphaMode::Opaque],
            scaling_modes: surface::SCALING_MODES.to_vec(),
        })
    }

    unsafe fn get_presentation_timestamp(&self) -> wgt::PresentationTimestamp {
        wgt::PresentationTimestamp::INVALID_TIMESTAMP
    }

    fn get_ordered_buffer_usages(&self) -> wgt::BufferUses {
        wgt::BufferUses::INCLUSIVE | wgt::BufferUses::MAP_WRITE
    }

    fn get_ordered_texture_usages(&self) -> wgt::TextureUses {
        wgt::TextureUses::INCLUSIVE
            | wgt::TextureUses::COLOR_TARGET
            | wgt::TextureUses::DEPTH_WRITE
            | wgt::TextureUses::STENCIL_WRITE
    }
}

impl crate::Queue for Context {
    type A = Api;

    unsafe fn submit(
        &self,
        command_buffers: &[&CommandBuffer],
        surface_textures: &[&SurfaceTexture],
        (fence, fence_value): (&Fence, crate::FenceValue),
    ) -> DeviceResult<()> {
        #[cfg(feature = "noop_test_hooks")]
        {
            let hooks = self.hooks();
            hooks.run_during_submit();
            if let Some(error) = hooks.take_failure(DeviceOperation::Submit) {
                return Err(error);
            }
        }
        for cb in command_buffers {
            cb.check_surface_images(surface_textures);
        }
        // All commands are executed synchronously.
        for cb in command_buffers {
            // SAFETY: Caller is responsible for ensuring synchronization between commands and
            // other mutations.
            unsafe {
                cb.execute();
            }
        }
        #[cfg(feature = "noop_test_hooks")]
        if self.hooks().stalled.load(Ordering::Acquire) {
            fence.stalled_value.fetch_max(fence_value, Ordering::AcqRel);
            self.hooks()
                .pending
                .fetch_max(fence_value, Ordering::AcqRel);
            return Ok(());
        }
        fence.value.store(fence_value, Ordering::Release);
        Ok(())
    }
    unsafe fn present(
        &self,
        surface: &Surface,
        texture: SurfaceTexture,
    ) -> Result<(), crate::SurfaceError> {
        surface.present(texture)
    }

    unsafe fn get_timestamp_period(&self) -> f32 {
        1.0
    }

    unsafe fn wait_for_idle(&self) -> Result<(), crate::DeviceError> {
        #[cfg(feature = "noop_test_hooks")]
        if self.hooks().submissions_pending(false) {
            self.hooks()
                .violation(DeviceViolation::WaitForIdleWithSubmissionsPending);
        }
        Ok(())
    }
}

impl crate::Device for Context {
    type A = Api;

    unsafe fn create_buffer(
        &self,
        desc: &crate::BufferDescriptor,
    ) -> DeviceResult<(Buffer, wgt::BufferAddress)> {
        Buffer::new(desc)
    }

    unsafe fn destroy_buffer(&self, buffer: Buffer) {}
    unsafe fn add_raw_buffer(&self, _buffer: &Buffer) {}

    unsafe fn map_buffer(
        &self,
        buffer: &Buffer,
        range: crate::MemoryRange,
    ) -> DeviceResult<crate::BufferMapping> {
        #[cfg(feature = "noop_test_hooks")]
        if let Some(error) = self.hooks().take_failure(DeviceOperation::MapBuffer) {
            return Err(error);
        }
        // Safety: the `wgpu-core` validation layer will prevent any user-accessible aliasing
        // mappings from being created, so we don’t need to perform any checks here, except for
        // bounds checks on the range which are built into `get_slice_ptr()`.
        Ok(crate::BufferMapping {
            ptr: ptr::NonNull::new(buffer.get_slice_ptr(range).cast::<u8>()).unwrap(),
            is_coherent: true,
        })
    }
    unsafe fn unmap_buffer(&self, buffer: &Buffer) {}
    unsafe fn flush_mapped_ranges<I>(&self, buffer: &Buffer, ranges: I) {}
    unsafe fn invalidate_mapped_ranges<I>(&self, buffer: &Buffer, ranges: I) {}

    unsafe fn create_texture(&self, desc: &crate::TextureDescriptor) -> DeviceResult<Texture> {
        Ok(Texture { image: None })
    }
    unsafe fn destroy_texture(&self, texture: Texture) {}
    unsafe fn add_raw_texture(&self, _texture: &Texture) {}

    unsafe fn create_texture_view(
        &self,
        texture: &Texture,
        desc: &crate::TextureViewDescriptor,
    ) -> DeviceResult<TextureView> {
        Ok(TextureView {
            image: texture.image.clone(),
        })
    }
    unsafe fn destroy_texture_view(&self, view: TextureView) {}
    unsafe fn create_sampler(&self, desc: &crate::SamplerDescriptor) -> DeviceResult<Resource> {
        Ok(Resource)
    }
    unsafe fn destroy_sampler(&self, sampler: Resource) {}

    unsafe fn create_command_encoder(
        &self,
        desc: &crate::CommandEncoderDescriptor<Context>,
    ) -> DeviceResult<CommandBuffer> {
        Ok(CommandBuffer::new(
            #[cfg(feature = "noop_test_hooks")]
            self.device.clone(),
        ))
    }

    unsafe fn create_bind_group_layout(
        &self,
        desc: &crate::BindGroupLayoutDescriptor,
    ) -> DeviceResult<Resource> {
        Ok(Resource)
    }
    unsafe fn destroy_bind_group_layout(&self, bg_layout: Resource) {}
    unsafe fn create_pipeline_layout(
        &self,
        desc: &crate::PipelineLayoutDescriptor<Resource>,
    ) -> DeviceResult<Resource> {
        Ok(Resource)
    }
    unsafe fn destroy_pipeline_layout(&self, pipeline_layout: Resource) {}
    unsafe fn create_bind_group(
        &self,
        desc: &crate::BindGroupDescriptor<Resource, Buffer, Resource, TextureView, Resource>,
    ) -> DeviceResult<Resource> {
        Ok(Resource)
    }
    unsafe fn destroy_bind_group(&self, group: Resource) {}

    unsafe fn create_shader_module(
        &self,
        desc: &crate::ShaderModuleDescriptor,
        shader: crate::ShaderInput,
    ) -> Result<Resource, crate::ShaderError> {
        Ok(Resource)
    }
    unsafe fn destroy_shader_module(&self, module: Resource) {}
    unsafe fn create_render_pipeline(
        &self,
        desc: &crate::RenderPipelineDescriptor<Resource, Resource, Resource>,
    ) -> Result<Resource, crate::PipelineError> {
        Ok(Resource)
    }
    unsafe fn destroy_render_pipeline(&self, pipeline: Resource) {}
    unsafe fn create_compute_pipeline(
        &self,
        desc: &crate::ComputePipelineDescriptor<Resource, Resource, Resource>,
    ) -> Result<Resource, crate::PipelineError> {
        Ok(Resource)
    }
    unsafe fn destroy_compute_pipeline(&self, pipeline: Resource) {}
    unsafe fn create_ray_tracing_pipeline(
        &self,
        desc: &crate::RayTracingPipelineDescriptor<Resource, Resource, Resource>,
    ) -> Result<Resource, crate::PipelineError> {
        Ok(Resource)
    }
    unsafe fn destroy_ray_tracing_pipeline(&self, pipeline: Resource) {}
    unsafe fn get_raytracing_pipeline_group_data(
        &self,
        pipeline: &Resource,
        groups: core::ops::Range<u32>,
    ) -> Result<Vec<u8>, crate::DeviceError> {
        Ok(vec![0; groups.count()])
    }
    unsafe fn create_pipeline_cache(
        &self,
        desc: &crate::PipelineCacheDescriptor<'_>,
    ) -> Result<Resource, crate::PipelineCacheError> {
        Ok(Resource)
    }
    unsafe fn destroy_pipeline_cache(&self, cache: Resource) {}

    unsafe fn create_query_set(
        &self,
        desc: &wgt::QuerySetDescriptor<crate::Label>,
    ) -> DeviceResult<Resource> {
        Ok(Resource)
    }
    unsafe fn destroy_query_set(&self, set: Resource) {}
    unsafe fn create_fence(&self) -> DeviceResult<Fence> {
        Ok(Fence {
            value: AtomicU64::new(0),
            #[cfg(feature = "noop_test_hooks")]
            stalled_value: AtomicU64::new(0),
        })
    }
    unsafe fn destroy_fence(&self, fence: Fence) {}
    unsafe fn get_fence_value(&self, fence: &Fence) -> DeviceResult<crate::FenceValue> {
        Ok(fence.completed(
            #[cfg(feature = "noop_test_hooks")]
            self.hooks(),
        ))
    }
    unsafe fn wait(
        &self,
        fence: &Fence,
        value: crate::FenceValue,
        timeout: Option<Duration>,
    ) -> DeviceResult<bool> {
        #[cfg(feature = "noop_test_hooks")]
        if let Some(error) = self.hooks().take_failure(DeviceOperation::Wait) {
            if timeout.is_none() {
                self.hooks().failed_at.fetch_max(value, Ordering::AcqRel);
            }
            return Err(error);
        }
        #[cfg(feature = "noop_test_hooks")]
        self.hooks().run_during_wait();
        let completed = fence.completed(
            #[cfg(feature = "noop_test_hooks")]
            self.hooks(),
        );
        // A stalled GPU finishes nothing until it resumes: the wait times out.
        #[cfg(feature = "noop_test_hooks")]
        if completed < value && self.hooks().stalled.load(Ordering::Acquire) {
            if timeout.is_none() {
                self.hooks().gave_up_at.fetch_max(value, Ordering::AcqRel);
            }
            return Ok(false);
        }
        // The relevant commands must have already been submitted, and noop-backend commands are
        // executed synchronously, so there is no waiting — either it is already done,
        // or this method was called incorrectly.
        assert!(completed >= value, "submission must have already been done");
        Ok(true)
    }

    unsafe fn start_graphics_debugger_capture(&self) -> bool {
        false
    }
    unsafe fn stop_graphics_debugger_capture(&self) {}
    unsafe fn create_acceleration_structure(
        &self,
        desc: &crate::AccelerationStructureDescriptor,
    ) -> DeviceResult<Resource> {
        Ok(Resource)
    }
    unsafe fn get_acceleration_structure_build_sizes<'a>(
        &self,
        _desc: &crate::GetAccelerationStructureBuildSizesDescriptor<'a, Buffer>,
    ) -> crate::AccelerationStructureBuildSizes {
        Default::default()
    }
    unsafe fn get_acceleration_structure_device_address(
        &self,
        _acceleration_structure: &Resource,
    ) -> wgt::BufferAddress {
        Default::default()
    }
    unsafe fn destroy_acceleration_structure(&self, _acceleration_structure: Resource) {}

    fn tlas_instance_to_bytes(&self, instance: TlasInstance, to_extend: &mut Vec<u8>) {}

    fn get_internal_counters(&self) -> wgt::HalCounters {
        Default::default()
    }

    fn features(&self) -> wgt::Features {
        self.features
    }

    fn downlevel_capabilities(&self) -> wgt::DownlevelCapabilities {
        CAPABILITIES.downlevel
    }

    fn supports_present_scaling(&self) -> bool {
        self.options.present_scaling.unwrap_or(true)
    }

    fn check_if_oom(&self) -> DeviceResult<()> {
        #[cfg(feature = "noop_test_hooks")]
        if let Some(error) = self.hooks().take_failure(DeviceOperation::CheckIfOom) {
            return Err(error);
        }
        Ok(())
    }
}
