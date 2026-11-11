//! The noop backend's surfaces: their swapchain books, and the requirements they check.

use alloc::vec;

use raw_window_handle::{RawDisplayHandle, RawWindowHandle, WebDisplayHandle, WebWindowHandle};

use super::{Context, Surface, SurfaceCall, SurfaceTexture, SurfaceViolation};
use crate::{
    Adapter as _, CommandEncoder as _, Device as _, Instance as _, Queue as _, Surface as _,
};

struct Setup {
    instance: Context,
    surface: Surface,
    device: Context,
    queue: Context,
    fence: super::Fence,
}

fn setup() -> Setup {
    let instance_desc = crate::InstanceDescriptor {
        name: "noop surface tests",
        flags: wgt::InstanceFlags::empty(),
        memory_budget_thresholds: wgt::MemoryBudgetThresholds::default(),
        backend_options: wgt::BackendOptions {
            noop: wgt::NoopBackendOptions::enabled(),
            ..Default::default()
        },
        telemetry: None,
        display: None,
    };
    // SAFETY: The noop backend uses neither handle.
    unsafe {
        let instance = Context::init(&instance_desc).unwrap();
        let surface = new_surface(&instance);
        let adapter = instance
            .enumerate_adapters(Some(&surface))
            .remove(0)
            .adapter;
        let crate::OpenDevice { device, queue } = adapter
            .open(
                wgt::Features::empty(),
                &wgt::Limits::default(),
                &wgt::MemoryHints::default(),
            )
            .unwrap();
        let fence = device.create_fence().unwrap();
        Setup {
            instance,
            surface,
            device,
            queue,
            fence,
        }
    }
}

fn new_surface(instance: &Context) -> Surface {
    // SAFETY: The noop backend uses neither handle.
    unsafe {
        instance.create_surface(
            RawDisplayHandle::Web(WebDisplayHandle::new()),
            RawWindowHandle::Web(WebWindowHandle::new(1)),
        )
    }
    .unwrap()
}

impl Setup {
    /// Configures `surface` with a swapchain of two images.
    fn configure(&self, surface: &Surface) {
        let config = crate::SurfaceConfiguration {
            maximum_frame_latency: 1,
            present_mode: wgt::PresentMode::Fifo,
            composite_alpha_mode: wgt::CompositeAlphaMode::Opaque,
            format: wgt::TextureFormat::Bgra8Unorm,
            color_space: wgt::SurfaceColorSpace::Srgb,
            extent: wgt::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            usage: wgt::TextureUses::COLOR_TARGET,
            view_formats: vec![],
        };
        // SAFETY: The noop backend checks the requirements.
        unsafe { surface.configure(&self.device, &config) }.unwrap();
    }

    fn acquire(&self, surface: &Surface) -> Result<SurfaceTexture, crate::SurfaceError> {
        // SAFETY: The noop backend checks the requirements.
        unsafe { surface.acquire_texture(None, &self.fence) }.map(|acquired| acquired.texture)
    }

    fn present(&self, surface: &Surface, texture: SurfaceTexture) {
        // SAFETY: The noop backend checks the requirements.
        unsafe { self.queue.present(surface, texture) }.unwrap();
    }

    fn discard(&self, surface: &Surface, texture: SurfaceTexture) {
        // SAFETY: The noop backend checks the requirements.
        unsafe { surface.discard_texture(texture) };
    }
}

fn assert_times_out(acquired: Result<SurfaceTexture, crate::SurfaceError>) {
    assert!(
        matches!(acquired, Err(crate::SurfaceError::Timeout)),
        "{acquired:?}"
    );
}

#[test]
fn a_swapchain_hands_out_each_image_once() {
    let s = setup();
    s.configure(&s.surface);
    // `maximum_frame_latency + 1` images.
    let first = s.acquire(&s.surface).unwrap();
    let second = s.acquire(&s.surface).unwrap();
    assert_times_out(s.acquire(&s.surface));

    s.present(&s.surface, first);
    let third = s.acquire(&s.surface).unwrap();
    assert_times_out(s.acquire(&s.surface));
    s.present(&s.surface, second);
    s.present(&s.surface, third);
    let fourth = s.acquire(&s.surface).unwrap();
    let fifth = s.acquire(&s.surface).unwrap();
    s.present(&s.surface, fourth);
    s.present(&s.surface, fifth);
    assert_eq!(s.surface.take_violations(), []);
}

#[test]
fn a_discarded_image_is_handed_out_again() {
    let s = setup();
    s.configure(&s.surface);
    let first = s.acquire(&s.surface).unwrap();
    let second = s.acquire(&s.surface).unwrap();
    s.discard(&s.surface, first);
    let third = s.acquire(&s.surface).unwrap();
    assert_times_out(s.acquire(&s.surface));
    s.present(&s.surface, second);
    s.present(&s.surface, third);
    assert_eq!(s.surface.take_violations(), []);
}

#[test]
fn a_dropped_texture_keeps_its_image_until_the_surface_is_configured_again() {
    let s = setup();
    s.configure(&s.surface);
    drop(s.acquire(&s.surface).unwrap());
    drop(s.acquire(&s.surface).unwrap());
    assert_times_out(s.acquire(&s.surface));
    s.configure(&s.surface);
    let texture = s.acquire(&s.surface).unwrap();
    s.present(&s.surface, texture);
    assert_eq!(s.surface.take_violations(), []);
}

/// A texture presented to another surface leaves both surfaces' books alone, though its image has
/// the same index as an acquired image of the other surface, in a swapchain with the same number.
#[test]
fn an_image_of_another_surface_gives_nothing_back() {
    let s = setup();
    let other = new_surface(&s.instance);
    s.configure(&s.surface);
    s.configure(&other);
    let texture = s.acquire(&s.surface).unwrap();
    let others = [s.acquire(&other).unwrap(), s.acquire(&other).unwrap()];

    s.present(&other, texture);
    assert_eq!(
        other.take_violations(),
        [SurfaceViolation::OtherSurface {
            call: SurfaceCall::Present,
            image: 0,
        }]
    );
    assert_times_out(s.acquire(&other));
    let second = s.acquire(&s.surface).unwrap();
    assert_times_out(s.acquire(&s.surface));

    for texture in others {
        s.present(&other, texture);
    }
    s.present(&s.surface, second);
    assert_eq!(other.take_violations(), []);
    assert_eq!(s.surface.take_violations(), []);
}

/// A texture of a swapchain that the surface has replaced since gives nothing back to the new
/// swapchain, though its image has the same index as an acquired image of the new one.
#[test]
fn an_image_of_a_replaced_swapchain_gives_nothing_back() {
    let s = setup();
    s.configure(&s.surface);
    let old = s.acquire(&s.surface).unwrap();
    s.configure(&s.surface);
    let new = [
        s.acquire(&s.surface).unwrap(),
        s.acquire(&s.surface).unwrap(),
    ];

    s.present(&s.surface, old);
    assert_eq!(
        s.surface.take_violations(),
        [
            SurfaceViolation::ImagesInUse {
                call: SurfaceCall::Configure,
                images: vec![0],
            },
            SurfaceViolation::ReplacedSwapchain {
                call: SurfaceCall::Present,
                image: 0,
            },
        ]
    );
    assert_times_out(s.acquire(&s.surface));
    for texture in new {
        s.present(&s.surface, texture);
    }
    assert_eq!(s.surface.take_violations(), []);
}

#[test]
fn configuring_with_a_view_of_an_image_is_a_violation() {
    let s = setup();
    s.configure(&s.surface);
    let texture = s.acquire(&s.surface).unwrap();
    // SAFETY: The view is destroyed below.
    let view = unsafe {
        s.device.create_texture_view(
            core::borrow::Borrow::borrow(&texture),
            &crate::TextureViewDescriptor {
                label: None,
                format: wgt::TextureFormat::Bgra8Unorm,
                dimension: wgt::TextureViewDimension::D2,
                usage: wgt::TextureUses::COLOR_TARGET,
                range: wgt::ImageSubresourceRange::default(),
                swizzle: wgt::TextureComponentSwizzle::default(),
            },
        )
    }
    .unwrap();
    s.present(&s.surface, texture);

    s.configure(&s.surface);
    assert_eq!(
        s.surface.take_violations(),
        [SurfaceViolation::ImagesInUse {
            call: SurfaceCall::Configure,
            images: vec![0],
        }]
    );
    drop(view);
}

#[test]
fn a_call_on_an_unconfigured_surface_is_a_violation() {
    let s = setup();
    s.configure(&s.surface);
    let texture = s.acquire(&s.surface).unwrap();
    // SAFETY: The noop backend checks the requirements.
    unsafe { s.surface.unconfigure(&s.device) };
    assert_eq!(
        s.surface.take_violations(),
        [SurfaceViolation::ImagesInUse {
            call: SurfaceCall::Unconfigure,
            images: vec![0],
        }]
    );
    s.discard(&s.surface, texture);
    assert!(matches!(
        s.acquire(&s.surface),
        Err(crate::SurfaceError::Outdated)
    ));
    // SAFETY: The noop backend checks the requirements.
    unsafe { s.surface.unconfigure(&s.device) };
    assert_eq!(
        s.surface.take_violations(),
        [
            SurfaceViolation::NotConfigured(SurfaceCall::Discard),
            SurfaceViolation::NotConfigured(SurfaceCall::Acquire),
            SurfaceViolation::NotConfigured(SurfaceCall::Unconfigure),
        ]
    );
}

#[test]
fn a_submission_must_list_the_images_it_uses() {
    let s = setup();
    s.configure(&s.surface);
    let texture = s.acquire(&s.surface).unwrap();
    // SAFETY: The noop backend checks the requirements.
    unsafe {
        let mut encoder = s
            .device
            .create_command_encoder(&crate::CommandEncoderDescriptor {
                label: None,
                queue: &s.queue,
            })
            .unwrap();
        encoder.begin_encoding(None).unwrap();
        encoder.transition_textures(core::iter::once(crate::TextureBarrier {
            texture: core::borrow::Borrow::borrow(&texture),
            range: wgt::ImageSubresourceRange::default(),
            usage: crate::StateTransition {
                from: wgt::TextureUses::UNINITIALIZED,
                to: wgt::TextureUses::PRESENT,
            },
            queue_family_ownership_transfer: None,
        }));
        let commands = encoder.end_encoding().unwrap();
        s.queue.submit(&[&commands], &[], (&s.fence, 1)).unwrap();
        assert_eq!(
            s.surface.take_violations(),
            [SurfaceViolation::Unlisted { image: 0 }]
        );
        s.queue
            .submit(&[&commands], &[&texture], (&s.fence, 2))
            .unwrap();
        assert_eq!(s.surface.take_violations(), []);
        s.present(&s.surface, texture);
        s.queue.submit(&[&commands], &[], (&s.fence, 3)).unwrap();
    }
    assert_eq!(
        s.surface.take_violations(),
        [
            SurfaceViolation::Unlisted { image: 0 },
            SurfaceViolation::NotAcquiredInSubmission { image: 0 },
        ]
    );
}

/// The test hooks of the `noop_test_hooks` feature.
#[cfg(feature = "noop_test_hooks")]
mod hooks {
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, Ordering};

    use super::setup;
    use crate::{
        noop::{DeviceOperation, SurfaceCall, SurfaceViolation},
        CommandEncoder as _, Device as _, Instance as _, Queue as _, Surface as _,
    };

    #[test]
    fn acquired_images_are_counted() {
        let s = setup();
        assert_eq!(s.surface.acquired_images(), 0);
        s.configure(&s.surface);
        let first = s.acquire(&s.surface).unwrap();
        let second = s.acquire(&s.surface).unwrap();
        assert_eq!(s.surface.acquired_images(), 2);
        s.present(&s.surface, first);
        s.discard(&s.surface, second);
        assert_eq!(s.surface.acquired_images(), 0);
    }

    #[test]
    fn configuring_during_an_acquisition_is_a_violation() {
        let s = setup();
        s.configure(&s.surface);
        let s = Arc::new(s);
        let ran = Arc::new(AtomicBool::new(false));
        {
            let hook_setup = Arc::clone(&s);
            let ran = Arc::clone(&ran);
            s.surface.run_during_next(SurfaceCall::Acquire, move || {
                hook_setup.configure(&hook_setup.surface);
                ran.store(true, Ordering::Relaxed);
            });
        }
        let texture = s.acquire(&s.surface).unwrap();
        assert!(ran.load(Ordering::Relaxed));
        assert_eq!(
            s.surface.take_violations(),
            [SurfaceViolation::Overlap {
                in_progress: SurfaceCall::Acquire,
                call: SurfaceCall::Configure,
            }]
        );
        drop(texture);
    }

    #[test]
    fn a_surface_call_fails_when_asked_to() {
        let s = setup();
        s.configure(&s.surface);
        s.surface
            .fail_next(SurfaceCall::Acquire, crate::SurfaceError::Outdated);
        assert!(matches!(
            s.acquire(&s.surface),
            Err(crate::SurfaceError::Outdated)
        ));
        assert_eq!(s.surface.acquired_images(), 0);

        let texture = s.acquire(&s.surface).unwrap();
        s.surface.fail_next(
            SurfaceCall::Present,
            crate::SurfaceError::Device(crate::DeviceError::Lost),
        );
        // SAFETY: The noop backend checks the requirements.
        let presented = unsafe { s.queue.present(&s.surface, texture) };
        assert!(matches!(
            presented,
            Err(crate::SurfaceError::Device(crate::DeviceError::Lost))
        ));
        // The texture was not given back.
        assert_eq!(s.surface.acquired_images(), 1);
        assert_eq!(s.surface.take_violations(), []);
    }

    #[test]
    fn a_failed_configuration_leaves_the_surface_unconfigured() {
        let s = setup();
        s.configure(&s.surface);
        s.surface
            .fail_next(SurfaceCall::Configure, crate::SurfaceError::Lost);
        let config = crate::SurfaceConfiguration {
            maximum_frame_latency: 1,
            present_mode: wgt::PresentMode::Fifo,
            composite_alpha_mode: wgt::CompositeAlphaMode::Opaque,
            format: wgt::TextureFormat::Bgra8Unorm,
            color_space: wgt::SurfaceColorSpace::Srgb,
            extent: wgt::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            usage: wgt::TextureUses::COLOR_TARGET,
            view_formats: alloc::vec![],
        };
        // SAFETY: The noop backend checks the requirements.
        let configured = unsafe { s.surface.configure(&s.device, &config) };
        assert!(matches!(configured, Err(crate::SurfaceError::Lost)));
        assert!(matches!(
            s.acquire(&s.surface),
            Err(crate::SurfaceError::Outdated)
        ));
        assert_eq!(
            s.surface.take_violations(),
            [SurfaceViolation::NotConfigured(SurfaceCall::Acquire)]
        );
    }

    #[test]
    fn a_device_operation_fails_when_asked_to() {
        let s = setup();
        s.device
            .fail_next(DeviceOperation::Submit, crate::DeviceError::OutOfMemory);
        // The queue shares the device's hooks.
        s.queue
            .fail_next(DeviceOperation::Wait, crate::DeviceError::Lost);
        // SAFETY: The noop backend runs nothing that could break the requirements.
        unsafe {
            assert_eq!(
                s.queue.submit(&[], &[], (&s.fence, 1)),
                Err(crate::DeviceError::OutOfMemory)
            );
            s.queue.submit(&[], &[], (&s.fence, 1)).unwrap();
            assert_eq!(
                s.device.wait(&s.fence, 1, None),
                Err(crate::DeviceError::Lost)
            );
            assert_eq!(s.device.wait(&s.fence, 1, None), Ok(true));
        }

        s.device
            .fail_next(DeviceOperation::CheckIfOom, crate::DeviceError::OutOfMemory);
        assert_eq!(
            s.device.check_if_oom(),
            Err(crate::DeviceError::OutOfMemory)
        );
        assert_eq!(s.device.check_if_oom(), Ok(()));

        s.device
            .fail_next(DeviceOperation::MapBuffer, crate::DeviceError::Lost);
        // SAFETY: The buffer is unmapped before it is destroyed, and its mapping is not used.
        unsafe {
            let (buffer, _) = s
                .device
                .create_buffer(&crate::BufferDescriptor {
                    label: None,
                    size: 4,
                    usage: wgt::BufferUses::MAP_READ,
                    memory_flags: crate::MemoryFlags::empty(),
                })
                .unwrap();
            assert!(matches!(
                s.device.map_buffer(&buffer, 0..4),
                Err(crate::DeviceError::Lost)
            ));
            s.device.map_buffer(&buffer, 0..4).unwrap();
            s.device.unmap_buffer(&buffer);
            s.device.destroy_buffer(buffer);
        }

        s.device
            .fail_next(DeviceOperation::EndEncoding, crate::DeviceError::Lost);
        // SAFETY: The encoder records nothing.
        unsafe {
            let mut encoder = s
                .device
                .create_command_encoder(&crate::CommandEncoderDescriptor {
                    label: None,
                    queue: &s.queue,
                })
                .unwrap();
            encoder.begin_encoding(None).unwrap();
            assert!(matches!(
                encoder.end_encoding(),
                Err(crate::DeviceError::Lost)
            ));
            encoder.begin_encoding(None).unwrap();
            encoder.end_encoding().unwrap();
        }
    }

    #[test]
    fn code_runs_during_the_next_submission() {
        let s = setup();
        let ran = Arc::new(AtomicBool::new(false));
        {
            let ran = Arc::clone(&ran);
            s.device
                .run_during_next_submit(move || ran.store(true, Ordering::Relaxed));
        }
        assert!(!ran.load(Ordering::Relaxed));
        // SAFETY: The noop backend runs nothing that could break the requirements.
        unsafe { s.queue.submit(&[], &[], (&s.fence, 1)) }.unwrap();
        assert!(ran.load(Ordering::Relaxed));
    }

    #[test]
    #[should_panic(expected = "only a noop device or queue has test hooks")]
    fn an_instance_has_no_device_hooks() {
        let s = setup();
        s.instance
            .fail_next(DeviceOperation::Submit, crate::DeviceError::Lost);
    }

    #[test]
    #[should_panic(expected = "only a noop device or queue has test hooks")]
    fn an_adapter_has_no_device_hooks() {
        let s = setup();
        // SAFETY: The noop backend has no requirements here.
        let adapter = unsafe { s.instance.enumerate_adapters(None) }
            .remove(0)
            .adapter;
        adapter.run_during_next_submit(|| ());
    }
}
