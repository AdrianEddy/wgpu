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

use std::{ops::Deref, sync::Arc};

use raw_window_handle::{RawDisplayHandle, RawWindowHandle, WebDisplayHandle, WebWindowHandle};
use wgpu::CurrentSurfaceTexture;
use wgpu_hal::{
    api::Noop,
    noop::{self, SurfaceViolation},
};
use wgpu_test::fail;

/// The message of `SurfaceError::NothingToPresent`. Configuring destroys the texture acquired
/// from the surface, so presenting or dropping its `SurfaceTexture` afterwards reports it.
const NOTHING_TO_PRESENT: &str = "No surface image is currently acquired";

struct Setup {
    surface: Arc<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
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
