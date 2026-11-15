/// Native alias for the send-completion callback. The completion status arrives as
/// [`crate::Status`], which is `#[repr(transparent)]` over the raw status, so this matches
/// UCX's C callback type.
pub type SendCallback = Option<
    unsafe extern "C" fn(
        request: *mut std::os::raw::c_void,
        status: crate::Status,
        user_data: *mut std::os::raw::c_void,
    ),
>;

use crate::ffi::*;
use crate::status::status_from_ptr;
use crate::status_to_result;
use crate::worker;
use crate::Status;
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
#[derive(Debug)]
pub struct Request {
    // A dangling pointer denotes an inert handle, leaving the null niche for Pending.
    pub(crate) handle: NonNull<::std::os::raw::c_void>,
}

/// Outcome of a non-blocking UCX operation.
#[derive(Debug)]
pub enum Pending<T = Request> {
    /// Completed inline; input buffers may be reused immediately.
    Complete,
    /// In flight; drive with worker.progress() and check_finished().
    InProgress(T),
}

impl<T> Pending<T> {
    /// True if this operation completed inline.
    #[inline]
    pub fn is_complete(&self) -> bool {
        matches!(self, Pending::Complete)
    }

    /// Consume this and return the underlying request if in progress.
    /// Returns `None` if the operation completed inline.
    #[inline]
    pub fn into_request(self) -> Option<T> {
        match self {
            Pending::Complete => None,
            Pending::InProgress(req) => Some(req),
        }
    }
}

impl Pending {
    /// Bounded progress loop that waits for completion.
    ///
    /// Calls `worker.progress()` up to 1_000_000 times or until the
    /// request completes. On timeout, the request is leaked (not freed
    /// while in flight). Returns `Ok(())` on success, `Err(status)` on
    /// failure.
    pub fn wait(self, worker: &crate::worker::Worker) -> Result<(), crate::Status> {
        match self {
            Pending::Complete => Ok(()),
            Pending::InProgress(req) => {
                for _ in 0..1_000_000 {
                    if let Ok(true) = req.check_finished() {
                        return Ok(());
                    }
                    worker.progress();
                }
                // Timeout: leak the request rather than freeing it in flight
                std::mem::forget(req);
                Err(crate::Status::from(ucs_status_t::UCS_ERR_TIMED_OUT))
            }
        }
    }
}

/// An active-message send request borrowing its worker, header, and data.
///
/// Dropping this guard progresses the worker until completion or timeout.
/// On timeout, the request is leaked rather than freed while in flight.
pub struct AmRequest<'w, 'a> {
    pub(crate) worker: &'w worker::Worker,
    // These fields retain the buffer borrows for the lifetime of the guard.
    #[allow(dead_code)]
    pub(crate) header: &'a [u8],
    #[allow(dead_code)]
    pub(crate) data: &'a [u8],
    pub(crate) request: Request,
}

impl Drop for AmRequest<'_, '_> {
    fn drop(&mut self) {
        let _ = self.wait_for_completion();
    }
}

impl<'w, 'a> AmRequest<'w, 'a> {
    /// Returns `Ok(true)` when complete, `Ok(false)` while in progress,
    /// or `Err(status)` on failure.
    #[inline]
    pub fn check_finished(&self) -> Result<bool, Status> {
        self.request.check_finished()
    }

    /// Progress to completion and release the request and its borrows.
    ///
    /// On timeout, the request is leaked and a timeout status is returned.
    pub fn free(mut self) -> Result<(), Status> {
        self.wait_for_completion()
    }

    /// Extract the underlying request without progressing it.
    ///
    /// # Safety
    /// The caller must keep the worker and both header and data buffers valid,
    /// and must not modify either buffer, until the request completes.
    pub unsafe fn into_inner(self) -> Request {
        let this = std::mem::ManuallyDrop::new(self);
        // SAFETY: suppressing the guard's destructor transfers sole ownership
        // of the request to the caller.
        unsafe { std::ptr::read(&this.request) }
    }

    /// Return the worker borrowed by this request.
    #[inline]
    pub fn worker(&self) -> &worker::Worker {
        self.worker
    }

    fn wait_for_completion(&mut self) -> Result<(), Status> {
        for _ in 0..1_000_000 {
            match self.request.check_finished() {
                Ok(true) => return Ok(()),
                Ok(false) => {
                    self.worker.progress();
                }
                Err(status) => return Err(status),
            }
        }
        // Timeout: leave the handle unowned rather than freeing it in flight.
        let _ = self.request.take_handle();
        Err(Status::from(ucs_status_t::UCS_ERR_TIMED_OUT))
    }
}

/// The result of testing a non-blocking request.
#[derive(Debug, Clone)]
pub struct RequestState {
    /// `Ok(())` means the request completed successfully; `Err` contains the
    /// UCX status returned by `ucp_request_test` (including `UCS_INPROGRESS`).
    pub status: Result<(), Status>,
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Some(h) = self.take_handle() {
            // SAFETY: handle came from UCX as a request pointer and has not been freed yet.
            unsafe { ucp_request_free(h.as_ptr()) };
        }
    }
}

impl Request {
    /// Wrap a non-null request pointer. Returns `None` if `request_handle` is null.
    #[inline]
    pub fn new(request_handle: *mut std::os::raw::c_void) -> Option<Request> {
        NonNull::new(request_handle).map(|h| Request { handle: h })
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
            handle: NonNull::new(ptr).expect("Request::from_raw with null"),
        }
    }

    /// Raw pointer for FFI, or null if already freed/cancelled.
    #[inline]
    pub fn as_ptr(&self) -> *mut std::os::raw::c_void {
        if self.is_live() {
            self.handle.as_ptr()
        } else {
            std::ptr::null_mut()
        }
    }

    /// True if this still owns a live UCX request.
    #[inline]
    pub fn is_live(&self) -> bool {
        self.handle != NonNull::dangling()
    }

    #[inline]
    fn take_handle(&mut self) -> Option<NonNull<::std::os::raw::c_void>> {
        if !self.is_live() {
            return None;
        }
        Some(std::mem::replace(&mut self.handle, NonNull::dangling()))
    }

    /// Check an outstanding request.
    ///
    /// Returns `Ok(true)` completed, `Ok(false)` in progress, `Err` on failure.
    /// Returns `Ok(true)` if the request was already freed/cancelled.
    #[inline]
    pub fn check_finished(&self) -> Result<bool, Status> {
        if !self.is_live() {
            return Ok(true);
        }
        let status = unsafe { ucp_request_check_status(self.handle.as_ptr()) };
        let status_ptr = status as isize as usize as ucs_status_ptr_t;
        if status_ptr_is_err(status_ptr) {
            return Err(crate::Status::from(status_from_ptr(status_ptr)));
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
        if !self.is_live() {
            return true;
        }
        unsafe { ucp_request_is_completed(self.handle.as_ptr()) != 0 }
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
        if !self.is_live() {
            return RequestState { status: Ok(()) };
        }
        let mut info: ucp_tag_recv_info_t = unsafe { std::mem::zeroed() };
        let status = unsafe { ucp_request_test(self.handle.as_ptr(), &mut info) };
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
        if let Some(h) = self.take_handle() {
            unsafe { ucp_request_release(h.as_ptr()) };
        }
    }

    /// Cancel this request on `worker`, then free it (handle becomes inert).
    ///
    /// After cancel, further `check_finished` returns `Ok(true)`.
    pub fn cancel(&mut self, worker: &worker::Worker) {
        if let Some(h) = self.take_handle() {
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
            .take_handle()
            .map(|h| unsafe { ucp_request_free(h.as_ptr()) });
    }
}

// UCX uses `ucs_status_ptr_t` for nonblocking operations: the result is either
// an immediate status, a request pointer, or an error. This helper maps those
// outcomes to `Result<Pending, Status>`. A status-pointer API
// never returns `UCS_INPROGRESS`; an incomplete operation returns a request
// pointer instead, while callers observe that status through plain
// `ucs_status_t` APIs such as `ucp_request_check_status`.

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
    pub fn datatype(&mut self, dt: u64) -> &mut Self {
        self.field_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_DATATYPE as u32;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.datatype = dt as ucp_datatype_t;
        self
    }

    #[inline]
    pub fn send_callback(&mut self, cb: SendCallback) -> &mut Self {
        self.field_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_CALLBACK as u32;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        // SAFETY: `Status` is `#[repr(transparent)]` over `ucs_status_t`, so `SendCallback` and
        // UCX's `ucp_send_nbx_callback_t` have ABI-compatible signatures.
        params.cb.send =
            unsafe { std::mem::transmute::<SendCallback, ucp_send_nbx_callback_t>(cb) };
        self
    }

    /// Set the memory type of the operation's buffer, so UCX does not have to detect it.
    ///
    /// Leaving the memory type unset, or passing `MemoryType::Unknown`, makes UCX detect the
    /// memory type of the buffer itself.
    #[inline]
    pub fn memory_type(&mut self, mt: crate::MemoryType) -> &mut Self {
        self.field_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_MEMORY_TYPE as u32;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.memory_type = mt.to_ffi();
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
pub unsafe fn request_alloc(worker: &crate::worker::Worker) -> Request {
    let ptr = ucp_request_alloc(worker.handle);
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
) -> Result<RequestAttr, Status> {
    let mut attr: ucp_request_attr_t = std::mem::zeroed();
    attr.field_mask = mask;
    status_to_result(ucp_request_query(request, &mut attr)).map(|()| RequestAttr {
        status: if mask & 4 != 0 {
            Status::from(attr.status)
        } else {
            Status::from(ucs_status_t::UCS_OK)
        },
    })
}

/// Request attribute result.
#[derive(Debug, Clone)]
pub struct RequestAttr {
    pub status: Status,
}

pub struct RequestAttrFields(u64);

#[allow(non_upper_case_globals)]
impl RequestAttrFields {
    /// Fill the informational string describing the request.
    pub const InfoString: RequestAttrFields = RequestAttrFields(1);
    /// Size of the informational string buffer.
    pub const InfoStringSize: RequestAttrFields = RequestAttrFields(2);
    /// Current status of the request.
    pub const Status: RequestAttrFields = RequestAttrFields(4);
    /// Memory type of the request's buffer.
    pub const MemType: RequestAttrFields = RequestAttrFields(8);

    /// An empty mask: query no attribute.
    #[inline]
    pub const fn empty() -> RequestAttrFields {
        RequestAttrFields(0)
    }

    /// The raw field-mask bits handed to UCX.
    #[inline]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// True if every bit of `other` is set in `self`.
    #[inline]
    pub const fn contains(self, other: RequestAttrFields) -> bool {
        (self.0 & other.0) == other.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::am::AmRecvParam;
    use crate::tests::setup_default;

    const TEST_AM_ID: u32 = 5;

    unsafe extern "C" fn am_cb(
        arg: *mut ::std::os::raw::c_void,
        header: *const ::std::os::raw::c_void,
        header_length: usize,
        _data: *mut ::std::os::raw::c_void,
        _length: usize,
        _param: *const AmRecvParam,
    ) -> Status {
        let message = std::slice::from_raw_parts_mut(arg as *mut i8, 1);
        let in_data = std::slice::from_raw_parts(header as *const i8, header_length);
        message[0] = in_data[0];
        Status::OK
    }

    #[test]
    fn request_helper_api_signatures() {
        let is_completed: fn(&Request) -> bool = Request::is_completed;
        let test: fn(&Request) -> RequestState = Request::test;
        let release: fn(Request) = Request::release;
        let check_finished: fn(&Request) -> Result<bool, Status> = Request::check_finished;
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

        let am_params = crate::am::HandlerParamsBuilder::new()
            .id(TEST_AM_ID)
            .cb(am_cb)
            .arg(recv_buffer.as_mut_ptr() as *mut std::os::raw::c_void)
            .build();
        comms.worker.am_register(&am_params).unwrap();

        let am_flags = RequestParamBuilder::new().no_imm_cmpl().build();

        let req = comms
            .ep
            .am_send(
                &comms.worker,
                TEST_AM_ID,
                send_buffer.as_slice(),
                b"",
                &am_flags,
            )
            .unwrap();
        if let Pending::InProgress(req) = req {
            while !req.check_finished().unwrap() {
                comms.worker.progress();
            }
        }
        assert_eq!(send_buffer[0], recv_buffer[0]);
    }

    #[test]
    fn status_ptr_to_result_immediate_completion() {
        let result = crate::status::status_ptr_to_result(std::ptr::null_mut());
        assert!(matches!(result, Ok(Pending::Complete)));
    }

    #[test]
    fn pending_size_assertion() {
        use std::mem::size_of;
        assert_eq!(
            size_of::<Pending>(),
            8,
            "Pending must be 8 bytes for niche optimization"
        );
    }

    #[test]
    fn status_ptr_to_result_decodes_error() {
        let ptr = ucs_status_t::UCS_ERR_NO_MEMORY as isize as ucs_status_ptr_t;
        assert_eq!(
            crate::status::status_ptr_to_result(ptr).unwrap_err(),
            Status::from(ucs_status_t::UCS_ERR_NO_MEMORY)
        );
    }
}
