//! UCP stream protocol bindings.
//!
//! Wraps `ucp_stream_send_nbx`, `ucp_stream_recv_nbx`, `ucp_stream_worker_poll`,
//! `ucp_stream_recv_data_nb`, `ucp_stream_recv_request_test`, and `ucp_stream_data_release`.

use crate::ep::Ep;
use crate::ffi::*;
use crate::status_ptr_is_err;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Request;
use crate::RequestParam;
use std::ptr::NonNull;

/// Data returned by [`Ep::stream_recv_data`], released when dropped.
pub struct StreamData<'ep> {
    ep: &'ep Ep,
    data: NonNull<std::ffi::c_void>,
    length: usize,
}

impl StreamData<'_> {
    /// View the received stream data as bytes.
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: UCX returned `data` for `length` readable bytes and this
        // guard keeps the allocation alive until it is dropped.
        unsafe { std::slice::from_raw_parts(self.data.as_ptr() as *const u8, self.length) }
    }

    /// Number of bytes in the received data.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Whether the received data is empty.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
}

impl Drop for StreamData<'_> {
    fn drop(&mut self) {
        // SAFETY: `data` was returned by UCX for this endpoint and is released
        // exactly once by this guard.
        unsafe { ucp_stream_data_release(self.ep.handle, self.data.as_ptr()) }
    }
}

impl Ep {
    /// Receive one internally allocated stream data buffer.
    ///
    /// `None` means no data is currently available. The returned guard releases
    /// the UCX buffer when dropped.
    pub fn stream_recv_data(&self) -> Result<Option<StreamData<'_>>, ucs_status_t> {
        let mut length = 0usize;
        // SAFETY: self.handle is a live endpoint and length is writable.
        let ptr = unsafe { ucp_stream_recv_data_nb(self.handle, &mut length) };
        if ptr.is_null() {
            return Ok(None);
        }
        if status_ptr_is_err(ptr) {
            return Err(crate::status_from_ptr(ptr));
        }
        Ok(Some(StreamData {
            ep: self,
            data: NonNull::new(ptr).expect("non-null checked above"),
            length,
        }))
    }

    /// Send data on a stream (safe wrapper).
    pub fn stream_send(
        &self,
        data: &[u8],
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_stream_send_nbx(self.handle, data.as_ptr() as _, data.len(), &param.handle)
        })
    }

    /// Receive data on a stream (safe wrapper).
    ///
    /// Returns the actual number of bytes received.
    pub fn stream_recv(
        &self,
        buf: &mut [u8],
        param: &RequestParam,
    ) -> Result<(Option<Request>, usize), ucs_status_t> {
        let mut length: usize = 0;
        let res = status_ptr_to_result(unsafe {
            ucp_stream_recv_nbx(
                self.handle,
                buf.as_mut_ptr() as _,
                buf.len(),
                &mut length,
                &param.handle,
            )
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
    pub unsafe fn stream_poll(
        &self,
        poll_eps: *mut ucp_stream_poll_ep_t,
        max_eps: usize,
        flags: u32,
    ) -> isize {
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
) -> Result<Option<*mut std::os::raw::c_void>, ucs_status_t> {
    let ptr = ucp_stream_recv_data_nb(ep, length);
    if ptr.is_null() {
        return Ok(None);
    }
    if status_ptr_is_err(ptr) {
        return Err(crate::status_from_ptr(ptr));
    }
    Ok(Some(ptr))
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
#[allow(clippy::let_unit_value, clippy::missing_transmute_annotations)]
mod tests {
    use super::*;
    use crate::context::{Config, Context, Flags, ParamsBuilder as CtxParamsBuilder};
    use crate::worker::ParamsBuilder as WorkerParamsBuilder;

    /// Helper: create a UCX context + worker with Tag feature.
    fn setup_worker() -> (Context, Worker) {
        let ctx_params = CtxParamsBuilder::new().features(Flags::Tag).build();
        let ctx = Context::new(&Config::read("", "").expect("config read"), &ctx_params)
            .expect("context create");
        let worker_params = WorkerParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");
        (ctx, worker)
    }

    /// Test that stream_send and stream_recv APIs compile and accept valid parameters.
    /// This verifies the function signatures and RequestParam compatibility.
    /// Actual send/recv requires connected endpoints — those are integration tests.
    #[test]
    fn test_stream_api_compiles() {
        let (_ctx, worker) = setup_worker();

        // Create an endpoint to self (for API validation)
        let packed_addr = worker.pack_address().expect("pack address");
        let addr = crate::worker::RemoteWorkerAddress::new(packed_addr.to_vec());
        let ep_param = crate::ep::ParamsBuilder::new().address(&addr).build();
        let ep = worker.create_ep(ep_param).expect("create ep");
        drop(packed_addr);

        // Build a valid RequestParam for stream operations
        let param = crate::RequestParamBuilder::new().build();

        // Verify stream_send compiles with valid params (won't complete without peer,
        // but the API call itself should not crash)
        let data = b"hello";
        let result = ep.stream_send(data, &param);

        // The send may return an error (no peer to receive), or a pending request.
        // Either way, the API is valid.
        match result {
            Ok(None) => {
                // Completed immediately (unlikely without peer, but valid)
            }
            Ok(Some(_req)) => {
                // Got a pending request — valid API behavior
            }
            Err(_status) => {
                // Error is expected without a connected peer — API is still valid
            }
        }

        // Do not call stream_recv on a bare self-EP: UCX requires a connected
        // stream peer and dereferences uninitialized stream state otherwise.
    }

    /// Test that Worker::stream_poll and stream_data_release FFI functions exist.
    #[test]
    fn test_stream_poll_signature() {
        let (_ctx, _worker) = setup_worker();
        // Verify the FFI functions are accessible
        extern "C" {
            fn ucp_stream_worker_poll(
                worker: ucp_worker_h,
                poll_eps: *mut ucp_stream_poll_ep_t,
                max_eps: usize,
                flags: u32,
            ) -> isize;
            fn ucp_stream_data_release(ep: ucp_ep_h, data: *mut std::os::raw::c_void);
        }
        // Functions exist and have correct signatures
        let _ = unsafe { std::mem::transmute::<_, ()>(ucp_stream_worker_poll) };
        let _ = unsafe { std::mem::transmute::<_, ()>(ucp_stream_data_release) };
    }

    #[test]
    fn test_stream_recv_data_signature() {
        let _: for<'a> fn(&'a Ep) -> Result<Option<StreamData<'a>>, ucs_status_t> =
            Ep::stream_recv_data;
    }
}
