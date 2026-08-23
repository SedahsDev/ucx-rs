//! Rust bindings for UCX's UCP API.
//!
//! # Threading model
//!
//! The default policy is a single-threaded progress loop. `Context`, `Worker`,
//! and `Ep` are intentionally `!Send` and `!Sync` today (as are the associated
//! handle wrappers such as `MemHandle` and `RemoteKey`). This is the current
//! safe Rust API policy, not a promise that UCX can never use these objects from
//! multiple threads. In particular, this crate does not provide unsafe `Send`
//! or `Sync` implementations, and making the handles transferable in UCX's
//! `UCS_THREAD_MODE_MULTI` mode is future work.
//!
//! `Worker::ParamsBuilder::thread_mode` selects the UCX contract for calls on
//! that worker:
//!
//! * `UCS_THREAD_MODE_SINGLE` permits calls from one thread only.
//! * `UCS_THREAD_MODE_SERIALIZED` permits multiple callers, but the application
//!   must serialize UCX calls.
//! * `UCS_THREAD_MODE_MULTI` permits concurrent UCX calls where UCX documents
//!   them as thread-safe, but it does not make these Rust wrapper values
//!   transferable or remove application-level protocol synchronization.
//!
//! Regardless of the selected mode, `Worker::progress()` must be externally
//! serialized per worker: do not call it concurrently on the same worker.
//! Operations and their borrowed buffers must also remain valid until UCX says
//! they have completed. Always drop/close endpoints before their `Worker`.
//! `Ep::Drop` checks the runtime `worker_alive` guard and skips a late cleanup as
//! a safety net; that guard is not a license to violate the required drop order.
//!
//! An `MtWorker` abstraction may be added in a future release; it is not
//! provided by this crate currently.
#![allow(unused_imports)]

mod ffi;
mod threading_assert;
use crate::ffi::*;

pub use ffi::{ucp_am_recv_param_t, ucs_status_t};

pub mod am;
pub mod config;
pub mod context;
pub mod dt;
pub mod ep;
pub mod listener;
pub mod memh;
pub mod rma;
pub mod stream;
pub mod tag;
pub mod version;
pub mod worker;

use std::ffi::CString;
use std::ptr::NonNull;

/// UCX non-blocking request handle (`ucs_status_ptr_t` that is a real request).
///
/// Dropping a live request calls `ucp_request_free`. After cancel/free, the
/// internal handle is cleared so double-free cannot occur.
///
/// # Thread safety
///
/// Not `Send`/`Sync`. Pair with a worker in the same thread (see
/// `docs/FFI-CONVENTIONS.md` in the monorepo docs tree).
pub struct Request {
    pub(crate) handle: Option<NonNull<::std::os::raw::c_void>>,
}

/// The result of testing a non-blocking request.
#[derive(Debug, Clone)]
pub struct RequestState {
    /// `Ok(())` means the request completed successfully; `Err` contains the
    /// UCX status returned by `ucp_request_test` (including `UCS_INPROGRESS`).
    pub status: Result<(), ucs_status_t>,
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            // SAFETY: handle came from UCX as a request pointer and has not been freed yet.
            unsafe { ucp_request_free(h.as_ptr()) };
        }
    }
}

impl Request {
    /// Wrap a non-null request pointer. Returns `None` if `request_handle` is null.
    #[inline]
    pub fn new(request_handle: *mut std::os::raw::c_void) -> Option<Request> {
        NonNull::new(request_handle).map(|h| Request { handle: Some(h) })
    }

    /// Create a Request from a raw pointer.
    ///
    /// # Safety
    /// Caller must ensure `ptr` is a valid UCX request pointer (not a status code,
    /// not null) obtained from a UCX API that returns `ucs_status_ptr_t` as a request.
    #[inline]
    pub unsafe fn from_raw(ptr: *mut std::os::raw::c_void) -> Request {
        debug_assert!(!ptr.is_null(), "Request::from_raw with null");
        Request {
            handle: NonNull::new(ptr),
        }
    }

    /// Raw pointer for FFI, or null if already freed/cancelled.
    #[inline]
    pub fn as_ptr(&self) -> *mut std::os::raw::c_void {
        self.handle
            .map(|h| h.as_ptr())
            .unwrap_or(std::ptr::null_mut())
    }

    /// True if this still owns a live UCX request.
    #[inline]
    pub fn is_live(&self) -> bool {
        self.handle.is_some()
    }

    /// Check an outstanding request.
    ///
    /// Returns `Ok(true)` completed, `Ok(false)` in progress, `Err` on failure.
    /// Returns `Ok(true)` if the request was already freed/cancelled.
    #[inline]
    pub fn check_finished(&self) -> Result<bool, ucs_status_t> {
        let Some(h) = self.handle else {
            return Ok(true);
        };
        let status = unsafe { ucp_request_check_status(h.as_ptr()) };
        let status_ptr = status as isize as usize as ucs_status_ptr_t;
        if status_ptr_is_err(status_ptr) {
            return Err(status_from_ptr(status_ptr));
        }
        Ok(status == ucs_status_t::UCS_OK)
    }

    /// Check completion status without consuming or otherwise changing the
    /// request. This is the lowest-cost completion query and does not fill any
    /// receive metadata. Unlike [`Request::test`], it is suitable when only a
    /// boolean completion indication is needed; unlike [`Request::check_finished`],
    /// it does not decode a UCX status or report an error.
    ///
    /// An inert request (one already freed or cancelled) is complete.
    #[inline]
    pub fn is_completed(&self) -> bool {
        let Some(h) = self.handle else {
            return true;
        };
        unsafe { ucp_request_is_completed(h.as_ptr()) != 0 }
    }

    /// Test a request and return the status reported by UCX.
    ///
    /// This uses the legacy `ucp_request_test` API, which fills tag-receive
    /// metadata in an out-parameter. The metadata is intentionally discarded:
    /// this method exposes only the status, and callers needing receive
    /// information should use a more specific UCX API. Unlike
    /// [`Request::check_finished`], this invokes the richer test routine and
    /// supplies its out-parameter; unlike [`Request::is_completed`], it
    /// returns the actual UCX status (including `UCS_INPROGRESS`). The request
    /// remains owned and must still be freed or released after completion.
    ///
    /// An inert request is represented as a successfully completed request.
    #[inline]
    pub fn test(&self) -> RequestState {
        let Some(h) = self.handle else {
            return RequestState { status: Ok(()) };
        };
        let mut info: ucp_tag_recv_info_t = unsafe { std::mem::zeroed() };
        let status = unsafe { ucp_request_test(h.as_ptr(), &mut info) };
        RequestState {
            status: status_to_result(status),
        }
    }

    /// Release this request through UCX's deprecated `ucp_request_release`.
    ///
    /// This consumes the wrapper and releases the request memory regardless of
    /// its current state. The operation is not cancelled: it continues to
    /// progress internally, and its completion callback may still fire. This
    /// differs from [`Request::free`] (and `Drop`), which uses
    /// `ucp_request_free` to release the request and disable further callback
    /// invocation.
    ///
    /// # Warning
    ///
    /// After `release()`, any `user_data` and callback closures that touch Rust
    /// resources must remain valid until UCX has completed the operation and no
    /// longer invokes the callback. The caller must not free resources on which
    /// the callback depends.
    #[inline]
    pub fn release(mut self) {
        if let Some(h) = self.handle.take() {
            unsafe { ucp_request_release(h.as_ptr()) };
        }
    }

    /// Cancel this request on `worker`, then free it (handle becomes inert).
    ///
    /// After cancel, further `check_finished` returns `Ok(true)`.
    pub fn cancel(&mut self, worker: &worker::Worker) {
        if let Some(h) = self.handle.take() {
            // SAFETY: worker and request handles are valid UCX objects.
            unsafe {
                ucp_request_cancel(worker.handle, h.as_ptr());
                ucp_request_free(h.as_ptr());
            }
        }
    }

    /// Explicit free without drop glue (handle becomes inert). Prefer Drop normally.
    pub fn free(mut self) {
        let _ = self
            .handle
            .take()
            .map(|h| unsafe { ucp_request_free(h.as_ptr()) });
    }
}

// UCX uses `ucs_status_ptr_t` for nonblocking operations: the result is either
// an immediate status, a request pointer, or an error. This helper maps those
// outcomes to `Result<Option<Request>, ucs_status_t>`. A status-pointer API
// never returns `UCS_INPROGRESS`; an incomplete operation returns a request
// pointer instead, while callers observe that status through plain
// `ucs_status_t` APIs such as `ucp_request_check_status`.

/// Translates a UCX status pointer into an immediate result, request, or error.
///
/// # Invariant
///
/// The `nbx` family and the compat close/flush/modify/disconnect
/// status-pointer APIs routed through this helper never return
/// `UCS_INPROGRESS`: when an operation does not complete immediately, UCP
/// allocates a request and returns a pointer to that request instead of the
/// status code (`UCS_INPROGRESS = 1` is a small integer that can never be a
/// valid pointer; UCX's own `ucp_request_complete` asserts completed requests
/// do not carry it). Some legacy callback-bearing `_nb` APIs, such as
/// `ucp_tag_send_nb`, can return `UCS_INPROGRESS` and must never be passed to
/// this helper. Callers observe `UCS_INPROGRESS` only through plain
/// `ucs_status_t` APIs such as `ucp_request_check_status`
/// (`Request::check_finished`). This classification MUST NOT change if UCX
/// is upgraded — re-verify against
/// `src/ucp/core/ucp_request.inl` and `src/ucs/type/status.h` in the new UCX
/// version first.
#[inline]
pub fn status_ptr_to_result(ptr: ucs_status_ptr_t) -> Result<Option<Request>, ucs_status_t> {
    if status_ptr_is_err(ptr) {
        return Err(status_from_ptr(ptr));
    }
    // Invariant guard: see doc note above. UCS_INPROGRESS must never arrive
    // through a status_ptr; treat any future violation loudly in debug builds.
    debug_assert!(
        ptr as usize != ucs_status_t::UCS_INPROGRESS as usize,
        "UCX returned UCS_INPROGRESS through a ucs_status_ptr_t API - \
         violates the status_ptr contract, see status_ptr_to_result docs"
    );
    // UCX uses small non-negative integers for immediate statuses; only larger
    // values can be real request addresses.
    if ptr as usize <= ucs_status_t::UCS_INPROGRESS as usize {
        return Ok(None);
    }
    Ok(Request::new(ptr))
}

#[inline]
pub fn status_to_result(status: ucs_status_t) -> Result<(), ucs_status_t> {
    // Per ucs/type/status.h, UCS_ERR_* values are negative and success values are non-negative.
    if status_value_is_err(status) {
        return Err(status);
    }
    Ok(())
}

/// Classify and decode UCX's tagged status pointer representation.
///
/// SAFETY/correctness: ucs/type/status.h defines `UCS_PTR_IS_ERR(ptr)` as
/// `((uintptr_t)ptr >= (uintptr_t)UCS_ERR_LAST)` and `UCS_PTR_STATUS(ptr)` as
/// `(ucs_status_t)(intptr_t)ptr`. Casting through `isize` preserves the signed
/// status value and avoids truncating an arbitrary pointer to `i8`.
#[inline]
pub(crate) fn status_ptr_is_err(ptr: ucs_status_ptr_t) -> bool {
    ptr as usize >= (ucs_status_t::UCS_ERR_LAST as isize) as usize
}

#[inline]
fn status_value_is_err(status: ucs_status_t) -> bool {
    (status as i8) < 0
}

#[inline]
fn status_from_ptr(ptr: ucs_status_ptr_t) -> ucs_status_t {
    let status = ptr as isize as i32;
    match status {
        -1 => ucs_status_t::UCS_ERR_NO_MESSAGE,
        -2 => ucs_status_t::UCS_ERR_NO_RESOURCE,
        -3 => ucs_status_t::UCS_ERR_IO_ERROR,
        -4 => ucs_status_t::UCS_ERR_NO_MEMORY,
        -5 => ucs_status_t::UCS_ERR_INVALID_PARAM,
        -6 => ucs_status_t::UCS_ERR_UNREACHABLE,
        -7 => ucs_status_t::UCS_ERR_INVALID_ADDR,
        -8 => ucs_status_t::UCS_ERR_NOT_IMPLEMENTED,
        -9 => ucs_status_t::UCS_ERR_MESSAGE_TRUNCATED,
        -10 => ucs_status_t::UCS_ERR_NO_PROGRESS,
        -11 => ucs_status_t::UCS_ERR_BUFFER_TOO_SMALL,
        -12 => ucs_status_t::UCS_ERR_NO_ELEM,
        -13 => ucs_status_t::UCS_ERR_SOME_CONNECTS_FAILED,
        -14 => ucs_status_t::UCS_ERR_NO_DEVICE,
        -15 => ucs_status_t::UCS_ERR_BUSY,
        -16 => ucs_status_t::UCS_ERR_CANCELED,
        -17 => ucs_status_t::UCS_ERR_SHMEM_SEGMENT,
        -18 => ucs_status_t::UCS_ERR_ALREADY_EXISTS,
        -19 => ucs_status_t::UCS_ERR_OUT_OF_RANGE,
        -20 => ucs_status_t::UCS_ERR_TIMED_OUT,
        -21 => ucs_status_t::UCS_ERR_EXCEEDS_LIMIT,
        -22 => ucs_status_t::UCS_ERR_UNSUPPORTED,
        -23 => ucs_status_t::UCS_ERR_REJECTED,
        -24 => ucs_status_t::UCS_ERR_NOT_CONNECTED,
        -25 => ucs_status_t::UCS_ERR_CONNECTION_RESET,
        -40 => ucs_status_t::UCS_ERR_FIRST_LINK_FAILURE,
        -59 => ucs_status_t::UCS_ERR_LAST_LINK_FAILURE,
        -60 => ucs_status_t::UCS_ERR_FIRST_ENDPOINT_FAILURE,
        -80 => ucs_status_t::UCS_ERR_ENDPOINT_TIMEOUT,
        -89 => ucs_status_t::UCS_ERR_LAST_ENDPOINT_FAILURE,
        -100 => ucs_status_t::UCS_ERR_LAST,
        // The committed bindgen enum contains every status value UCX can return;
        // reaching this arm indicates an invalid or unsupported status pointer.
        // Keep decoding non-panicking when runtime UCX returns an unknown status.
        _ => ucs_status_t::UCS_ERR_LAST,
    }
}

// Keep the decoder's literal status table synchronized with the bindgen output.
// These assertions are evaluated while compiling, so regenerated bindings that
// change a status discriminant fail immediately instead of silently misdecoding.
const _: () = {
    assert!(ucs_status_t::UCS_ERR_NO_MESSAGE as i32 == -1);
    assert!(ucs_status_t::UCS_ERR_NO_RESOURCE as i32 == -2);
    assert!(ucs_status_t::UCS_ERR_IO_ERROR as i32 == -3);
    assert!(ucs_status_t::UCS_ERR_NO_MEMORY as i32 == -4);
    assert!(ucs_status_t::UCS_ERR_INVALID_PARAM as i32 == -5);
    assert!(ucs_status_t::UCS_ERR_UNREACHABLE as i32 == -6);
    assert!(ucs_status_t::UCS_ERR_INVALID_ADDR as i32 == -7);
    assert!(ucs_status_t::UCS_ERR_NOT_IMPLEMENTED as i32 == -8);
    assert!(ucs_status_t::UCS_ERR_MESSAGE_TRUNCATED as i32 == -9);
    assert!(ucs_status_t::UCS_ERR_NO_PROGRESS as i32 == -10);
    assert!(ucs_status_t::UCS_ERR_BUFFER_TOO_SMALL as i32 == -11);
    assert!(ucs_status_t::UCS_ERR_NO_ELEM as i32 == -12);
    assert!(ucs_status_t::UCS_ERR_SOME_CONNECTS_FAILED as i32 == -13);
    assert!(ucs_status_t::UCS_ERR_NO_DEVICE as i32 == -14);
    assert!(ucs_status_t::UCS_ERR_BUSY as i32 == -15);
    assert!(ucs_status_t::UCS_ERR_CANCELED as i32 == -16);
    assert!(ucs_status_t::UCS_ERR_SHMEM_SEGMENT as i32 == -17);
    assert!(ucs_status_t::UCS_ERR_ALREADY_EXISTS as i32 == -18);
    assert!(ucs_status_t::UCS_ERR_OUT_OF_RANGE as i32 == -19);
    assert!(ucs_status_t::UCS_ERR_TIMED_OUT as i32 == -20);
    assert!(ucs_status_t::UCS_ERR_EXCEEDS_LIMIT as i32 == -21);
    assert!(ucs_status_t::UCS_ERR_UNSUPPORTED as i32 == -22);
    assert!(ucs_status_t::UCS_ERR_REJECTED as i32 == -23);
    assert!(ucs_status_t::UCS_ERR_NOT_CONNECTED as i32 == -24);
    assert!(ucs_status_t::UCS_ERR_CONNECTION_RESET as i32 == -25);
    assert!(ucs_status_t::UCS_ERR_FIRST_LINK_FAILURE as i32 == -40);
    assert!(ucs_status_t::UCS_ERR_LAST_LINK_FAILURE as i32 == -59);
    assert!(ucs_status_t::UCS_ERR_FIRST_ENDPOINT_FAILURE as i32 == -60);
    assert!(ucs_status_t::UCS_ERR_ENDPOINT_TIMEOUT as i32 == -80);
    assert!(ucs_status_t::UCS_ERR_LAST_ENDPOINT_FAILURE as i32 == -89);
    assert!(ucs_status_t::UCS_ERR_LAST as i32 == -100);
};

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn status_ptr_to_result_immediate_completion() {
        assert!(matches!(
            status_ptr_to_result(ucs_status_t::UCS_OK as usize as ucs_status_ptr_t),
            Ok(None)
        ));
    }

    #[test]
    fn status_ptr_to_result_decodes_error() {
        assert!(matches!(
            status_ptr_to_result(ucs_status_t::UCS_ERR_NO_MEMORY as usize as ucs_status_ptr_t),
            Err(ucs_status_t::UCS_ERR_NO_MEMORY)
        ));
    }

    #[test]
    #[should_panic(expected = "UCX returned UCS_INPROGRESS through a ucs_status_ptr_t API")]
    fn status_ptr_to_result_panics_on_in_progress_in_debug() {
        let _ = status_ptr_to_result(ucs_status_t::UCS_INPROGRESS as usize as ucs_status_ptr_t);
    }

    #[test]
    fn status_from_ptr_unknown_error_is_generic() {
        assert_eq!(
            status_from_ptr((-101i32) as isize as usize as ucs_status_ptr_t),
            ucs_status_t::UCS_ERR_LAST
        );
    }

    #[test]
    fn request_check_finished_on_freed_handle_is_complete() {
        let request = Request { handle: None };
        assert_eq!(request.check_finished(), Ok(true));
    }

    #[test]
    fn request_params_no_imm_cmpl_sets_flag_in_op_attr_mask_only() {
        let mut builder = RequestParamBuilder::new();
        let params = builder.no_imm_cmpl().build();
        assert_ne!(
            params.handle.op_attr_mask & ucp_op_attr_t::UCP_OP_ATTR_FLAG_NO_IMM_CMPL as u32,
            0
        );
        // The flag lives in op_attr_mask; the op-specific `flags` field must stay 0.
        assert_eq!(params.handle.flags, 0);
    }

    #[test]
    fn request_params_reply_buffer_does_not_set_flags_field_mask() {
        let mut reply = 0_u64;
        let params = RequestParamBuilder::new()
            .reply_buffer(&mut reply as *mut _ as *mut std::os::raw::c_void)
            .build();

        assert_ne!(
            params.handle.op_attr_mask & ucp_op_attr_t::UCP_OP_ATTR_FIELD_REPLY_BUFFER as u32,
            0
        );
        assert_eq!(
            params.handle.op_attr_mask & ucp_op_attr_t::UCP_OP_ATTR_FIELD_FLAGS as u32,
            0
        );
    }

    #[test]
    fn request_params_flags_preserves_reply_buffer_field_mask() {
        let mut reply = 0_u64;
        let flags = 0x1234_u32;
        let params = RequestParamBuilder::new()
            .reply_buffer(&mut reply as *mut _ as *mut std::os::raw::c_void)
            .flags(flags)
            .build();

        assert_eq!(params.handle.flags, flags);
        assert_ne!(
            params.handle.op_attr_mask & ucp_op_attr_t::UCP_OP_ATTR_FIELD_FLAGS as u32,
            0
        );
        assert_ne!(
            params.handle.op_attr_mask & ucp_op_attr_t::UCP_OP_ATTR_FIELD_REPLY_BUFFER as u32,
            0
        );
    }

    #[test]
    #[should_panic(expected = "UCP_OP_ATTR_FLAG_NO_IMM_CMPL")]
    fn request_params_force_and_no_imm_cmpl_remain_mutually_exclusive() {
        let mut builder = RequestParamBuilder::new();
        builder.flags(0x1).force_imm_cmpl().no_imm_cmpl();
    }
}

pub struct RequestParam {
    pub(crate) handle: ucp_request_param_t,
}

impl RequestParam {
    /// Build parameters for a fetch operation writing its result to `reply`.
    ///
    /// The reply buffer must outlive every request created from these params
    /// until completion. This helper avoids a raw-pointer conversion at the
    /// call site; the returned params must only be used while `reply` remains
    /// valid.
    #[inline]
    pub fn fetch_params<T>(reply: &mut T) -> RequestParam {
        RequestParamBuilder::new()
            .reply_buffer(reply as *mut T as *mut std::os::raw::c_void)
            .build()
    }
}

#[derive(Debug, Copy, Clone)]
pub struct RequestParamBuilder {
    uninit_handle: std::mem::MaybeUninit<ucp_request_param_t>,
    field_mask: u32,
}

impl Default for RequestParamBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestParamBuilder {
    #[inline]
    pub fn new() -> RequestParamBuilder {
        // SAFETY: UCX parameter structs are valid when zeroed; the field mask
        // controls which fields UCX reads.
        let uninit_params = std::mem::MaybeUninit::new(unsafe { std::mem::zeroed() });
        RequestParamBuilder {
            uninit_handle: uninit_params,
            field_mask: 0,
        }
    }

    #[inline]
    pub fn force_imm_cmpl(&mut self) -> &mut RequestParamBuilder {
        if self.field_mask & ucp_op_attr_t::UCP_OP_ATTR_FLAG_NO_IMM_CMPL as u32 != 0 {
            panic!("Requesting UCP_OP_ATTR_FLAG_FORCE_IMM_CMPL while UCP_OP_ATTR_FLAG_NO_IMM_CMPL is also set");
        }
        self.field_mask |= ucp_op_attr_t::UCP_OP_ATTR_FLAG_FORCE_IMM_CMPL as u32;
        self
    }

    #[inline]
    pub fn no_imm_cmpl(&mut self) -> &mut RequestParamBuilder {
        if self.field_mask & ucp_op_attr_t::UCP_OP_ATTR_FLAG_FORCE_IMM_CMPL as u32 != 0 {
            panic!("Requesting UCP_OP_ATTR_FLAG_NO_IMM_CMPL while UCP_OP_ATTR_FLAG_FORCE_IMM_CMPL is also set");
        }
        self.field_mask |= ucp_op_attr_t::UCP_OP_ATTR_FLAG_NO_IMM_CMPL as u32;
        self
    }

    /// Set operation-specific flags without changing any other field mask bits.
    #[inline]
    pub fn flags(&mut self, flags: u32) -> &mut Self {
        self.field_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_FLAGS as u32;
        // SAFETY: the builder initialized the parameter storage with zeroed UCX fields.
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.flags = flags;
        self
    }

    /// Set the reply buffer field without changing operation-specific flags.
    ///
    /// Prefer [`RequestParam::fetch_params`] when the reply buffer is a Rust
    /// reference, as it makes the buffer lifetime requirement explicit.
    #[inline]
    pub fn reply_buffer(&mut self, buf: *mut std::os::raw::c_void) -> &mut Self {
        self.field_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_REPLY_BUFFER as u32;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.reply_buffer = buf;
        self
    }

    #[inline]
    pub fn datatype(&mut self, dt: ucp_datatype_t) -> &mut Self {
        self.field_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_DATATYPE as u32;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.datatype = dt;
        self
    }

    #[inline]
    pub fn send_callback(&mut self, cb: ucp_send_nbx_callback_t) -> &mut Self {
        self.field_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_CALLBACK as u32;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.cb.send = cb;
        self
    }

    #[inline]
    pub fn memory_type(&mut self, mt: ucs_memory_type_t) -> &mut Self {
        self.field_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_MEMORY_TYPE as u32;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.memory_type = mt;
        self
    }

    #[inline]
    pub fn build(&mut self) -> RequestParam {
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.op_attr_mask = self.field_mask;

        RequestParam {
            handle: unsafe { self.uninit_handle.assume_init() },
        }
    }
}

/// Allocate a request object from the worker.
///
/// # Safety
/// The returned request must be freed with `Request::from_raw().free()` or similar.
pub unsafe fn request_alloc(worker: ucp_worker_h) -> Request {
    let ptr = ucp_request_alloc(worker);
    Request::from_raw(ptr)
}

/// Query request attributes.
///
/// Field masks for ucp_request_attr_t:
/// - UCP_REQUEST_ATTR_FIELD_INFO_STRING = 1
/// - UCP_REQUEST_ATTR_FIELD_INFO_STRING_SIZE = 2
/// - UCP_REQUEST_ATTR_FIELD_STATUS = 4
/// - UCP_REQUEST_ATTR_FIELD_MEM_TYPE = 8
///
/// # Safety
/// Caller must ensure `request` is a valid request pointer.
pub unsafe fn request_query(
    request: *mut std::os::raw::c_void,
    mask: u64,
) -> Result<RequestAttr, ucs_status_t> {
    let mut attr: ucp_request_attr_t = std::mem::zeroed();
    attr.field_mask = mask;
    status_to_result(ucp_request_query(request, &mut attr)).map(|()| RequestAttr {
        status: if mask & 4 != 0 {
            attr.status
        } else {
            ucs_status_t::UCS_OK
        },
    })
}

/// Request attribute result.
#[derive(Debug, Clone)]
pub struct RequestAttr {
    pub status: ucs_status_t,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context;
    use crate::context::Context;
    use crate::ep;
    use crate::worker;
    use crate::worker::RemoteWorkerAddress;
    use std::rc::Rc;

    const TEST_AM_ID: u32 = 5;

    extern "C" fn init(_request: *mut ::std::os::raw::c_void) {}

    extern "C" fn cleanup(_request: *mut ::std::os::raw::c_void) {}

    unsafe extern "C" fn am_cb(
        arg: *mut ::std::os::raw::c_void,
        header: *const ::std::os::raw::c_void,
        header_length: usize,
        _data: *mut ::std::os::raw::c_void,
        _length: usize,
        _param: *const ucp_am_recv_param_t,
    ) -> ucs_status_t {
        let message = std::slice::from_raw_parts_mut(arg as *mut i8, 1);
        let in_data = std::slice::from_raw_parts(header as *const i8, header_length);
        message[0] = in_data[0];
        ucs_status_t::UCS_OK
    }

    pub struct CommsContext {
        pub ep: ep::Ep,
        pub worker: worker::Worker,
        #[allow(dead_code)]
        pub context: context::Context,
    }

    pub fn setup_default() -> Rc<CommsContext> {
        let features = context::Flags::Am
            | context::Flags::Rma
            | context::Flags::Amo32
            | context::Flags::Amo64
            | context::Flags::Tag;

        let params = context::ParamsBuilder::new()
            .features(features)
            .mt_workers_shared(1)
            .request_init(init)
            .request_cleanup(cleanup)
            .request_size(8)
            .name("My Awesome Test")
            .expect("context name")
            .tag_sender_mask(u64::MAX)
            .estimated_num_eps(4)
            .estimated_num_ppn(2)
            .build();

        let worker_features = worker::ParamsBuilder::new()
            .thread_mode(ucs_thread_mode_t::UCS_THREAD_MODE_MULTI)
            .build();

        let mut context = Context::new(
            &context::Config::read("", "").expect("config read"),
            &params,
        )
        .unwrap();

        let worker = context.worker_create(&worker_features).unwrap();
        let packed_addr = worker.pack_address().unwrap();
        let addr = RemoteWorkerAddress::new(packed_addr.to_vec());

        let ep_param = ep::ParamsBuilder::new().address(&addr).build();
        let ep = worker.create_ep(ep_param).unwrap();
        // If we don't drop this than the compiler complains about how the
        // worker is borrowed in the packed_addr.
        drop(packed_addr);

        let mut progressed = worker.progress();
        while progressed {
            progressed = worker.progress();
        }
        Rc::new(CommsContext {
            context,
            worker,
            ep,
        })
    }

    #[test]
    fn request_helper_api_signatures() {
        let is_completed: fn(&Request) -> bool = Request::is_completed;
        let test: fn(&Request) -> RequestState = Request::test;
        let release: fn(Request) = Request::release;
        let check_finished: fn(&Request) -> Result<bool, ucs_status_t> = Request::check_finished;
        let _ = (is_completed, test, release, check_finished);
    }

    #[test]
    fn setup() {
        let _ = setup_default();
    }

    #[test]
    fn am() {
        let comms = setup_default();
        let send_buffer = vec![32];
        let mut recv_buffer = vec![0];

        let am_params = am::HandlerParamsBuilder::new()
            .id(TEST_AM_ID)
            .cb(am_cb)
            .arg(recv_buffer.as_mut_ptr() as *mut std::os::raw::c_void)
            .build();
        comms.worker.am_register(&am_params).unwrap();

        let am_flags = RequestParamBuilder::new().no_imm_cmpl().build();

        let req = comms
            .ep
            .am_send(TEST_AM_ID, send_buffer.as_slice(), b"", &am_flags)
            .unwrap();
        if let Some(req) = req {
            while !req.check_finished().unwrap() {
                comms.worker.progress();
            }
        }
        assert_eq!(send_buffer[0], recv_buffer[0]);
    }
}
