//! The Vulkan backend on real devices, through wgpu.
//!
//! Every test opens a Vulkan device. They skip at run time where there is no Vulkan, and run on
//! CPU adapters too. A run that must not use a GPU leaves them all out by their name, which
//! contains `device_tests::`. They watch the Vulkan calls themselves, through
//! `wgpu_hal::vulkan::call_log`.

#[cfg(any(windows, target_os = "linux"))]
mod device_tests;
