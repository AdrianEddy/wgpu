use alloc::vec::Vec;
use arrayvec::ArrayVec;
use ash::vk;
use hashbrown::{HashMap, HashSet};

const POOL_MIN_SETS: u32 = 64;
const POOL_MAX_SETS: u32 = 512;

/// The dynamic counts are tracked separately since dynamic textures/buffers
/// have distinct descriptor types compared to their non-dynamic counterparts.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct DescriptorCounts {
    pub sampler: u32,
    pub sampled_image: u32,
    pub storage_image: u32,
    pub uniform_buffer: u32,
    pub uniform_buffer_dynamic: u32,
    pub storage_buffer: u32,
    pub storage_buffer_dynamic: u32,
    pub acceleration_structure: u32,
}

impl DescriptorCounts {
    fn total(&self) -> u32 {
        self.sampler
            + self.sampled_image
            + self.storage_image
            + self.uniform_buffer
            + self.uniform_buffer_dynamic
            + self.storage_buffer
            + self.storage_buffer_dynamic
            + self.acceleration_structure
    }
}

#[derive(Debug)]
pub struct DescriptorSet {
    raw: vk::DescriptorSet,
    bucket_key: BucketKey,
    pool_index: usize,
}

impl DescriptorSet {
    pub fn raw(&self) -> vk::DescriptorSet {
        self.raw
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct BucketKey {
    counts: DescriptorCounts,
    update_after_bind: bool,
}

struct Pool {
    raw: vk::DescriptorPool,
    capacity: u32,
    available: u32,
    /// Set when the driver refused to allocate from this pool despite it
    /// having free capacity, making `alloc` skip it until the pool is empty
    /// again and can be reset.
    fragmented: bool,
}

/// Keeps track of all pools created with this bucket's [`BucketKey`].
#[derive(Default)]
struct Bucket {
    /// We keep track of all descriptor set layouts that might use this bucket.
    /// If the set becomes empty, the bucket is destroyed.
    layouts: HashSet<vk::DescriptorSetLayout>,
    pools: Vec<Pool>,
    available_sets: u32,
    allocated_sets: u32,
}

impl Bucket {
    fn create_pool(
        &mut self,
        device: &ash::Device,
        key: &BucketKey,
        capacity_hint: u32,
    ) -> Result<(usize, &mut Pool), crate::DeviceError> {
        let index = self.pools.len();
        let pool = create_descriptor_pool(device, key, capacity_hint)?;
        self.available_sets += pool.capacity;
        self.pools.push(pool);
        Ok((index, self.pools.last_mut().unwrap()))
    }
}

pub struct DescriptorAllocator {
    buckets: HashMap<BucketKey, Bucket>,
    max_update_after_bind_descriptors_in_all_pools: u32,
    update_after_bind_descriptors_in_all_pools: u32,
}

impl super::BindGroupLayout {
    fn bucket_key(&self) -> BucketKey {
        let update_after_bind = self.contains_binding_arrays;
        let counts = self.desc_count.clone();
        BucketKey {
            counts,
            update_after_bind,
        }
    }
}

impl DescriptorAllocator {
    pub fn new(max_update_after_bind_descriptors_in_all_pools: u32) -> Self {
        DescriptorAllocator {
            buckets: HashMap::default(),
            max_update_after_bind_descriptors_in_all_pools,
            update_after_bind_descriptors_in_all_pools: 0,
        }
    }

    /// Pools are made by the first `alloc` that needs one, none for a layout
    /// without descriptors.
    pub fn register_layout(&mut self, layout: &super::BindGroupLayout) {
        if layout.desc_count.total() == 0 {
            return;
        }
        let bucket = self.buckets.entry(layout.bucket_key()).or_default();
        assert!(bucket.layouts.insert(layout.raw));
    }

    pub fn unregister_layout(&mut self, device: &ash::Device, layout: &super::BindGroupLayout) {
        if layout.desc_count.total() == 0 {
            return;
        }
        let key = layout.bucket_key();
        let bucket = self.buckets.get_mut(&key).unwrap();

        assert!(bucket.layouts.remove(&layout.raw));

        if bucket.layouts.is_empty() {
            // Remove the bucket and destroy any remaining pools.
            let bucket = self.buckets.remove(&key).unwrap();
            for pool in bucket.pools {
                assert_eq!(
                    pool.available, pool.capacity,
                    "pool is not empty, at least one DescriptorSet has not been freed"
                );
                unsafe { device.destroy_descriptor_pool(pool.raw, None) };
            }
        }
    }

    pub unsafe fn alloc(
        &mut self,
        device: &ash::Device,
        layout: &super::BindGroupLayout,
    ) -> Result<DescriptorSet, crate::DeviceError> {
        let update_after_bind = layout.contains_binding_arrays;
        let total_descriptors = layout.desc_count.total();

        // No shader can use a layout without descriptors, so its groups get no
        // set, and `set_bind_group` binds nothing for them.
        if total_descriptors == 0 {
            return Ok(DescriptorSet {
                raw: vk::DescriptorSet::null(),
                bucket_key: layout.bucket_key(),
                pool_index: usize::MAX,
            });
        }

        if update_after_bind
            && self.max_update_after_bind_descriptors_in_all_pools
                - self.update_after_bind_descriptors_in_all_pools
                < total_descriptors
        {
            return Err(crate::DeviceError::OutOfMemory);
        }

        let key = layout.bucket_key();
        let bucket = self.buckets.get_mut(&key).unwrap();
        let capacity_hint = bucket.allocated_sets;

        // Prefer smaller/older/fuller pools for new allocations to prevent
        // fragmentation and possibly fragmentation of hardware resources
        // (VK_ERROR_FRAGMENTATION)
        let mut candidate = bucket
            .pools
            .iter()
            .position(|pool| !pool.fragmented && pool.available != 0);

        let (raw, pool_index) = loop {
            let index = match candidate {
                Some(index) => index,
                None => bucket.create_pool(device, &key, capacity_hint)?.0,
            };
            let created_pool = candidate.is_none();

            let vk_info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(bucket.pools[index].raw)
                .set_layouts(core::slice::from_ref(&layout.raw));

            // From https://docs.vulkan.org/refpages/latest/refpages/source/VkDescriptorPoolCreateInfo.html#_description
            // > if all sets allocated from the pool since it was created or
            // > most recently reset use the same number of descriptors
            // > (of each type) and the requested allocation also uses that
            // > same number of descriptors (of each type), then
            // > fragmentation must not cause an allocation failure
            //
            // These pools satisfy that condition, but some drivers (e.g.
            // Mali) fail such allocations anyway once a pool has served
            // a number of allocations; freeing sets does not make the
            // pool usable again, only resetting it does.
            //
            // Recover instead of failing: reset a pool that has no live sets,
            // otherwise mark the pool `fragmented` so that allocations skip
            // it. A fragmented pool is reset once its last set is freed.
            //
            // See https://github.com/gfx-rs/wgpu/pull/10264
            match unsafe { device.allocate_descriptor_sets(&vk_info) } {
                Ok(sets) => break (sets[0], index),
                Err(vk::Result::ERROR_FRAGMENTED_POOL | vk::Result::ERROR_OUT_OF_POOL_MEMORY) => {
                    if created_pool {
                        // A fresh pool failing to allocate even a single set leaves
                        // no pool left to fall back to, so drop it and give up.
                        let pool = bucket.pools.pop().unwrap();
                        bucket.available_sets -= pool.capacity;
                        unsafe { device.destroy_descriptor_pool(pool.raw, None) };
                        log::error!(
                            "descriptor set allocation from a fresh pool failed unexpectedly"
                        );
                        return Err(crate::DeviceError::OutOfMemory);
                    }

                    // Resetting a pool restores the spec guarantee that
                    // fragmentation cannot cause an allocation failure, but it
                    // implicitly frees all of the pool's sets, so only reset a
                    // pool that has no live sets.
                    let pool = &bucket.pools[index];
                    if pool.available == pool.capacity {
                        let reset_result = unsafe {
                            device.reset_descriptor_pool(
                                pool.raw,
                                vk::DescriptorPoolResetFlags::empty(),
                            )
                        };
                        if reset_result.is_ok() {
                            match unsafe { device.allocate_descriptor_sets(&vk_info) } {
                                Ok(sets) => break (sets[0], index),
                                Err(
                                    vk::Result::ERROR_FRAGMENTED_POOL
                                    | vk::Result::ERROR_OUT_OF_POOL_MEMORY,
                                ) => {}
                                Err(err) => return Err(super::map_host_device_oom_err(err)),
                            }
                        }
                    }

                    let available = bucket.pools[index].available;
                    let capacity = bucket.pools[index].capacity;
                    bucket.pools[index].fragmented = true;
                    bucket.available_sets -= available;
                    log::debug!(
                        "Retiring fragmented descriptor pool ({available}/{capacity} sets free), retrying on another pool"
                    );
                    candidate = bucket
                        .pools
                        .iter()
                        .position(|pool| !pool.fragmented && pool.available != 0);
                }
                Err(err) => return Err(super::map_host_device_oom_err(err)),
            }
        };

        let pool = &mut bucket.pools[pool_index];
        pool.available -= 1;
        bucket.available_sets -= 1;
        bucket.allocated_sets += 1;
        if update_after_bind {
            self.update_after_bind_descriptors_in_all_pools += total_descriptors;
        }

        Ok(DescriptorSet {
            raw,
            bucket_key: key,
            pool_index,
        })
    }

    pub unsafe fn free(&mut self, device: &ash::Device, set: DescriptorSet) {
        if set.raw == vk::DescriptorSet::null() {
            return;
        }
        let bucket = self.buckets.get_mut(&set.bucket_key).unwrap();
        let pool = bucket.pools.get_mut(set.pool_index).unwrap();

        let result =
            unsafe { device.free_descriptor_sets(pool.raw, core::slice::from_ref(&set.raw())) };
        if let Err(err) = result {
            // vkFreeDescriptorSets is documented to return:
            // - VK_ERROR_UNKNOWN
            // - VK_ERROR_VALIDATION_FAILED (we shouldn't encounter this one)
            // wgpu-hal doesn't currently report errors in destroy functions.
            // Panic here for now. It might be ok to ignore the error but the
            // proper solution would probably be to lose the device.
            panic!("vkFreeDescriptorSets error: {err}, please report this error");
        }

        pool.available += 1;

        if !pool.fragmented {
            bucket.available_sets += 1;
        } else if pool.available == pool.capacity {
            // A fragmented pool's free sets are unusable until it is reset, so
            // they do not count towards `available_sets`.
            // On Mali, freeing sets does not make such a pool usable again;
            // resetting it is what does, and that is only safe once no sets are live.
            let reset_result = unsafe {
                device.reset_descriptor_pool(pool.raw, vk::DescriptorPoolResetFlags::empty())
            };
            if reset_result.is_ok() {
                pool.fragmented = false;
                bucket.available_sets += pool.capacity;
            }
        }
        bucket.allocated_sets -= 1;
        if set.bucket_key.update_after_bind {
            self.update_after_bind_descriptors_in_all_pools -= set.bucket_key.counts.total();
        }

        // Do not immediately destroy empty pools since they might have
        // been recently created.
        // Destroy a pool only if it's empty and we have 1/4th its capacity
        // in other pools.
        // Note that this logic will never destroy the last pool.
        let last = bucket.pools.last().unwrap();
        let last_available = last.available;
        let last_capacity = last.capacity;
        // Only a non-fragmented pool contributes its capacity to
        // `available_sets`.
        let last_contribution = if last.fragmented { 0 } else { last_capacity };
        if last_available == last_capacity
            && bucket.available_sets - last_contribution > last_capacity / 4
        {
            let pool = bucket.pools.pop().unwrap();
            unsafe { device.destroy_descriptor_pool(pool.raw, None) };
            bucket.available_sets -= last_contribution;
        }
    }
}

impl Drop for DescriptorAllocator {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(
                self.buckets.is_empty(),
                "buckets are not empty, at least one BGL has not been unregistered"
            )
        }
    }
}

fn create_descriptor_pool(
    device: &ash::Device,
    key: &BucketKey,
    capacity_hint: u32,
) -> Result<Pool, crate::DeviceError> {
    let counts = &key.counts;

    const NR_OF_DESCRIPTOR_TYPES: usize = 8;

    use vk::DescriptorType as Dt;
    let counts: [_; NR_OF_DESCRIPTOR_TYPES] = [
        (Dt::SAMPLER, counts.sampler),
        (Dt::SAMPLED_IMAGE, counts.sampled_image),
        (Dt::STORAGE_IMAGE, counts.storage_image),
        (Dt::UNIFORM_BUFFER, counts.uniform_buffer),
        (Dt::UNIFORM_BUFFER_DYNAMIC, counts.uniform_buffer_dynamic),
        (Dt::STORAGE_BUFFER, counts.storage_buffer),
        (Dt::STORAGE_BUFFER_DYNAMIC, counts.storage_buffer_dynamic),
        (
            Dt::ACCELERATION_STRUCTURE_KHR,
            counts.acceleration_structure,
        ),
    ];

    // bounded doubling growth strategy
    let mut capacity = capacity_hint
        .clamp(POOL_MIN_SETS, POOL_MAX_SETS)
        .next_power_of_two();

    // Lower capacity to avoid overflowing `count * capacity` calculations
    // below. The resulting `capacity` might not be a power of 2 but that's
    // ok.
    for (_, count) in counts {
        capacity = (u32::MAX / count.max(1)).min(capacity);
    }

    let pool_sizes = counts
        .into_iter()
        .filter(|&(_, count)| count != 0)
        .map(|(ty, count)| vk::DescriptorPoolSize {
            ty,
            descriptor_count: count * capacity,
        })
        .collect::<ArrayVec<_, NR_OF_DESCRIPTOR_TYPES>>();

    // Prior to Vulkan 1.3.215 `poolSizeCount` was required to be non-zero, so
    // add a single small sampler entry if `pool_sizes` would otherwise be
    // empty.
    let mut pool_sizes = pool_sizes;
    if pool_sizes.is_empty() {
        pool_sizes.push(vk::DescriptorPoolSize {
            ty: Dt::SAMPLER,
            descriptor_count: 1,
        });
    }

    let mut flags = vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET;
    if key.update_after_bind {
        flags |= vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND;
    };

    let vk_info = vk::DescriptorPoolCreateInfo::default()
        .flags(flags)
        .max_sets(capacity)
        .pool_sizes(&pool_sizes);

    let raw = unsafe { device.create_descriptor_pool(&vk_info, None) }
        .map_err(super::map_host_device_oom_and_fragmentation_err)?;

    Ok(Pool {
        raw,
        capacity,
        available: capacity,
        fragmented: false,
    })
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use core::{cell::Cell, ffi::c_void, ptr};

    use ash::vk::Handle as _;

    use super::*;

    std::thread_local! {
        /// Pools created and destroyed, and sets allocated and freed, on this thread.
        static CALLS: Cell<[u32; 4]> = const { Cell::new([0; 4]) };
    }

    fn count(call: usize) -> u64 {
        CALLS.with(|calls| {
            let mut counts = calls.get();
            counts[call] += 1;
            calls.set(counts);
            u64::from(counts[call])
        })
    }

    unsafe extern "system" fn create_descriptor_pool(
        _: vk::Device,
        _: *const vk::DescriptorPoolCreateInfo<'_>,
        _: *const vk::AllocationCallbacks<'_>,
        pool: *mut vk::DescriptorPool,
    ) -> vk::Result {
        unsafe { *pool = vk::DescriptorPool::from_raw(count(0)) };
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn destroy_descriptor_pool(
        _: vk::Device,
        _: vk::DescriptorPool,
        _: *const vk::AllocationCallbacks<'_>,
    ) {
        count(1);
    }

    unsafe extern "system" fn allocate_descriptor_sets(
        _: vk::Device,
        _: *const vk::DescriptorSetAllocateInfo<'_>,
        sets: *mut vk::DescriptorSet,
    ) -> vk::Result {
        unsafe { *sets = vk::DescriptorSet::from_raw(count(2)) };
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn free_descriptor_sets(
        _: vk::Device,
        _: vk::DescriptorPool,
        _: u32,
        _: *const vk::DescriptorSet,
    ) -> vk::Result {
        count(3);
        vk::Result::SUCCESS
    }

    /// A device with only the descriptor pool and set functions, which count their calls. Any
    /// other call panics.
    fn counting_device() -> ash::Device {
        // SAFETY: The functions have the signatures of the commands they are loaded for.
        unsafe {
            ash::Device::load_with(
                |name| match name.to_bytes() {
                    b"vkCreateDescriptorPool" => create_descriptor_pool as *const c_void,
                    b"vkDestroyDescriptorPool" => destroy_descriptor_pool as *const c_void,
                    b"vkAllocateDescriptorSets" => allocate_descriptor_sets as *const c_void,
                    b"vkFreeDescriptorSets" => free_descriptor_sets as *const c_void,
                    _ => ptr::null(),
                },
                vk::Device::null(),
            )
        }
    }

    fn layout(raw: u64, desc_count: DescriptorCounts) -> super::super::BindGroupLayout {
        super::super::BindGroupLayout {
            raw: vk::DescriptorSetLayout::from_raw(raw),
            desc_count,
            entries: Box::new([]),
            binding_map: Vec::new(),
            contains_binding_arrays: false,
        }
    }

    #[test]
    fn a_layout_without_descriptors_never_makes_a_pool() {
        let device = counting_device();
        let mut allocator = DescriptorAllocator::new(0);
        let empty = layout(1, DescriptorCounts::default());

        allocator.register_layout(&empty);
        let set = unsafe { allocator.alloc(&device, &empty) }.unwrap();
        assert_eq!(set.raw(), vk::DescriptorSet::null());
        unsafe { allocator.free(&device, set) };
        allocator.unregister_layout(&device, &empty);

        assert_eq!(CALLS.get(), [0; 4]);
    }

    #[test]
    fn a_pool_is_made_for_the_first_set_of_its_layouts() {
        let device = counting_device();
        let mut allocator = DescriptorAllocator::new(0);
        let uniform = DescriptorCounts {
            uniform_buffer: 1,
            ..Default::default()
        };
        let layouts = [layout(1, uniform.clone()), layout(2, uniform)];

        for layout in &layouts {
            allocator.register_layout(layout);
        }
        assert_eq!(CALLS.get(), [0; 4], "registering made a pool");

        let sets = layouts
            .each_ref()
            .map(|layout| unsafe { allocator.alloc(&device, layout) });
        assert_eq!(CALLS.get(), [1, 0, 2, 0], "both sets come from one pool");
        for set in sets {
            unsafe { allocator.free(&device, set.unwrap()) };
        }
        for layout in &layouts {
            allocator.unregister_layout(&device, layout);
        }
        assert_eq!(CALLS.get(), [1, 1, 2, 2]);
    }
}
