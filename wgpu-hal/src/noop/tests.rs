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
        unsafe { self.queue.present(surface, texture, &[]) }.unwrap();
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
