//! Surface operations, on the noop backend's surfaces.
//!
//! A noop surface keeps a swapchain's books and checks the requirements that `wgpu_hal::Surface`
//! and `wgpu_hal::Queue` place on wgpu-core, recording each one it finds broken (see
//! `wgpu_hal::noop::Surface`). Every test checks that none was.
//!
//! The noop surface also runs code inside a call ([`noop::Surface::run_during_next`],
//! [`noop::Context::run_during_next_submit`]), so that a call on another thread can be
//! interleaved with it deterministically, and makes calls fail ([`noop::Surface::fail_next`],
//! [`noop::Context::fail_next`]).

use std::{
    ops::Deref,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

use raw_window_handle::{RawDisplayHandle, RawWindowHandle, WebDisplayHandle, WebWindowHandle};
use wgpu::CurrentSurfaceTexture;
use wgpu_hal::{
    api::Noop,
    noop::{self, DeviceOperation, SurfaceCall, SurfaceViolation},
};
use wgpu_test::fail;

/// How long a call that must stay blocked is given to finish anyway.
const BLOCKED_FOR: Duration = Duration::from_millis(500);

/// The message of `SurfaceError::NothingToPresent`. Configuring destroys the texture acquired
/// from the surface, so presenting or dropping its `SurfaceTexture` afterwards reports it.
const NOTHING_TO_PRESENT: &str = "No surface image is currently acquired";

struct Setup {
    surface: Arc<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    instance: wgpu::Instance,
}

fn setup() -> Setup {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::NOOP,
        backend_options: wgpu::BackendOptions {
            noop: wgpu::NoopBackendOptions::enabled(),
            ..Default::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    // SAFETY: The noop backend does not use the handles.
    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(RawDisplayHandle::Web(WebDisplayHandle::new())),
            raw_window_handle: RawWindowHandle::Web(WebWindowHandle::new(1)),
        })
    }
    .unwrap();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .unwrap();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let mut config = surface.get_default_config(&adapter, 64, 64).unwrap();
    // Three images.
    config.desired_maximum_frame_latency = 2;
    config.usage |= wgpu::TextureUsages::COPY_DST;
    surface.configure(&device, &config);
    Setup {
        surface: Arc::new(surface),
        device,
        queue,
        config,
        instance,
    }
}

impl Setup {
    fn noop_surface(&self) -> impl Deref<Target = noop::Surface> + '_ {
        // SAFETY: The noop surface is only inspected.
        unsafe { self.surface.as_hal::<Noop>() }.unwrap()
    }

    fn noop_device(&self) -> impl Deref<Target = noop::Context> + '_ {
        // SAFETY: The noop device is only inspected.
        unsafe { self.device.as_hal::<Noop>() }.unwrap()
    }

    fn assert_no_violations(&self) {
        assert_eq!(
            self.noop_surface().take_violations(),
            [] as [SurfaceViolation; 0]
        );
    }

    fn acquire(&self) -> wgpu::SurfaceTexture {
        acquire(&self.surface)
    }

    /// A configuration of another size, so that each configuration differs from the last.
    fn resized(&self, step: u32) -> wgpu::SurfaceConfiguration {
        wgpu::SurfaceConfiguration {
            width: self.config.width + step % 2,
            ..self.config.clone()
        }
    }
}

fn acquire(surface: &wgpu::Surface<'_>) -> wgpu::SurfaceTexture {
    match surface.get_current_texture() {
        CurrentSurfaceTexture::Success(frame) | CurrentSurfaceTexture::Suboptimal(frame) => frame,
        other => panic!("no texture acquired: {other:?}"),
    }
}

/// Drops `frame`, whose texture configuring the surface destroyed.
fn drop_destroyed(device: &wgpu::Device, frame: wgpu::SurfaceTexture) {
    fail(device, || drop(frame), Some(NOTHING_TO_PRESENT));
}

// Views of textures that the application holds.

/// Configuring requires the views of the previous configuration's textures to be destroyed. The
/// application can still hold the texture of a presented `SurfaceTexture`, and with it its clear
/// view, and views of it.
#[test]
fn configure_destroys_the_views_of_held_textures() {
    let s = setup();
    let frame = s.acquire();
    let texture = frame.texture.clone();
    let view = frame.texture.create_view(&Default::default());
    s.queue.present(frame);

    s.surface.configure(&s.device, &s.resized(1));
    s.assert_no_violations();

    // The view is of a destroyed texture.
    let mut encoder = s.device.create_command_encoder(&Default::default());
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations::default(),
        })],
        ..Default::default()
    });
    fail(
        &s.device,
        || s.queue.submit([encoder.finish()]),
        Some("destroyed"),
    );
    drop((texture, view));
    s.assert_no_violations();
}

/// A `SurfaceTexture` dropped while unwinding gives its texture back without discarding it. The
/// texture is destroyed all the same, though the application holds it.
#[test]
fn a_released_texture_is_destroyed() {
    let s = setup();
    let held = Mutex::new(None);
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let frame = s.acquire();
        *held.lock().unwrap() = Some(frame.texture.clone());
        panic!("dropping the frame while unwinding");
    }));
    assert!(unwound.is_err());

    s.surface.configure(&s.device, &s.resized(1));
    s.assert_no_violations();
    drop(held);
}

/// Configuring destroys the acquired texture, whose `SurfaceTexture` the application still holds.
#[test]
fn configure_destroys_the_acquired_texture() {
    let s = setup();
    let frame = s.acquire();
    s.surface.configure(&s.device, &s.resized(1));
    s.assert_no_violations();
    drop_destroyed(&s.device, frame);

    let frame = s.acquire();
    assert_eq!(frame.texture.width(), s.resized(1).width);
    s.queue.present(frame);
    s.assert_no_violations();
}

/// A `SurfaceTexture` dropped while unwinding, after configuring destroyed its texture, logs the
/// error of its release: the error sink's default handler panics, and a panic while unwinding
/// aborts the process.
#[test]
fn releasing_a_texture_that_configuring_destroyed_does_not_abort() {
    let s = setup();
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _frame = s.acquire();
        s.surface.configure(&s.device, &s.resized(1));
        panic!("dropping the frame while unwinding");
    }));
    assert!(unwound.is_err());
    s.assert_no_violations();
}

/// A `SurfaceTexture` dropped while unwinding does not wait for an `as_hal` guard that another
/// thread holds, which would hold up the unwinding thread for as long as the guard lives.
/// Configuring the surface destroys the texture instead.
#[test]
fn releasing_a_texture_does_not_wait_for_an_as_hal_guard() {
    let s = setup();
    let guarded = s.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d::default(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let (guard_held, wait_for_guard) = mpsc::channel();
    let (released, wait_for_release) = mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        // SAFETY: The guard is only held.
        let guard = unsafe { guarded.as_hal::<Noop>() };
        guard_held.send(()).unwrap();
        let waited = wait_for_release.recv_timeout(Duration::from_secs(10));
        drop(guard);
        waited.is_ok()
    });
    wait_for_guard.recv().unwrap();

    let held = Mutex::new(None);
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let frame = s.acquire();
        *held.lock().unwrap() = Some(frame.texture.clone());
        panic!("dropping the frame while unwinding");
    }));
    assert!(unwound.is_err());
    let _ = released.send(());
    assert!(
        holder.join().unwrap(),
        "the release waited for the guard to be dropped"
    );

    s.surface.configure(&s.device, &s.resized(1));
    s.assert_no_violations();
    drop(held);
}

/// Releases a texture of the surface while another thread holds the snatch lock, through an
/// `as_hal` guard: the release then leaves the texture alive. Returns the texture.
fn release_while_the_snatch_lock_is_held(s: &Setup) -> wgpu::Texture {
    let guarded = s.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let (guard_held, wait_for_guard) = mpsc::channel();
    let (released, wait_for_release) = mpsc::channel::<()>();
    let holder = thread::spawn(move || {
        // SAFETY: The guard is only held.
        let guard = unsafe { guarded.as_hal::<Noop>() };
        guard_held.send(()).unwrap();
        let _ = wait_for_release.recv_timeout(Duration::from_secs(10));
        drop(guard);
    });
    wait_for_guard.recv().unwrap();
    let held = Mutex::new(None);
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let frame = s.acquire();
        *held.lock().unwrap() = Some(frame.texture.clone());
        panic!("dropping the frame while unwinding");
    }));
    assert!(unwound.is_err());
    released.send(()).unwrap();
    holder.join().unwrap();
    held.into_inner().unwrap().unwrap()
}

/// A texture released while another thread held the snatch lock stays alive until the surface
/// is configured again. Configuring destroys it before it waits for the GPU, so that no work can
/// use it while configuring waits, and destroys its views afterwards: here the wait times out, and
/// the texture is destroyed all the same.
#[test]
fn configuring_destroys_a_released_texture_before_it_waits() {
    let s = setup();
    let texture = release_while_the_snatch_lock_is_held(&s);
    s.noop_device().stall_gpu();
    s.queue.submit([]);
    fail(
        &s.device,
        || s.surface.configure(&s.device, &s.resized(1)),
        Some("Failed to wait for GPU"),
    );
    fail(
        &s.device,
        || {
            s.queue.write_texture(
                texture.as_image_copy(),
                &[0; 4],
                wgpu::TexelCopyBufferLayout::default(),
                wgpu::Extent3d::default(),
            )
        },
        Some("has been destroyed"),
    );
    s.noop_device().resume_gpu();
    s.surface.configure(&s.device, &s.resized(1));
    s.assert_no_violations();
}

// Releasing the device configured before.

/// Configuring a surface for another device releases the device it was configured for. If the
/// application no longer references that device, that is its last reference, and dropping the
/// device drops its device lost callback, and with it what the callback captured. That must
/// happen with the surface unlocked: here, what the callback captured acquires from the surface
/// when it drops.
#[test]
fn configuring_for_another_device_releases_the_previous_one_unlocked() {
    struct AcquiresOnDrop(Arc<wgpu::Surface<'static>>);
    impl Drop for AcquiresOnDrop {
        fn drop(&mut self) {
            drop(self.0.get_current_texture());
        }
    }

    let Setup {
        surface,
        device,
        queue,
        config,
        instance,
    } = setup();
    let captured = AcquiresOnDrop(Arc::clone(&surface));
    device.set_device_lost_callback(move |_reason, _message| {
        let _captured = &captured;
    });
    drop((device, queue));

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .unwrap();
    let (other_device, _other_queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let (done, configured) = mpsc::channel();
    thread::spawn(move || {
        surface.configure(&other_device, &config);
        done.send(()).unwrap();
    });
    configured
        .recv_timeout(Duration::from_secs(10))
        .expect("configuring for another device did not finish");
}

// A GPU that does not finish in time.

/// Configuring waits for the GPU to finish its work on the surface's textures. A wait that times
/// out, as GL's do after about two seconds, is an error, and the surface stays configured.
#[test]
fn configuring_while_the_gpu_does_not_finish_fails() {
    let s = setup();
    s.noop_device().stall_gpu();
    s.queue.submit([]);
    fail(
        &s.device,
        || s.surface.configure(&s.device, &s.resized(1)),
        Some("Failed to wait for GPU"),
    );

    let frame = s.acquire();
    assert_eq!(frame.texture.width(), s.config.width);
    s.noop_device().resume_gpu();
    s.queue.present(frame);
    s.surface.configure(&s.device, &s.resized(1));
    s.assert_no_violations();
}

/// A configuration that times out keeps the surface configured, but the texture acquired from it
/// is destroyed all the same, and given back to the HAL, so that the next acquisition does not
/// take a second texture while the HAL still counts the first one as acquired.
#[test]
fn configuring_while_the_gpu_does_not_finish_gives_the_acquired_texture_back() {
    let s = setup();
    let frame = s.acquire();
    s.noop_device().stall_gpu();
    s.queue.submit([]);
    fail(
        &s.device,
        || s.surface.configure(&s.device, &s.resized(1)),
        Some("Failed to wait for GPU"),
    );
    assert_eq!(s.noop_surface().acquired_images(), 0);
    drop_destroyed(&s.device, frame);

    let frame = s.acquire();
    assert_eq!(s.noop_surface().acquired_images(), 1);
    s.noop_device().resume_gpu();
    s.queue.present(frame);
    s.assert_no_violations();
}

/// Dropping a surface, like unconfiguring it, cannot fail: a wait for the GPU that times out is
/// logged, and the surface is released all the same.
#[test]
fn dropping_a_surface_while_the_gpu_does_not_finish() {
    let s = setup();
    s.noop_device().stall_gpu();
    s.queue.submit([]);
    let Setup {
        surface, device, ..
    } = s;
    drop(Arc::into_inner(surface).expect("the test holds the only reference"));
    // SAFETY: The noop device is only told to resume.
    unsafe { device.as_hal::<Noop>() }.unwrap().resume_gpu();
}

// Device lost callbacks.

/// How long acquiring on another thread is given to finish when it must not block. Generous, as a
/// loaded machine can be slow to schedule it: it is waited for only until it finishes.
const FINISH_WITHIN: Duration = Duration::from_secs(10);

/// Runs `probe` on another thread, and returns whether it finished, or panicked, within
/// [`FINISH_WITHIN`].
fn finishes_in_time(probe: impl FnOnce() + Send + 'static) -> bool {
    let (sender, receiver) = mpsc::channel::<()>();
    thread::spawn(move || {
        // Dropped when `probe` returns or unwinds.
        let _sender = sender;
        probe();
    });
    match receiver.recv_timeout(FINISH_WITHIN) {
        Err(mpsc::RecvTimeoutError::Disconnected) => true,
        Err(mpsc::RecvTimeoutError::Timeout) => false,
        Ok(()) => unreachable!("nothing is sent"),
    }
}

/// Sets a device lost callback that records whether acquiring from the surface on another thread
/// finished in time, which it can't while the callback is called with the surface locked. Returns
/// the record: `None` until the callback is called.
fn probe_the_surface_when_lost(s: &Setup) -> Arc<Mutex<Option<bool>>> {
    let surface = Arc::clone(&s.surface);
    let device = s.device.clone();
    let finished = Arc::new(Mutex::new(None));
    let recorded = Arc::clone(&finished);
    s.device.set_device_lost_callback(move |_reason, _message| {
        let surface = Arc::clone(&surface);
        let device = device.clone();
        let finished = finishes_in_time(move || {
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _ = surface.get_current_texture();
            let _ = pollster::block_on(scope.pop());
        });
        *recorded.lock().unwrap() = Some(finished);
    });
    finished
}

/// Losing the device while configuring waits for the GPU calls the callback once configuring has
/// unlocked the surface, before it returns.
#[test]
fn the_device_lost_callback_of_the_configure_wait_is_called_unlocked() {
    let s = setup();
    let finished = probe_the_surface_when_lost(&s);
    s.noop_device()
        .fail_next(DeviceOperation::Wait, wgpu_hal::DeviceError::Lost);
    s.surface.configure(&s.device, &s.resized(1));
    assert_eq!(*finished.lock().unwrap(), Some(true));
    s.assert_no_violations();
}

/// A HAL configuration that loses the device calls the callback once configuring has unlocked the
/// surface, before it returns.
#[test]
fn the_device_lost_callback_of_a_failed_configuration_is_called_unlocked() {
    let s = setup();
    let finished = probe_the_surface_when_lost(&s);
    s.noop_surface().fail_next(
        SurfaceCall::Configure,
        wgpu_hal::SurfaceError::Device(wgpu_hal::DeviceError::Lost),
    );
    let error_scope = s.device.push_error_scope(wgpu::ErrorFilter::Validation);
    s.surface.configure(&s.device, &s.resized(1));
    let _ = pollster::block_on(error_scope.pop());
    assert_eq!(*finished.lock().unwrap(), Some(true));
    s.assert_no_violations();
}

/// An acquisition that loses the device calls the callback before it returns, with the surface
/// unlocked.
#[test]
fn the_device_lost_callback_of_a_failed_acquisition_is_called_before_it_returns() {
    let s = setup();
    let finished = probe_the_surface_when_lost(&s);
    s.noop_surface().fail_next(
        SurfaceCall::Acquire,
        wgpu_hal::SurfaceError::Device(wgpu_hal::DeviceError::Lost),
    );
    let _ = s.surface.get_current_texture();
    assert_eq!(*finished.lock().unwrap(), Some(true));
    s.assert_no_violations();
}

/// A presentation that loses the device calls the callback before it returns, with the surface
/// unlocked.
#[test]
fn the_device_lost_callback_of_a_failed_presentation_is_called_before_it_returns() {
    let s = setup();
    let finished = probe_the_surface_when_lost(&s);
    let frame = s.acquire();
    s.noop_surface().fail_next(
        SurfaceCall::Present,
        wgpu_hal::SurfaceError::Device(wgpu_hal::DeviceError::Lost),
    );
    s.queue.present(frame);
    assert_eq!(*finished.lock().unwrap(), Some(true));
    s.assert_no_violations();
}

// A write that races a presentation.

/// Presenting a texture that nothing wrote to clears it first. A write to the texture on another
/// thread can be recorded while the presentation prepares the texture, and the presentation must
/// then not clear the texture after the write.
///
/// The write is held inside the `begin_encoding` of the pending writes, after it locked them and
/// before it marks the texture as initialized, while the presentation starts on another thread;
/// then the write goes on.
#[test]
fn a_write_recorded_while_a_presentation_prepares_is_presented() {
    let s = setup();
    let frame = s.acquire();
    let texture = frame.texture.clone();
    // Submits the pending writes, so that the next write begins encoding them anew.
    s.queue.submit([]);

    let (presented, presentation) = mpsc::channel();
    let queue = s.queue.clone();
    s.noop_device().run_during_next_begin_encoding(move || {
        thread::spawn(move || {
            queue.present(frame);
            presented.send(()).unwrap();
        });
        // Lets the presentation reach the pending writes.
        thread::sleep(BLOCKED_FOR);
    });
    let bytes_per_row = texture.width() * texture.format().block_copy_size(None).unwrap();
    // The whole texture, so that the write clears nothing itself.
    s.queue.write_texture(
        texture.as_image_copy(),
        &vec![0; (bytes_per_row * texture.height()) as usize],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: None,
        },
        texture.size(),
    );
    presentation
        .recv_timeout(Duration::from_secs(10))
        .expect("the presentation did not finish");

    assert_eq!(
        s.noop_surface().last_presented_write(),
        Some(noop::ImageWrite::Copy)
    );
    s.assert_no_violations();
}

// Acquiring.

/// Acquiring while a texture is acquired fails without acquiring one from the swapchain, which
/// would never be given back.
#[test]
fn acquiring_while_acquired_takes_no_image() {
    let s = setup();
    let frame = s.acquire();
    for _ in 0..3 {
        fail(
            &s.device,
            || s.surface.get_current_texture(),
            Some("already acquired"),
        );
    }
    s.queue.present(frame);
    assert_eq!(s.noop_surface().acquired_images(), 0);
    for _ in 0..4 {
        let frame = s.acquire();
        s.queue.present(frame);
    }
    s.assert_no_violations();
}

/// Configuring on another thread waits until the acquisition is done.
#[test]
fn configure_waits_for_an_acquisition() {
    let s = setup();
    let finished_during_acquisition = Arc::new(Mutex::new(None));
    let configure_thread = Arc::new(Mutex::new(None));
    {
        let surface = Arc::clone(&s.surface);
        let device = s.device.clone();
        let config = s.resized(1);
        let finished_during_acquisition = Arc::clone(&finished_during_acquisition);
        let configure_thread = Arc::clone(&configure_thread);
        s.noop_surface()
            .run_during_next(SurfaceCall::Acquire, move || {
                let (sender, receiver) = mpsc::channel();
                let handle = thread::spawn(move || {
                    surface.configure(&device, &config);
                    let _ = sender.send(());
                });
                let finished = receiver.recv_timeout(BLOCKED_FOR).is_ok();
                *finished_during_acquisition.lock().unwrap() = Some(finished);
                *configure_thread.lock().unwrap() = Some(handle);
            });
    }

    let frame = s.acquire();
    let handle = configure_thread.lock().unwrap().take().unwrap();
    handle.join().unwrap();
    assert_eq!(*finished_during_acquisition.lock().unwrap(), Some(false));
    s.assert_no_violations();
    drop_destroyed(&s.device, frame);
}
