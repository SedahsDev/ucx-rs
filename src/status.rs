use crate::ffi::*;
use crate::request::Request;

/// Native error type wrapping a UCX status code.
///
/// This is the public error type for the crate. It wraps the raw
/// `ucs_status_t` so consumers never name the bindgen type directly.
/// `#[repr(transparent)]` gives `Status` exactly the ABI of the raw status type, so callback types such as `crate::am::AmRecvCb` can take or return `Status` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Status(pub(crate) ucs_status_t);

impl Status {
    /// `UCS_OK`
    pub const OK: Status = Status(ucs_status_t::UCS_OK);
    /// `UCS_INPROGRESS`
    pub const IN_PROGRESS: Status = Status(ucs_status_t::UCS_INPROGRESS);
    /// `UCS_ERR_NO_MESSAGE`
    pub const NO_MESSAGE: Status = Status(ucs_status_t::UCS_ERR_NO_MESSAGE);
    /// `UCS_ERR_NO_RESOURCE`
    pub const NO_RESOURCE: Status = Status(ucs_status_t::UCS_ERR_NO_RESOURCE);
    /// `UCS_ERR_IO_ERROR`
    pub const IO_ERROR: Status = Status(ucs_status_t::UCS_ERR_IO_ERROR);
    /// `UCS_ERR_NO_MEMORY`
    pub const NO_MEMORY: Status = Status(ucs_status_t::UCS_ERR_NO_MEMORY);
    /// `UCS_ERR_INVALID_PARAM`
    pub const INVALID_PARAM: Status = Status(ucs_status_t::UCS_ERR_INVALID_PARAM);
    /// `UCS_ERR_UNREACHABLE`
    pub const UNREACHABLE: Status = Status(ucs_status_t::UCS_ERR_UNREACHABLE);
    /// `UCS_ERR_INVALID_ADDR`
    pub const INVALID_ADDR: Status = Status(ucs_status_t::UCS_ERR_INVALID_ADDR);
    /// `UCS_ERR_NOT_IMPLEMENTED`
    pub const NOT_IMPLEMENTED: Status = Status(ucs_status_t::UCS_ERR_NOT_IMPLEMENTED);
    /// `UCS_ERR_MESSAGE_TRUNCATED`
    pub const MESSAGE_TRUNCATED: Status = Status(ucs_status_t::UCS_ERR_MESSAGE_TRUNCATED);
    /// `UCS_ERR_NO_PROGRESS`
    pub const NO_PROGRESS: Status = Status(ucs_status_t::UCS_ERR_NO_PROGRESS);
    /// `UCS_ERR_BUFFER_TOO_SMALL`
    pub const BUFFER_TOO_SMALL: Status = Status(ucs_status_t::UCS_ERR_BUFFER_TOO_SMALL);
    /// `UCS_ERR_NO_ELEM`
    pub const NO_ELEM: Status = Status(ucs_status_t::UCS_ERR_NO_ELEM);
    /// `UCS_ERR_SOME_CONNECTS_FAILED`
    pub const SOME_CONNECTS_FAILED: Status = Status(ucs_status_t::UCS_ERR_SOME_CONNECTS_FAILED);
    /// `UCS_ERR_NO_DEVICE`
    pub const NO_DEVICE: Status = Status(ucs_status_t::UCS_ERR_NO_DEVICE);
    /// `UCS_ERR_BUSY`
    pub const BUSY: Status = Status(ucs_status_t::UCS_ERR_BUSY);
    /// `UCS_ERR_CANCELED`
    pub const CANCELED: Status = Status(ucs_status_t::UCS_ERR_CANCELED);
    /// `UCS_ERR_SHMEM_SEGMENT`
    pub const SHMEM_SEGMENT: Status = Status(ucs_status_t::UCS_ERR_SHMEM_SEGMENT);
    /// `UCS_ERR_ALREADY_EXISTS`
    pub const ALREADY_EXISTS: Status = Status(ucs_status_t::UCS_ERR_ALREADY_EXISTS);
    /// `UCS_ERR_OUT_OF_RANGE`
    pub const OUT_OF_RANGE: Status = Status(ucs_status_t::UCS_ERR_OUT_OF_RANGE);
    /// `UCS_ERR_TIMED_OUT`
    pub const TIMED_OUT: Status = Status(ucs_status_t::UCS_ERR_TIMED_OUT);
    /// `UCS_ERR_EXCEEDS_LIMIT`
    pub const EXCEEDS_LIMIT: Status = Status(ucs_status_t::UCS_ERR_EXCEEDS_LIMIT);
    /// `UCS_ERR_UNSUPPORTED`
    pub const UNSUPPORTED: Status = Status(ucs_status_t::UCS_ERR_UNSUPPORTED);
    /// `UCS_ERR_REJECTED`
    pub const REJECTED: Status = Status(ucs_status_t::UCS_ERR_REJECTED);
    /// `UCS_ERR_NOT_CONNECTED`
    pub const NOT_CONNECTED: Status = Status(ucs_status_t::UCS_ERR_NOT_CONNECTED);
    /// `UCS_ERR_CONNECTION_RESET`
    pub const CONNECTION_RESET: Status = Status(ucs_status_t::UCS_ERR_CONNECTION_RESET);
    /// `UCS_ERR_ENDPOINT_TIMEOUT`
    pub const ENDPOINT_TIMEOUT: Status = Status(ucs_status_t::UCS_ERR_ENDPOINT_TIMEOUT);

    /// Whether this status represents an error (negative UCX status).
    pub fn is_err(self) -> bool {
        (self.0 as i8) < 0
    }
}

// `Status` is passed to and returned from C callbacks in place of `ucs_status_t`.
const _: () = {
    assert!(std::mem::size_of::<Status>() == std::mem::size_of::<ucs_status_t>());
    assert!(std::mem::align_of::<Status>() == std::mem::align_of::<ucs_status_t>());
};

impl From<ucs_status_t> for Status {
    fn from(status: ucs_status_t) -> Self {
        Status(status)
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UCX status {:?}", self.0)
    }
}

impl std::error::Error for Status {}

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
pub(crate) fn status_ptr_to_result(ptr: ucs_status_ptr_t) -> Result<Option<Request>, Status> {
    if status_ptr_is_err(ptr) {
        return Err(Status::from(status_from_ptr(ptr)));
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
pub(crate) fn status_to_result(status: ucs_status_t) -> Result<(), Status> {
    // Per ucs/type/status.h, UCS_ERR_* values are negative and success values are non-negative.
    if status_value_is_err(status) {
        return Err(Status::from(status));
    }
    Ok(())
}

#[inline]
fn status_ptr_is_err(ptr: ucs_status_ptr_t) -> bool {
    ptr as usize >= (ucs_status_t::UCS_ERR_LAST as isize) as usize
}

#[inline]
fn status_value_is_err(status: ucs_status_t) -> bool {
    (status as i8) < 0
}

#[inline]
pub(crate) fn status_from_ptr(ptr: ucs_status_ptr_t) -> ucs_status_t {
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
    use crate::RequestParamBuilder;

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
            Err(Status(ucs_status_t::UCS_ERR_NO_MEMORY))
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

#[cfg(test)]
mod native_status_tests {
    use super::*;
    use crate::am::HandlerParamsBuilder;
    use crate::ep::ParamsBuilder as EpParamsBuilder;
    use crate::request::RequestParamBuilder;
    use std::os::raw::c_void;

    #[test]
    fn constants_wrap_matching_raw_codes() {
        assert_eq!(Status::OK, Status(ucs_status_t::UCS_OK));
        assert_eq!(Status::IN_PROGRESS, Status(ucs_status_t::UCS_INPROGRESS));
        assert_eq!(
            Status::INVALID_PARAM,
            Status(ucs_status_t::UCS_ERR_INVALID_PARAM)
        );
        assert_eq!(
            Status::ENDPOINT_TIMEOUT,
            Status(ucs_status_t::UCS_ERR_ENDPOINT_TIMEOUT)
        );
        assert!(!Status::OK.is_err());
        assert!(!Status::IN_PROGRESS.is_err());
        assert!(Status::CANCELED.is_err());
    }

    #[test]
    fn send_callback_receives_status_through_raw_pointer() {
        unsafe extern "C" fn record(_request: *mut c_void, status: Status, user_data: *mut c_void) {
            *(user_data as *mut Status) = status;
        }

        let params = RequestParamBuilder::new()
            .send_callback(Some(record))
            .build();
        let raw = unsafe { params.handle.cb.send }.expect("callback stored");
        let mut seen = Status::OK;
        unsafe {
            raw(
                std::ptr::null_mut(),
                ucs_status_t::UCS_ERR_CANCELED,
                &mut seen as *mut Status as *mut c_void,
            )
        };
        assert_eq!(seen, Status::CANCELED);
    }

    #[test]
    fn err_handler_receives_status_through_raw_pointer() {
        unsafe extern "C" fn on_error(arg: *mut c_void, _ep: ucp_ep_h, status: Status) {
            *(arg as *mut Status) = status;
        }

        let params = EpParamsBuilder::new().err_handler(Some(on_error)).build();
        let raw = params.handle.err_handler.cb.expect("callback stored");
        let mut seen = Status::OK;
        unsafe {
            raw(
                &mut seen as *mut Status as *mut c_void,
                std::ptr::null_mut(),
                ucs_status_t::UCS_ERR_TIMED_OUT,
            )
        };
        assert_eq!(seen, Status::TIMED_OUT);
    }

    #[test]
    fn am_callback_returns_status_through_raw_pointer() {
        unsafe extern "C" fn reject(
            _arg: *mut c_void,
            _header: *const c_void,
            _header_length: usize,
            _data: *mut c_void,
            _length: usize,
            _param: *const ucp_am_recv_param_t,
        ) -> Status {
            Status::REJECTED
        }

        let params = HandlerParamsBuilder::new().cb(reject).build();
        let raw = params.handle.cb.expect("callback stored");
        let result = unsafe {
            raw(
                std::ptr::null_mut(),
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        assert_eq!(result, ucs_status_t::UCS_ERR_REJECTED);
    }
}
