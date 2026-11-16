//! Surfaces of the noop backend.
//!
//! A noop surface displays nothing, but it keeps a swapchain's books. Configuring it creates a
//! swapchain of `maximum_frame_latency + 1` images. Acquiring hands out an image that is not
//! acquired yet, or fails with [`SurfaceError::Timeout`] when every image is, as a swapchain
//! whose images are all acquired would. Presenting or discarding a texture gives its image back,
//! as [`crate::Surface::discard_texture`] promises. A texture dropped instead keeps its image
//! acquired until the surface is configured again.
//!
//! It also checks requirements that [`crate::Surface`] and [`crate::Queue`] place on their
//! callers, and records each one it finds broken as a [`SurfaceViolation`] (see
//! [`Surface::take_violations`]), which it also logs as an error. It checks that:
//!
//! - no texture of the swapchain's images, or view of one, is left when the surface is
//!   configured or unconfigured;
//! - no call on the surface overlaps its configuration or unconfiguration;
//! - the surface is configured when it is unconfigured, acquired from, presented or discarded;
//! - only an acquired image of the surface's current swapchain is presented or discarded, and on
//!   the surface it was acquired from;
//! - the surface is configured with [`wgt::SurfaceScalingMode::Auto`] or a scaling mode its
//!   capabilities list ([`SCALING_MODES`]), the latter only on a device that supports present
//!   scaling ([`crate::Device::supports_present_scaling`], from
//!   [`wgt::NoopBackendOptions::present_scaling`]). It also refuses another, as a backend does,
//!   with [`SurfaceError::UnsupportedScalingMode`], leaving its configuration as it was;
//! - a submission uses only acquired images, and lists the surface texture of each one it uses.
//!   This is stricter than [`crate::Queue::submit`], which requires the list for the images that
//!   the submission writes to; wgpu-core lists every image a submission uses;
//! - every submission to the device's queue has completed when a configuration is replaced or
//!   the surface unconfigured. Only a stalled GPU (`Context::stall_gpu`, a test hook) leaves one
//!   incomplete.
//!
//! It does not check, so a test that finds no violation proves nothing about, that:
//!
//! - configuring or unconfiguring does not overlap queue operations, or the configuration of the
//!   device's other surfaces;
//! - the surface is configured for one device at a time, and presented through that device's
//!   queue;
//! - only one texture is acquired at a time;
//! - the GPU work on the images is complete when the surface is configured again (noop runs every
//!   submission when it is made), an image is in the `PRESENT` state when it is presented,
//!   acquisitions and submissions pass the same fence, and a submission lists each surface
//!   texture once.
//!
//! With the `noop_test_hooks` feature, tests can run code inside a call on the surface
//! ([`Surface::run_during_next`]), to interleave another thread's call with it
//! deterministically, make a call fail ([`Surface::fail_next`]), and see which command last wrote
//! to the image last presented ([`Surface::last_presented_write`]).
//!
//! [`SurfaceError::Timeout`]: crate::SurfaceError::Timeout
//! [`SurfaceError::UnsupportedScalingMode`]: crate::SurfaceError::UnsupportedScalingMode

#[cfg(feature = "noop_test_hooks")]
use alloc::boxed::Box;
use alloc::{
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use core::{borrow::Borrow, fmt, mem, ptr, time::Duration};

use super::{lock::SpinLock, Api, Context, Fence, Texture};

/// A call on a [`Surface`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SurfaceCall {
    /// [`crate::Surface::configure`].
    Configure,
    /// [`crate::Surface::unconfigure`].
    Unconfigure,
    /// [`crate::Surface::acquire_texture`].
    Acquire,
    /// [`crate::Queue::present`].
    Present,
    /// [`crate::Surface::discard_texture`].
    Discard,
}

impl SurfaceCall {
    fn configures(self) -> bool {
        matches!(self, Self::Configure | Self::Unconfigure)
    }
}

/// A requirement of [`crate::Surface`] or [`crate::Queue`] that a caller broke.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SurfaceViolation {
    /// `call` (configuring or unconfiguring) while textures or texture views of `images` still
    /// existed.
    ImagesInUse {
        call: SurfaceCall,
        images: Vec<usize>,
    },
    /// `call` started while `in_progress` was in progress on the surface, and one of them
    /// (un)configures it.
    Overlap {
        in_progress: SurfaceCall,
        call: SurfaceCall,
    },
    /// `call` on a surface that is not configured.
    NotConfigured(SurfaceCall),
    /// `call` (presenting or discarding) gave back `image`, which is not acquired.
    NotAcquired { call: SurfaceCall, image: usize },
    /// `call` (presenting or discarding) gave back `image` of a swapchain that the surface has
    /// replaced since.
    ReplacedSwapchain { call: SurfaceCall, image: usize },
    /// `call` (presenting or discarding) gave back `image` of another surface.
    OtherSurface { call: SurfaceCall, image: usize },
    /// A submission used `image` without listing its surface texture in `surface_textures`.
    Unlisted { image: usize },
    /// A submission used `image` while it was not acquired, or after its swapchain was replaced.
    NotAcquiredInSubmission { image: usize },
    /// `call` (replacing a configuration, or unconfiguring) while a submission to the device's
    /// queue had not completed.
    SubmissionsPending { call: SurfaceCall },
    /// The surface was configured with a scaling mode that its capabilities do not list, or on a
    /// device whose [`crate::Device::supports_present_scaling`] is false, with one other than
    /// `Auto`.
    UnlistedScalingMode(wgt::SurfaceScalingMode),
}

/// The scaling modes a noop surface lists in its capabilities: those of a DX12 swapchain made
/// from a window.
pub(super) const SCALING_MODES: [wgt::SurfaceScalingMode; 2] = [
    wgt::SurfaceScalingMode::Stretch,
    wgt::SurfaceScalingMode::OneToOne,
];

/// A command that wrote to an image of a noop surface's swapchain, as
/// [`Surface::last_presented_write`] reports it.
#[cfg(feature = "noop_test_hooks")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageWrite {
    /// A copy to the image.
    Copy,
    /// A render pass with the image as a color attachment, which clears it first if `cleared`.
    RenderPass { cleared: bool },
}

/// A surface of the noop backend.
#[derive(Debug)]
pub struct Surface {
    shared: Arc<Shared>,
}

/// A texture acquired from a noop [`Surface`].
#[derive(Debug)]
pub struct SurfaceTexture {
    texture: Texture,
}

impl SurfaceTexture {
    fn image(&self) -> &Arc<Image> {
        self.texture
            .image
            .as_ref()
            .expect("a surface texture has an image")
    }
}

impl Borrow<Texture> for SurfaceTexture {
    fn borrow(&self) -> &Texture {
        &self.texture
    }
}

impl Borrow<dyn crate::DynTexture> for SurfaceTexture {
    fn borrow(&self) -> &dyn crate::DynTexture {
        &self.texture
    }
}

/// An image of a noop [`Surface`]'s swapchain.
///
/// The swapchain holds a strong reference to each of its images, and so does every texture and
/// texture view of one, so the surface can tell whether any is left when it is reconfigured.
#[derive(Debug)]
pub(super) struct Image {
    surface: Weak<Shared>,
    swapchain: u64,
    index: usize,
}

/// A command's use of an [`Image`], which does not keep the image alive.
#[derive(Debug, Clone)]
pub(super) struct ImageUse {
    surface: Weak<Shared>,
    swapchain: u64,
    index: usize,
}

impl ImageUse {
    pub(super) fn new(image: &Image) -> Self {
        Self {
            surface: image.surface.clone(),
            swapchain: image.swapchain,
            index: image.index,
        }
    }

    fn is_of(&self, image: &Image) -> bool {
        Weak::ptr_eq(&self.surface, &image.surface)
            && self.swapchain == image.swapchain
            && self.index == image.index
    }

    pub(super) fn same(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.surface, &other.surface)
            && self.swapchain == other.swapchain
            && self.index == other.index
    }

    /// Records that a submitted command wrote to the image, if it is still acquired.
    #[cfg(feature = "noop_test_hooks")]
    pub(super) fn record_write(&self, write: ImageWrite) {
        let Some(shared) = self.surface.upgrade() else {
            return;
        };
        shared.with_state(|state| {
            if let Some(swapchain) = state.swapchain.as_mut() {
                if swapchain.id == self.swapchain && swapchain.acquired[self.index] {
                    swapchain.last_writes[self.index] = Some(write);
                }
            }
        });
    }

    /// Checks a submission's use of the image: it must be acquired, and its surface texture must
    /// be among the submission's `surface_textures`.
    pub(super) fn check_submitted(&self, surface_textures: &[&SurfaceTexture]) {
        // A surface that no longer exists has nothing left to check against.
        let Some(shared) = self.surface.upgrade() else {
            return;
        };
        let listed = surface_textures
            .iter()
            .any(|texture| self.is_of(texture.image()));
        shared.with_state(|state| {
            if !listed {
                state.violation(SurfaceViolation::Unlisted { image: self.index });
            }
            let acquired = state.swapchain.as_ref().is_some_and(|swapchain| {
                swapchain.id == self.swapchain && swapchain.acquired[self.index]
            });
            if !acquired {
                state.violation(SurfaceViolation::NotAcquiredInSubmission { image: self.index });
            }
        });
    }
}

#[derive(Debug, Default)]
struct Shared {
    /// Only [`Shared::with_state`] locks it.
    state: SpinLock<State>,
}

impl Shared {
    /// Runs `f` on the locked state, then logs the violations that `f` recorded, once the state
    /// is unlocked: no code from outside the backend runs under the lock.
    fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let (result, unlogged) = {
            let mut state = self.state.lock();
            let result = f(&mut state);
            (result, mem::take(&mut state.unlogged))
        };
        for violation in unlogged {
            log::error!("noop surface: {violation:?}");
        }
        result
    }
}

#[derive(Debug, Default)]
struct State {
    swapchain: Option<Swapchain>,
    swapchains_created: u64,
    in_progress: Vec<SurfaceCall>,
    violations: Vec<SurfaceViolation>,
    /// The violations recorded under the current lock, to log once it is released.
    unlogged: Vec<SurfaceViolation>,
    hooks: Hooks,
    /// The command that last wrote to the image last presented, since it was acquired.
    #[cfg(feature = "noop_test_hooks")]
    last_presented_write: Option<ImageWrite>,
}

/// Code to run inside the next call of a kind, as [`Surface::run_during_next`] asks.
#[cfg(feature = "noop_test_hooks")]
type Hook = Box<dyn FnOnce() + Send>;

/// The test hooks of a surface: none without the `noop_test_hooks` feature.
#[derive(Default)]
struct Hooks {
    #[cfg(feature = "noop_test_hooks")]
    during: Vec<(SurfaceCall, Hook)>,
    #[cfg(feature = "noop_test_hooks")]
    failures: Vec<(SurfaceCall, crate::SurfaceError)>,
    /// Whether adapters report that they cannot present to the surface (see
    /// [`Surface::refuse_capabilities`]).
    #[cfg(feature = "noop_test_hooks")]
    refuse_capabilities: bool,
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hooks").finish_non_exhaustive()
    }
}

/// Whether a submission to `device`'s queue has not completed that no wait of the caller's gave
/// up or failed on. Only a stalled GPU leaves one incomplete, which needs the test hooks.
fn submissions_pending(device: &Context) -> bool {
    #[cfg(feature = "noop_test_hooks")]
    {
        device.hooks().submissions_pending(true)
    }
    #[cfg(not(feature = "noop_test_hooks"))]
    {
        let _ = device;
        false
    }
}

/// What the test hooks of a surface ask of a call: code to run inside it, and an error to fail
/// with.
struct CallHooks {
    #[cfg(feature = "noop_test_hooks")]
    during: Option<Hook>,
    failure: Option<crate::SurfaceError>,
}

impl Hooks {
    /// Takes the hooks of `call`: hooks for the same kind of call apply one per call, in the
    /// order they were added.
    fn take(&mut self, call: SurfaceCall) -> CallHooks {
        #[cfg(feature = "noop_test_hooks")]
        {
            let during = self
                .during
                .iter()
                .position(|&(hook, _)| hook == call)
                .map(|index| self.during.remove(index).1);
            let failure = self
                .failures
                .iter()
                .position(|&(failing, _)| failing == call)
                .map(|index| self.failures.remove(index).1);
            CallHooks { during, failure }
        }
        #[cfg(not(feature = "noop_test_hooks"))]
        {
            let _ = call;
            CallHooks { failure: None }
        }
    }
}

#[derive(Debug)]
struct Swapchain {
    id: u64,
    images: Vec<Arc<Image>>,
    acquired: Vec<bool>,
    next: usize,
    /// The scaling mode it was configured with ([`Surface::configured_scaling_mode`]).
    #[cfg(feature = "noop_test_hooks")]
    scaling_mode: wgt::SurfaceScalingMode,
    /// The background colour it was configured with ([`Surface::configured_background_color`]).
    #[cfg(feature = "noop_test_hooks")]
    background_color: Option<wgt::SurfaceBackgroundColor>,
    /// For each image, the command that last wrote to it since it was acquired.
    #[cfg(feature = "noop_test_hooks")]
    last_writes: Vec<Option<ImageWrite>>,
}

impl State {
    fn violation(&mut self, violation: SurfaceViolation) {
        self.unlogged.push(violation.clone());
        self.violations.push(violation);
    }

    /// Checks that no texture or texture view of the current swapchain's images is left.
    fn check_images_released(&mut self, call: SurfaceCall) {
        let Some(swapchain) = &self.swapchain else {
            return;
        };
        let images = swapchain
            .images
            .iter()
            .filter(|image| Arc::strong_count(image) > 1)
            .map(|image| image.index)
            .collect::<Vec<_>>();
        if !images.is_empty() {
            self.violation(SurfaceViolation::ImagesInUse { call, images });
        }
    }
}

/// Records that a call is in progress on a [`Surface`], until dropped.
struct CallGuard<'a> {
    shared: &'a Shared,
    call: SurfaceCall,
}

impl Drop for CallGuard<'_> {
    fn drop(&mut self) {
        self.shared.with_state(|state| {
            let index = state
                .in_progress
                .iter()
                .position(|&call| call == self.call)
                .expect("a call in progress is recorded");
            state.in_progress.remove(index);
        });
    }
}

impl Surface {
    pub(super) fn new() -> Self {
        Self {
            shared: Arc::default(),
        }
    }

    /// Returns the violations of the HAL's requirements found since the last call, and forgets
    /// them.
    pub fn take_violations(&self) -> Vec<SurfaceViolation> {
        self.shared
            .with_state(|state| mem::take(&mut state.violations))
    }

    /// Returns how many images of the current swapchain are acquired: handed out, and neither
    /// presented nor discarded since.
    #[cfg(feature = "noop_test_hooks")]
    pub fn acquired_images(&self) -> usize {
        self.shared.with_state(|state| {
            state.swapchain.as_ref().map_or(0, |swapchain| {
                swapchain
                    .acquired
                    .iter()
                    .filter(|&&acquired| acquired)
                    .count()
            })
        })
    }

    /// Returns the command that last wrote to the image last presented, from when it was
    /// acquired to when it was presented: `None` if nothing wrote to it, or nothing was presented.
    #[cfg(feature = "noop_test_hooks")]
    pub fn last_presented_write(&self) -> Option<ImageWrite> {
        self.shared.with_state(|state| state.last_presented_write)
    }

    /// Returns the scaling mode the current swapchain was configured with, or `None` while the
    /// surface is not configured.
    #[cfg(feature = "noop_test_hooks")]
    pub fn configured_scaling_mode(&self) -> Option<wgt::SurfaceScalingMode> {
        self.shared.with_state(|state| {
            state
                .swapchain
                .as_ref()
                .map(|swapchain| swapchain.scaling_mode)
        })
    }

    /// Returns the background colour the current swapchain was configured with: `None` for none,
    /// or while the surface is not configured.
    #[cfg(feature = "noop_test_hooks")]
    pub fn configured_background_color(&self) -> Option<wgt::SurfaceBackgroundColor> {
        self.shared.with_state(|state| {
            state
                .swapchain
                .as_ref()
                .and_then(|swapchain| swapchain.background_color)
        })
    }

    /// Runs `f` inside the next `call` on this surface, on the thread making the call, while the
    /// call is in progress. Hooks for the same call run one per call, in the order they were
    /// added.
    #[cfg(feature = "noop_test_hooks")]
    pub fn run_during_next(&self, call: SurfaceCall, f: impl FnOnce() + Send + 'static) {
        self.shared
            .with_state(|state| state.hooks.during.push((call, Box::new(f))));
    }

    /// Makes the next `call` on this surface fail with `error`. Only [`SurfaceCall::Configure`],
    /// [`SurfaceCall::Acquire`] and [`SurfaceCall::Present`] can fail. Failures of the same
    /// call apply one per call, in the order they were added.
    ///
    /// A failed configuration leaves the surface unconfigured, like a backend that released the
    /// previous swapchain before failing to create the next one. A failed acquisition acquires
    /// no image, and a failed presentation gives no image back.
    #[cfg(feature = "noop_test_hooks")]
    pub fn fail_next(&self, call: SurfaceCall, error: crate::SurfaceError) {
        assert!(
            matches!(
                call,
                SurfaceCall::Configure | SurfaceCall::Acquire | SurfaceCall::Present
            ),
            "{call:?} cannot fail"
        );
        self.shared
            .with_state(|state| state.hooks.failures.push((call, error)));
    }

    /// Makes [`crate::Adapter::surface_capabilities`] report, while `refuse`, that no adapter
    /// can present to this surface, as a backend whose surface support depends on the adapter
    /// can.
    #[cfg(feature = "noop_test_hooks")]
    pub fn refuse_capabilities(&self, refuse: bool) {
        self.shared
            .with_state(|state| state.hooks.refuse_capabilities = refuse);
    }

    #[cfg(feature = "noop_test_hooks")]
    pub(super) fn refuses_capabilities(&self) -> bool {
        self.shared
            .with_state(|state| state.hooks.refuse_capabilities)
    }

    /// Records the start of `call`, checks that it does not overlap a call it must not, then
    /// runs its test hook, if any. Returns a guard that records its end, and the error the call
    /// must fail with, if any.
    fn enter(&self, call: SurfaceCall) -> (CallGuard<'_>, Option<crate::SurfaceError>) {
        let hooks = self.shared.with_state(|state| {
            let overlapped = state
                .in_progress
                .iter()
                .copied()
                .find(|&in_progress| in_progress.configures() || call.configures());
            if let Some(in_progress) = overlapped {
                state.violation(SurfaceViolation::Overlap { in_progress, call });
            }
            state.in_progress.push(call);
            state.hooks.take(call)
        });
        // Created before the hook runs, so that a hook that panics still records the call's end.
        let guard = CallGuard {
            shared: &self.shared,
            call,
        };
        #[cfg(feature = "noop_test_hooks")]
        if let Some(during) = hooks.during {
            during();
        }
        (guard, hooks.failure)
    }

    /// Gives the image of `texture` back to the swapchain, after presenting or discarding it.
    ///
    /// An image of another surface, or of a swapchain that this surface has replaced since,
    /// gives nothing back.
    fn give_back(&self, texture: SurfaceTexture, call: SurfaceCall) {
        let image = texture.image();
        let of_this_surface = ptr::eq(image.surface.as_ptr(), Arc::as_ptr(&self.shared));
        self.shared.with_state(|state| {
            let violation = match &mut state.swapchain {
                _ if !of_this_surface => Some(SurfaceViolation::OtherSurface {
                    call,
                    image: image.index,
                }),
                None => Some(SurfaceViolation::NotConfigured(call)),
                Some(swapchain) if swapchain.id != image.swapchain => {
                    Some(SurfaceViolation::ReplacedSwapchain {
                        call,
                        image: image.index,
                    })
                }
                Some(swapchain) => (!mem::replace(&mut swapchain.acquired[image.index], false))
                    .then_some(SurfaceViolation::NotAcquired {
                        call,
                        image: image.index,
                    }),
            };
            if let Some(violation) = violation {
                state.violation(violation);
            }
        });
    }

    pub(super) fn present(&self, texture: SurfaceTexture) -> Result<(), crate::SurfaceError> {
        let (_call, failure) = self.enter(SurfaceCall::Present);
        if let Some(error) = failure {
            return Err(error);
        }
        #[cfg(feature = "noop_test_hooks")]
        {
            let image = texture.image();
            let (swapchain_id, index) = (image.swapchain, image.index);
            self.shared.with_state(|state| {
                state.last_presented_write = state
                    .swapchain
                    .as_ref()
                    .filter(|swapchain| swapchain.id == swapchain_id)
                    .and_then(|swapchain| swapchain.last_writes[index]);
            });
        }
        self.give_back(texture, SurfaceCall::Present);
        Ok(())
    }
}

impl crate::Surface for Surface {
    type A = Api;

    unsafe fn configure(
        &self,
        device: &Context,
        config: &crate::SurfaceConfiguration,
    ) -> Result<(), crate::SurfaceError> {
        let (_call, failure) = self.enter(SurfaceCall::Configure);
        let submissions_pending = submissions_pending(device);
        self.shared.with_state(|state| {
            if state.swapchain.is_some() && submissions_pending {
                state.violation(SurfaceViolation::SubmissionsPending {
                    call: SurfaceCall::Configure,
                });
            }
            state.check_images_released(SurfaceCall::Configure);
            if config.scaling_mode != wgt::SurfaceScalingMode::Auto
                && !(SCALING_MODES.contains(&config.scaling_mode)
                    && crate::Device::supports_present_scaling(device, self))
            {
                state.violation(SurfaceViolation::UnlistedScalingMode(config.scaling_mode));
                return Err(crate::SurfaceError::UnsupportedScalingMode);
            }
            if let Some(error) = failure {
                state.swapchain = None;
                return Err(error);
            }
            let id = state.swapchains_created;
            state.swapchains_created += 1;
            let image_count = config.maximum_frame_latency as usize + 1;
            state.swapchain = Some(Swapchain {
                id,
                images: (0..image_count)
                    .map(|index| {
                        Arc::new(Image {
                            surface: Arc::downgrade(&self.shared),
                            swapchain: id,
                            index,
                        })
                    })
                    .collect(),
                acquired: vec![false; image_count],
                next: 0,
                #[cfg(feature = "noop_test_hooks")]
                scaling_mode: config.scaling_mode,
                #[cfg(feature = "noop_test_hooks")]
                background_color: config.background_color,
                #[cfg(feature = "noop_test_hooks")]
                last_writes: vec![None; image_count],
            });
            Ok(())
        })
    }

    unsafe fn unconfigure(&self, device: &Context) {
        let (_call, _) = self.enter(SurfaceCall::Unconfigure);
        let submissions_pending = submissions_pending(device);
        self.shared.with_state(|state| {
            if state.swapchain.is_none() {
                state.violation(SurfaceViolation::NotConfigured(SurfaceCall::Unconfigure));
            }
            if submissions_pending {
                state.violation(SurfaceViolation::SubmissionsPending {
                    call: SurfaceCall::Unconfigure,
                });
            }
            state.check_images_released(SurfaceCall::Unconfigure);
            state.swapchain = None;
        });
    }

    unsafe fn acquire_texture(
        &self,
        _timeout: Option<Duration>,
        _fence: &Fence,
    ) -> Result<crate::AcquiredSurfaceTexture<Api>, crate::SurfaceError> {
        let (_call, failure) = self.enter(SurfaceCall::Acquire);
        if let Some(error) = failure {
            return Err(error);
        }
        self.shared.with_state(|state| {
            let Some(swapchain) = &mut state.swapchain else {
                state.violation(SurfaceViolation::NotConfigured(SurfaceCall::Acquire));
                return Err(crate::SurfaceError::Outdated);
            };
            let image_count = swapchain.images.len();
            let Some(index) = (0..image_count)
                .map(|offset| (swapchain.next + offset) % image_count)
                .find(|&index| !swapchain.acquired[index])
            else {
                return Err(crate::SurfaceError::Timeout);
            };
            swapchain.acquired[index] = true;
            swapchain.next = (index + 1) % image_count;
            #[cfg(feature = "noop_test_hooks")]
            {
                swapchain.last_writes[index] = None;
            }
            Ok(crate::AcquiredSurfaceTexture {
                texture: SurfaceTexture {
                    texture: Texture {
                        image: Some(Arc::clone(&swapchain.images[index])),
                    },
                },
                suboptimal: false,
            })
        })
    }

    unsafe fn discard_texture(&self, texture: SurfaceTexture) {
        let (_call, _) = self.enter(SurfaceCall::Discard);
        self.give_back(texture, SurfaceCall::Discard);
    }
}
