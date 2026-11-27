//! UCP stream protocol bindings.
//!
//! Wraps `ucp_stream_send_nbx`, `ucp_stream_recv_nbx`, `ucp_stream_worker_poll`,
//! `ucp_stream_recv_data_nb`, `ucp_stream_recv_request_test`, and `ucp_stream_data_release`.

use crate::ep::{Ep, EpHandle};

use crate::completion::{owned_op_param, owned_recv_param, status_ptr_with_data, RecvInfo};
use crate::ffi::*;
use crate::status_ptr_is_err;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Pending;
use crate::Request;
use crate::RequestParam;
use crate::Status;
use crate::{Completion, OwnedBuf, OwnedBufMut, OwningRequest, StreamRecvRequest};
use std::ptr::NonNull;

/// One endpoint reported ready by [`Worker::stream_poll`].
///
/// The endpoint handle is borrowed from UCX. It is valid until the next stream
/// poll or worker progress operation, according to the UCX stream poll contract;
/// this type never takes ownership of it and therefore cannot close the endpoint.
#[derive(Debug, Copy, Clone)]
pub struct StreamPollEvent {
    ep: EpHandle,
    user_data: *mut std::ffi::c_void,
    flags: u32,
}

impl StreamPollEvent {
    /// Return the borrowed UCX endpoint handle reported by the poll.
    pub fn ep_handle(&self) -> EpHandle {
        self.ep
    }

    /// Return the user data associated with the endpoint.
    pub fn user_data(&self) -> *mut std::ffi::c_void {
        self.user_data
    }

    /// Return the UCX stream poll flags for this endpoint.
    pub fn flags(&self) -> u32 {
        self.flags
    }
}

/// One entry of the array filled by the deprecated [`stream_worker_poll`].
///
/// `#[repr(transparent)]` over UCX's poll-entry struct, so an array of entries can be handed to
/// UCX directly. Create entries with [`StreamPollEp::default`]. Prefer [`Worker::stream_poll`].
#[repr(transparent)]
#[derive(Clone)]
pub struct StreamPollEp(ucp_stream_poll_ep_t);

// `stream_worker_poll` passes `*mut StreamPollEp` to UCX as `*mut ucp_stream_poll_ep_t`.
const _: () = {
    assert!(std::mem::size_of::<StreamPollEp>() == std::mem::size_of::<ucp_stream_poll_ep_t>());
    assert!(std::mem::align_of::<StreamPollEp>() == std::mem::align_of::<ucp_stream_poll_ep_t>());
};

impl Default for StreamPollEp {
    fn default() -> Self {
        // SAFETY: the poll-entry struct is plain C data (pointers, integers and reserved
        // bytes), for which all-zero is valid; `Worker::stream_poll` initializes it the same way.
        StreamPollEp(unsafe { std::mem::zeroed() })
    }
}

impl StreamPollEp {
    /// Return the borrowed UCX endpoint handle UCX wrote into this entry.
    pub fn ep_handle(&self) -> EpHandle {
        EpHandle(self.0.ep)
    }

    /// Return the user data associated with the endpoint.
    pub fn user_data(&self) -> *mut std::ffi::c_void {
        self.0.user_data
    }

    /// Return the UCX stream poll flags for this endpoint.
    pub fn flags(&self) -> u32 {
        self.0.flags
    }
}

impl std::fmt::Debug for StreamPollEp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StreamPollEp")
            .field("ep", &self.0.ep)
            .field("user_data", &self.0.user_data)
            .field("flags", &self.0.flags)
            .finish()
    }
}

impl Request {
    /// Test completion of a stream receive request and return its byte length.
    ///
    /// Returns `Ok(length)` only when UCX reports `UCS_OK` and the receive is
    /// complete. A pending receive returns `Err(UCS_INPROGRESS)`; any other
    /// `Err(status)` is a UCX failure status. The output length is only valid
    /// for `Ok` results, as specified by UCX. The request remains owned by this
    /// value in all cases. This method must only be used with a request returned
    /// by [`Ep::stream_recv`].
    pub fn stream_recv_test(&self) -> Result<usize, Status> {
        let request = self.handle;
        let mut length = 0usize;
        status_to_result(unsafe { ucp_stream_recv_request_test(request.as_ptr(), &mut length) })
            .map(|()| length)
    }
}

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

impl crate::completion::sealed::Sealed for usize {}

impl RecvInfo for usize {
    fn read(request: &Request) -> Self {
        let mut length = 0usize;
        // SAFETY: `request` is a live, completed stream receive request (the
        // caller reads the length before freeing it).
        let status = unsafe { ucp_stream_recv_request_test(request.as_ptr(), &mut length) };
        assert_eq!(status, ucs_status_t::UCS_OK, "completed stream receive");
        length
    }
}

impl Ep {
    /// Receive one internally allocated stream data buffer.
    ///
    /// `None` means no data is currently available. The returned guard releases
    /// the UCX buffer when dropped.
    pub fn stream_recv_data(&self) -> Result<Option<StreamData<'_>>, Status> {
        let mut length = 0usize;
        // SAFETY: self.handle is a live endpoint and length is writable.
        let ptr = unsafe { ucp_stream_recv_data_nb(self.handle, &mut length) };
        if ptr.is_null() {
            return Ok(None);
        }
        if status_ptr_is_err(ptr) {
            return Err(crate::Status::from(crate::status_from_ptr(ptr)));
        }
        Ok(Some(StreamData {
            ep: self,
            data: NonNull::new(ptr).expect("non-null checked above"),
            length,
        }))
    }

    /// Send `data` on a stream, taking ownership of it.
    ///
    /// Returns [`Completion::Immediate`] with the buffer, or
    /// [`Completion::Pending`] with an [`OwningRequest`] that hands it back
    /// from [`OwningRequest::try_complete`]. `worker` must be the worker this
    /// endpoint was created on. `param` contributes flags, callback, and user
    /// data; the datatype (contiguous bytes), reply buffer, receive info,
    /// memory handle, memory type, and request allocation fields are
    /// controlled here.
    pub fn stream_send<'w, D: OwnedBuf>(
        &self,
        worker: &'w Worker,
        data: D,
        param: &RequestParam,
    ) -> Result<Completion<D, OwningRequest<'w, D>>, Status> {
        debug_assert!(
            std::sync::Arc::ptr_eq(&self.worker_alive, &worker.alive),
            "stream_send: the passed worker is not the endpoint's worker"
        );
        let raw = owned_op_param(param);
        // SAFETY: the request (if any) reads only `data`'s stable storage
        // (`OwnedBuf`), which the returned value owns until UCX is done.
        unsafe {
            let ptr = ucp_stream_send_nbx(self.handle, data.as_ptr() as _, data.len(), &raw);
            status_ptr_with_data(ptr, data, worker)
        }
    }

    /// Receive stream data into `buffer`, taking ownership of it.
    ///
    /// Completes with the buffer and the number of bytes received, either
    /// immediately or through [`StreamRecvRequest::try_complete`]. Without
    /// `UCP_STREAM_RECV_FLAG_WAITALL` in `param` flags, UCX may complete with
    /// fewer bytes than `buffer.len()`. `param` contributes flags only; a
    /// callback or user data set on it is ignored (see [`Worker::tag_recv`]).
    ///
    /// A pending receive cannot outlive its worker:
    ///
    /// ```compile_fail
    /// # use ucx_sys::{ep::Ep, worker::Worker, RequestParam, StreamRecvRequest, Completion};
    /// fn escape(ep: &Ep, param: &RequestParam, worker: Worker) -> Option<StreamRecvRequest<'static, Vec<u8>>> {
    ///     let pending = ep.stream_recv(&worker, vec![0u8; 64], param).unwrap().into_pending();
    ///     drop(worker);
    ///     pending
    /// }
    /// ```
    pub fn stream_recv<'w, D: OwnedBufMut>(
        &self,
        worker: &'w Worker,
        mut buffer: D,
        param: &RequestParam,
    ) -> Result<Completion<(D, usize), StreamRecvRequest<'w, D>>, Status> {
        debug_assert!(
            std::sync::Arc::ptr_eq(&self.worker_alive, &worker.alive),
            "stream_recv: the passed worker is not the endpoint's worker"
        );
        let raw = owned_recv_param(param);
        let mut length: usize = 0;
        // SAFETY: the request (if any) writes only into `buffer`'s stable
        // storage, which the returned value owns until UCX is done; `length`
        // is written only if the receive completes inside this call.
        let completion = unsafe {
            let ptr = ucp_stream_recv_nbx(
                self.handle,
                buffer.as_mut_ptr() as _,
                buffer.len(),
                &mut length,
                &raw,
            );
            status_ptr_with_data(ptr, buffer, worker)?
        };
        Ok(match completion {
            Completion::Immediate(buffer) => Completion::Immediate((buffer, length)),
            Completion::Pending(request) => Completion::Pending(StreamRecvRequest::new(request)),
        })
    }
}

impl Worker {
    /// Poll for stream data on multiple endpoints.
    ///
    /// Each returned endpoint handle is borrowed from UCX and must not be
    /// closed by the caller. UCX may invalidate the handle after the next
    /// worker progress or stream poll operation.
    pub fn stream_poll(&self, max_eps: usize) -> Result<Vec<StreamPollEvent>, Status> {
        let mut poll_eps = vec![unsafe { std::mem::zeroed() }; max_eps];
        let count =
            unsafe { ucp_stream_worker_poll(self.handle, poll_eps.as_mut_ptr(), max_eps, 0) };
        if count < 0 {
            return Err(crate::Status::from(crate::status_from_ptr(
                count as isize as usize as ucs_status_ptr_t,
            )));
        }
        let count = count as usize;
        debug_assert!(count <= max_eps);
        poll_eps.truncate(count.min(max_eps));
        Ok(poll_eps
            .into_iter()
            .map(|entry| StreamPollEvent {
                ep: EpHandle(entry.ep),
                user_data: entry.user_data,
                flags: entry.flags,
            })
            .collect())
    }
}

/// Send data on a stream.
///
/// # Safety
/// Caller must ensure `buffer` is valid for `count` bytes.
#[deprecated(since = "0.1.0", note = "Use Ep::stream_send() instead")]
pub unsafe fn stream_send_nbx(
    ep: &crate::ep::Ep,
    buffer: *const std::os::raw::c_void,
    count: usize,
    param: &RequestParam,
) -> Result<Pending, Status> {
    status_ptr_to_result(ucp_stream_send_nbx(ep.handle, buffer, count, &param.handle))
}

/// Receive data on a stream.
///
/// Returns the actual number of bytes received in `length`.
///
/// # Safety
/// Caller must ensure `buffer` has space for `count` bytes.
#[deprecated(since = "0.1.0", note = "Use Ep::stream_recv() instead")]
pub unsafe fn stream_recv_nbx(
    ep: &crate::ep::Ep,
    buffer: *mut std::os::raw::c_void,
    count: usize,
    length: *mut usize,
    param: &RequestParam,
) -> Result<Pending, Status> {
    status_ptr_to_result(ucp_stream_recv_nbx(
        ep.handle,
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
    worker: &crate::worker::Worker,
    poll_eps: *mut StreamPollEp,
    max_eps: usize,
    flags: u32,
) -> isize {
    // `StreamPollEp` is `#[repr(transparent)]` over `ucp_stream_poll_ep_t`, so the caller's
    // array has exactly UCX's layout.
    ucp_stream_worker_poll(
        worker.handle,
        poll_eps.cast::<ucp_stream_poll_ep_t>(),
        max_eps,
        flags,
    )
}

/// Receive stream data with automatic buffer allocation.
///
/// Returns the allocated data pointer and its length.
/// The caller must eventually call `stream_data_release` on the returned data.
///
/// # Safety
/// The returned data pointer must be released with `stream_data_release`.
pub unsafe fn stream_recv_data_nb(
    ep: &crate::ep::Ep,
    length: *mut usize,
) -> Result<Option<*mut std::os::raw::c_void>, Status> {
    let ptr = ucp_stream_recv_data_nb(ep.handle, length);
    if ptr.is_null() {
        return Ok(None);
    }
    if status_ptr_is_err(ptr) {
        return Err(crate::Status::from(crate::status_from_ptr(ptr)));
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
) -> Result<(), Status> {
    status_to_result(ucp_stream_recv_request_test(request, length))
}

/// Release stream data obtained from `stream_recv_data_nb`.
///
/// # Safety
/// Caller must ensure `data` was obtained from `stream_recv_data_nb`.
pub unsafe fn stream_data_release(ep: EpHandle, data: *mut std::os::raw::c_void) {
    ucp_stream_data_release(ep.0, data);
}

#[cfg(test)]
#[allow(
    clippy::let_unit_value,
    clippy::missing_transmute_annotations,
    clippy::type_complexity
)]
mod tests {
    use super::*;
    use crate::context::{Config, Context, Flags, ParamsBuilder as CtxParamsBuilder};
    use crate::worker::ParamsBuilder as WorkerParamsBuilder;

    /// Helper: create a UCX context + worker with the Tag and Stream features.
    fn setup_worker() -> (Context, Worker) {
        let ctx_params = CtxParamsBuilder::new()
            .features(Flags::Tag | Flags::Stream)
            .mt_workers_shared(1)
            .build();
        let mut ctx = Context::new(&Config::read("", "").expect("config read"), &ctx_params)
            .expect("context create");
        let worker_params = WorkerParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");
        (ctx, worker)
    }

    /// Test that stream_send and stream_recv APIs compile and accept valid parameters.
    /// This verifies the function signatures and RequestParam compatibility.
    /// Actual send/recv requires connected endpoints — those are integration tests.
    #[test]
    #[allow(clippy::type_complexity)]
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
        let result = ep.stream_send(&worker, b"hello".to_vec(), &param);

        // The send may complete, stay pending, or fail without a stream peer;
        // any outcome is valid API behavior here. A pending request is
        // dropped, which cancels it through the fail-safe.
        match result {
            Ok(Completion::Immediate(data)) => assert_eq!(data, b"hello"),
            Ok(Completion::Pending(_request)) => {}
            Err(_status) => {}
        }

        // Do not call stream_recv on a bare self-EP: UCX requires a connected
        // stream peer and dereferences uninitialized stream state otherwise.
        let _: for<'w> fn(
            &Ep,
            &'w Worker,
            Vec<u8>,
            &RequestParam,
        ) -> Result<
            Completion<(Vec<u8>, usize), StreamRecvRequest<'w, Vec<u8>>>,
            Status,
        > = Ep::stream_recv;
    }

    /// Test that Worker::stream_poll and stream_data_release FFI functions exist.
    #[test]
    fn test_stream_poll_signature() {
        let (_ctx, _worker) = setup_worker();
        let _: fn(&Worker, usize) -> Result<Vec<StreamPollEvent>, Status> = Worker::stream_poll;
        let _: fn(&Request) -> Result<usize, Status> = Request::stream_recv_test;
        let _: unsafe extern "C" fn(ucp_ep_h, *mut std::os::raw::c_void) = ucp_stream_data_release;
    }

    #[test]
    fn test_stream_poll_empty_worker() {
        let (_ctx, worker) = setup_worker();
        assert!(worker
            .stream_poll(0)
            .expect("poll with zero capacity")
            .is_empty());
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn test_stream_owned_signatures() {
        let _: fn(&Request) -> Result<usize, Status> = Request::stream_recv_test;
        let _: for<'w> fn(
            &Ep,
            &'w Worker,
            Box<[u8]>,
            &RequestParam,
        )
            -> Result<Completion<Box<[u8]>, OwningRequest<'w, Box<[u8]>>>, Status> =
            Ep::stream_send;
        fn complete<'w>(
            request: StreamRecvRequest<'w, Vec<u8>>,
        ) -> Result<Result<(Vec<u8>, usize), crate::Failed<Vec<u8>>>, StreamRecvRequest<'w, Vec<u8>>>
        {
            request.try_complete()
        }
        let _ = complete;
    }

    #[test]
    fn test_stream_recv_data_signature() {
        let _: for<'a> fn(&'a Ep) -> Result<Option<StreamData<'a>>, Status> = Ep::stream_recv_data;
    }

    #[test]
    #[allow(deprecated)]
    fn stream_worker_poll_fills_native_entries() {
        let (_ctx, worker) = setup_worker();
        let mut entries = vec![StreamPollEp::default(); 4];
        let count = unsafe { stream_worker_poll(&worker, entries.as_mut_ptr(), entries.len(), 0) };
        assert_eq!(count, 0);
        assert!(entries
            .iter()
            .all(|entry| entry.flags() == 0 && entry.user_data().is_null()));
    }
}
