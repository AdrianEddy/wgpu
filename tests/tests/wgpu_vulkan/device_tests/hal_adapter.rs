//! An adapter that a hal user wraps offers what the instance's enumeration offers of it.

use wgpu_hal::{api::Vulkan, Instance as _};

/// On each adapter, one wrapped with `create_adapter_from_hal` has the limits that enumeration
/// gives it, and a device that requests them opens: with indirect validation on, as an
/// instance made from a hal instance always has it, its buffer limits fit the validation's `u32`
/// offsets. Before, on an adapter whose `maxBufferSize` exceeds `u32::MAX` (NVIDIA's), the
/// wrapped adapter kept the driver's limit, and the device's indirect validation panicked on it.
#[test]
fn a_device_of_a_wrapped_adapter_with_its_limits_opens() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, _messenger)) = super::instance(wgpu::InstanceFlags::empty()) else {
        return;
    };
    let enumerated = super::adapters(&instance);
    let exposed = {
        // SAFETY: Used to enumerate its adapters.
        let hal = unsafe { instance.as_hal::<Vulkan>() }.unwrap();
        // SAFETY: No surface.
        unsafe { hal.enumerate_adapters(None) }
    };
    let mut above = 0;
    for exposed in exposed {
        let name = exposed.info.name.clone();
        let driver_limit = exposed.capabilities.limits.max_buffer_size;
        // SAFETY: Exposed by this instance's Vulkan instance.
        let adapter = unsafe { instance.create_adapter_from_hal(exposed) };
        let same = enumerated
            .iter()
            .find(|other| other.get_info() == adapter.get_info())
            .unwrap();
        assert_eq!(adapter.limits(), same.limits(), "{name:?}");
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            }))
            .unwrap();
        println!(
            "{name:?}: maxBufferSize {driver_limit}, the device's {}",
            device.limits().max_buffer_size
        );
        above += usize::from(driver_limit > u64::from(u32::MAX));
    }
    println!("{above} adapters with a maxBufferSize above u32::MAX");
}
