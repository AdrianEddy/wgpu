//! Surfaces of the noop backend.
//!
//! A noop surface displays nothing, but it keeps a swapchain's books. Configuring it creates a
//! swapchain of `maximum_frame_latency + 1` images. Acquiring hands out an image that is not
//! acquired yet, or fails with [`SurfaceError::Timeout`] when every image is, as a swapchain
//! whose images are all acquired would. Presenting or discarding a texture gives its image back,
//! as [`crate::Surface::discard_texture`] promises. A texture dropped instead keeps its image
//! acquired until the surface is configured again.
//!
//! [`SurfaceError::Timeout`]: crate::SurfaceError::Timeout

use alloc::{
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use core::{borrow::Borrow, ptr, time::Duration};

use super::{lock::SpinLock, Api, Context, Fence, Texture};

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
#[derive(Debug)]
pub(super) struct Image {
    surface: Weak<Shared>,
    swapchain: u64,
    index: usize,
}

#[derive(Debug, Default)]
struct Shared {
    state: SpinLock<State>,
}

#[derive(Debug, Default)]
struct State {
    swapchain: Option<Swapchain>,
    swapchains_created: u64,
}

#[derive(Debug)]
struct Swapchain {
    id: u64,
    images: Vec<Arc<Image>>,
    acquired: Vec<bool>,
    next: usize,
}

impl Surface {
    pub(super) fn new() -> Self {
        Self {
            shared: Arc::default(),
        }
    }

    /// Gives the image of `texture` back to the swapchain, after presenting or discarding it.
    ///
    /// An image of another surface, or of a swapchain that this surface has replaced since,
    /// gives nothing back.
    fn give_back(&self, texture: SurfaceTexture) {
        let image = texture.image();
        if !ptr::eq(image.surface.as_ptr(), Arc::as_ptr(&self.shared)) {
            return;
        }
        let mut state = self.shared.state.lock();
        if let Some(swapchain) = &mut state.swapchain {
            if swapchain.id == image.swapchain {
                swapchain.acquired[image.index] = false;
            }
        }
    }

    pub(super) fn present(&self, texture: SurfaceTexture) -> Result<(), crate::SurfaceError> {
        self.give_back(texture);
        Ok(())
    }
}

impl crate::Surface for Surface {
    type A = Api;

    unsafe fn configure(
        &self,
        _device: &Context,
        config: &crate::SurfaceConfiguration,
    ) -> Result<(), crate::SurfaceError> {
        let mut state = self.shared.state.lock();
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
        });
        Ok(())
    }

    unsafe fn unconfigure(&self, _device: &Context) {
        self.shared.state.lock().swapchain = None;
    }

    unsafe fn acquire_texture(
        &self,
        _timeout: Option<Duration>,
        _fence: &Fence,
    ) -> Result<crate::AcquiredSurfaceTexture<Api>, crate::SurfaceError> {
        let mut state = self.shared.state.lock();
        let Some(swapchain) = &mut state.swapchain else {
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
        Ok(crate::AcquiredSurfaceTexture {
            texture: SurfaceTexture {
                texture: Texture {
                    image: Some(Arc::clone(&swapchain.images[index])),
                },
            },
            suboptimal: false,
        })
    }

    unsafe fn discard_texture(&self, texture: SurfaceTexture) {
        self.give_back(texture);
    }
}
