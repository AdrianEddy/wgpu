//! A poll that finds the device lost reports it as an error of `Device::poll`.
//!
//! `wgpu::Device::poll` returns the `PollError` that [`WaitIdleError::to_poll_error`] gives. It
//! treated an error that had none as fatal, and panicked on a lost device.

use alloc::string::ToString as _;

use super::WaitIdleError;
use crate::device::DeviceError;

/// `Device::maintain` returns a device error only after losing the device
/// (`Device::handle_hal_error`), whichever error the backend reported.
#[test]
fn a_device_error_is_a_lost_device() {
    for error in [DeviceError::Lost, DeviceError::OutOfMemory] {
        let poll_error = WaitIdleError::Device(error.clone()).to_poll_error();
        assert!(
            matches!(poll_error, wgt::PollError::DeviceLost),
            "{error:?}: {poll_error:?}"
        );
        assert_eq!(poll_error.to_string(), "The device was lost.", "{error:?}");
    }
}
