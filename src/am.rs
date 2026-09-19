use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::worker::Worker;
use crate::Request;
use crate::RequestParam;
use bitflags::bitflags;
use std::sync::{Arc, Mutex};

pub type AmRecvCb = unsafe extern "C" fn(
    arg: *mut ::std::os::raw::c_void,
    header: *const ::std::os::raw::c_void,
    header_length: usize,
    data: *mut ::std::os::raw::c_void,
    length: usize,
    param: *const ucp_am_recv_param_t,
) -> ucs_status_t;

type AmCallback = Box<dyn FnMut(&[u8], &[u8]) -> ucs_status_t + Send + 'static>;

/// The Rust state retained by [`Worker::am_register_handler`].
pub struct AmHandler {
    inner: Mutex<AmCallback>,
}

impl std::fmt::Debug for AmHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AmHandler").finish_non_exhaustive()
    }
}

unsafe extern "C" fn am_trampoline(
    arg: *mut std::os::raw::c_void,
    header: *const std::os::raw::c_void,
    header_length: usize,
    data: *mut std::os::raw::c_void,
    length: usize,
    _param: *const ucp_am_recv_param_t,
) -> ucs_status_t {
    // SAFETY: `arg` is an Arc<AmHandler> pointer installed by
    // am_register_handler and retained by Worker until after UCX destroys the
    // worker. UCX owns the callback buffers for this invocation; null pointers
    // are permitted for zero-length messages.
    let handler = unsafe { &*(arg as *const AmHandler) };
    let header = if header.is_null() && header_length == 0 {
        &[]
    } else if header.is_null() {
        return ucs_status_t::UCS_ERR_INVALID_PARAM;
    } else {
        unsafe { std::slice::from_raw_parts(header as *const u8, header_length) }
    };
    let data = if data.is_null() && length == 0 {
        &[]
    } else if data.is_null() {
        return ucs_status_t::UCS_ERR_INVALID_PARAM;
    } else {
        unsafe { std::slice::from_raw_parts(data as *const u8, length) }
    };
    let mut callback = match handler.inner.lock() {
        Ok(callback) => callback,
        // A panic poisons the mutex. Do not invoke a possibly inconsistent
        // handler again; report the callback failure to UCX instead.
        Err(_) => return ucs_status_t::UCS_ERR_IO_ERROR,
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        callback(header, data)
    })) {
        Ok(status) => status,
        Err(_) => ucs_status_t::UCS_ERR_IO_ERROR,
    }
}

