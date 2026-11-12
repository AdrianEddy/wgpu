use core::ffi::c_void;

use ash::vk;

/// A caller-provided `pNext` chain, stashed by one of the `set_next_*_chain`
/// setters until the Vulkan call that consumes it.
pub(crate) struct PnextChain(*mut vk::BaseOutStructure<'static>);

// SAFETY: The pointer is only dereferenced at the Vulkan call that consumes the
// chain. Each setter's contract keeps the chain valid and unaliased until then.
unsafe impl Send for PnextChain {}
unsafe impl Sync for PnextChain {}

impl PnextChain {
    /// Wraps the raw chain pointer that a `set_next_*_chain` setter received.
    pub(crate) fn new(chain: *mut c_void) -> Self {
        Self(chain.cast())
    }

    /// Whether the chain holds a structure of type `s_type`.
    ///
    /// # Safety
    ///
    /// The chain must still be valid.
    pub(crate) unsafe fn contains(&self, s_type: vk::StructureType) -> bool {
        let mut next = self.0;
        while !next.is_null() {
            // SAFETY: A valid chain, as our caller guarantees.
            unsafe {
                if (*next).s_type == s_type {
                    return true;
                }
                next = (*next).p_next;
            }
        }
        false
    }

    /// Splices this chain in front of `existing`, the current `p_next` of a
    /// Vulkan info struct, and returns the new chain head.
    ///
    /// # Safety
    ///
    /// The chain must still be valid, and the info struct must be passed to the
    /// Vulkan call that consumes the chain.
    pub(crate) unsafe fn splice_into(self, existing: *const c_void) -> *const c_void {
        unsafe {
            let mut tail = self.0;
            while !(*tail).p_next.is_null() {
                tail = (*tail).p_next;
            }
            (*tail).p_next = existing.cast_mut().cast();
        }
        self.0.cast()
    }
}
