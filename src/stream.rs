//! UCP stream protocol bindings.
//!
//! Wraps `ucp_stream_send_nbx`, `ucp_stream_recv_nbx`, `ucp_stream_worker_poll`,
//! `ucp_stream_recv_data_nb`, `ucp_stream_recv_request_test`, and `ucp_stream_data_release`.

use crate::ep::Ep;
use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Request;
use crate::RequestParam;

impl Ep {
    /// Send data on a stream (safe wrapper).
    pub fn stream_send(&self, data: &[u8], param: &RequestParam) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_stream_send_nbx(self.handle, data.as_ptr() as _, data.len(), &param.handle)
        })
    }

    /// Receive data on a stream (safe wrapper).
    ///
    /// Returns the actual number of bytes received.
    pub fn stream_recv(&self, buf: &mut [u8], param: &RequestParam) -> Result<(Option<Request>, usize), ucs_status_t> {
        let mut length: usize = 0;
        let res = status_ptr_to_result(unsafe {
            ucp_stream_recv_nbx(self.handle, buf.as_mut_ptr() as _, buf.len(), &mut length, &param.handle)
        });
        res.map(|r| (r, length))
    }
}

impl Worker {
    /// Poll for stream data on multiple endpoints.
    ///
    /// Returns the number of endpoints with data available (negative value on error).
    ///
    /// # Safety
    /// Caller must ensure `poll_eps` is valid for `max_eps` elements.
    pub unsafe fn stream_poll(&self, poll_eps: *mut ucp_stream_poll_ep_t, max_eps: usize, flags: u32) -> isize {
        ucp_stream_worker_poll(self.handle, poll_eps, max_eps, flags)
    }
}

/// Send data on a stream.
///
/// # Safety
/// Caller must ensure `buffer` is valid for `count` bytes.
#[deprecated(since = "0.1.0", note = "Use Ep::stream_send() instead")]
pub unsafe fn stream_send_nbx(
    ep: ucp_ep_h,
    buffer: *const std::os::raw::c_void,
    count: usize,
    param: &RequestParam,
) -> Result<Option<Request>, ucs_status_t> {
    status_ptr_to_result(ucp_stream_send_nbx(ep, buffer, count, &param.handle))
}

/// Receive data on a stream.
///
/// Returns the actual number of bytes received in `length`.
///
/// # Safety
/// Caller must ensure `buffer` has space for `count` bytes.
#[deprecated(since = "0.1.0", note = "Use Ep::stream_recv() instead")]
pub unsafe fn stream_recv_nbx(
    ep: ucp_ep_h,
    buffer: *mut std::os::raw::c_void,
    count: usize,
    length: *mut usize,
    param: &RequestParam,
) -> Result<Option<Request>, ucs_status_t> {
    status_ptr_to_result(ucp_stream_recv_nbx(
        ep,
        buffer,
        count,
        length,
        &param.handle,
    ))
}

/// Poll for stream data on multiple endpoints.
///
/// Returns the number of endpoints with data available (negative value on error).
///
/// # Safety
/// Caller must ensure `poll_eps` is valid for `max_eps` elements.
#[deprecated(since = "0.1.0", note = "Use Worker::stream_poll() instead")]
pub unsafe fn stream_worker_poll(
    worker: ucp_worker_h,
    poll_eps: *mut ucp_stream_poll_ep_t,
    max_eps: usize,
    flags: u32,
) -> isize {
    ucp_stream_worker_poll(worker, poll_eps, max_eps, flags)
}

/// Receive stream data with automatic buffer allocation.
///
/// Returns the allocated data pointer and its length.
/// The caller must eventually call `stream_data_release` on the returned data.
///
/// # Safety
/// The returned data pointer must be released with `stream_data_release`.
pub unsafe fn stream_recv_data_nb(
    ep: ucp_ep_h,
    length: *mut usize,
) -> Result<Option<Request>, ucs_status_t> {
    status_ptr_to_result(ucp_stream_recv_data_nb(ep, length))
}

/// Test a stream receive request and get the data length.
///
/// # Safety
/// Caller must ensure `request` is a valid stream receive request.
pub unsafe fn stream_recv_request_test(
    request: *mut std::os::raw::c_void,
    length: *mut usize,
) -> Result<(), ucs_status_t> {
    status_to_result(ucp_stream_recv_request_test(request, length))
}

/// Release stream data obtained from `stream_recv_data_nb`.
///
/// # Safety
/// Caller must ensure `data` was obtained from `stream_recv_data_nb`.
pub unsafe fn stream_data_release(ep: ucp_ep_h, data: *mut std::os::raw::c_void) {
    ucp_stream_data_release(ep, data);
}

#[cfg(test)]
mod tests {
    // Stream tests require connected endpoints — marked #[ignore] for now
    #[test]
    #[ignore]
    fn test_stream_send_recv() {
        // Requires two connected endpoints
        unimplemented!("Requires connected endpoints");
    }
}
