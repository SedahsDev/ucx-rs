//! Rust bindings for UCX's UCP API.
//!
//! The default threading model is a single-threaded progress loop. `Context`,
//! `Worker`, `Ep`, `MemHandle`, and `RemoteKey` are intentionally neither
//! `Send` nor `Sync`: UCX objects are not thread-safe unless the relevant
//! objects are created with `UCS_THREAD_MODE_MULTI`.
//!
//! Multi-threaded applications must provide their own synchronization and
//! ensure it matches UCX's threading contract. An `MtWorker` abstraction may be
//! added in a future release; it is not provided by this crate currently.
#![allow(unused_imports)]

mod ffi;
use crate::ffi::*;

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

// In UCX we usually use a ucs_status_ptr_t to represent the status of a nonblocking operation
// in this the possible outcomes can be UCS_OK, where the application can reuse all the input
// parameters immediately, a pointer that can be queried for the status of the underlying
// nonblocking operation, or an error. Rust APIs operate similarly, except it uses the Rust
// type system to express this. It will have a Result type that either contains an Ok() type
// or an Err() type. It also has an Option() type that basically is the equivalent of a nullable
// pointer, except Rust will force the user to be sure to check the Option().

// This helper function will automatically translate the ucs_status_ptr_t into a Result that
// either is an empty Ok() as the equivilent to UCS_OK, a Ok(Request) that represents getting
// back a pointer or an Err(ucs_status_t) that indicates an error. Compile test shows that this
// produces extremely efficient assembly

#[inline]
pub fn status_ptr_to_result(ptr: ucs_status_ptr_t) -> Result<Option<Request>, ucs_status_t> {
    if status_ptr_is_err(ptr) {
        return Err(status_from_ptr(ptr));
    }
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
    fn status_ptr_to_result_treats_in_progress_as_immediate() {
        assert!(matches!(
            status_ptr_to_result(ucs_status_t::UCS_INPROGRESS as usize as ucs_status_ptr_t),
            Ok(None)
        ));
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
}

pub struct RequestParam {
    pub(crate) handle: ucp_request_param_t,
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

    #[inline]
    pub fn reply_buffer(&mut self, buf: *mut std::os::raw::c_void) -> &mut Self {
        self.field_mask |= (ucp_op_attr_t::UCP_OP_ATTR_FIELD_REPLY_BUFFER as u32)
            | (ucp_op_attr_t::UCP_OP_ATTR_FIELD_FLAGS as u32);
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.flags = 0;
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

        let context = Context::new(
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
