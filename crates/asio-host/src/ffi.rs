//! The only module with `unsafe`: declarations of two ASIO SDK host functions (`asio.cpp`), which
//! asio-sys compiles into its static library but does not bind. They act on the SDK's one loaded
//! driver and return `ASE_NotPresent` when there is none, so calling them is always defined.
//!
//! The SDK is C++, so the symbols carry MSVC-decorated names (as in asio-sys's own bindings).

use std::os::raw::c_long;

/// `ASE_OK`.
const ASE_OK: c_long = 0;
/// `ASE_NotPresent`: no driver is loaded.
pub(crate) const ASE_NOT_PRESENT: i32 = -1000;

// Linked from asio-sys's `asio` static library (`extern crate` keeps the dependency linked).
extern crate asio_sys as _;

unsafe extern "C" {
    #[cfg_attr(target_pointer_width = "64", link_name = "\u{1}?ASIOGetBufferSize@@YAJPEAJ000@Z")]
    #[cfg_attr(target_pointer_width = "32", link_name = "\u{1}?ASIOGetBufferSize@@YAJPAJ000@Z")]
    fn ASIOGetBufferSize(min: *mut c_long, max: *mut c_long, preferred: *mut c_long, granularity: *mut c_long) -> c_long;

    #[cfg_attr(target_pointer_width = "64", link_name = "\u{1}?ASIOGetLatencies@@YAJPEAJ0@Z")]
    #[cfg_attr(target_pointer_width = "32", link_name = "\u{1}?ASIOGetLatencies@@YAJPAJ0@Z")]
    fn ASIOGetLatencies(input: *mut c_long, output: *mut c_long) -> c_long;

    #[link_name = "\u{1}?ASIOControlPanel@@YAJXZ"]
    fn ASIOControlPanel() -> c_long;
}

/// `(min, max, preferred, granularity)` of the loaded driver, or the ASIO error code.
pub(crate) fn get_buffer_size() -> Result<(i32, i32, i32, i32), i32> {
    let (mut min, mut max, mut preferred, mut granularity): (c_long, c_long, c_long, c_long) = (0, 0, 0, 0);
    // SAFETY: the four pointers are to live locals the call only writes through; with no driver
    // loaded the SDK returns ASE_NotPresent without touching them.
    let code = unsafe { ASIOGetBufferSize(&mut min, &mut max, &mut preferred, &mut granularity) };
    if code == ASE_OK { Ok((min, max, preferred, granularity)) } else { Err(code) }
}

/// `(input, output)` latency of the loaded driver in frames, or the ASIO error code.
pub(crate) fn get_latencies() -> Result<(i32, i32), i32> {
    let (mut input, mut output): (c_long, c_long) = (0, 0);
    // SAFETY: both pointers are to live locals the call only writes through; with no driver
    // loaded the SDK returns ASE_NotPresent without touching them.
    let code = unsafe { ASIOGetLatencies(&mut input, &mut output) };
    if code == ASE_OK { Ok((input, output)) } else { Err(code) }
}

/// Open the loaded driver's control panel, or the ASIO error code.
pub(crate) fn control_panel() -> Result<(), i32> {
    // SAFETY: takes no arguments; with no driver loaded the SDK returns ASE_NotPresent.
    let code = unsafe { ASIOControlPanel() };
    if code == ASE_OK { Ok(()) } else { Err(code) }
}
