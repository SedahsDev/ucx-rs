use crate::ffi::*;
use crate::request::Request;

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

#[inline]
fn status_ptr_is_err(ptr: ucs_status_ptr_t) -> bool {
    ptr as usize >= (ucs_status_t::UCS_ERR_LAST as isize) as usize
}

#[inline]
fn status_value_is_err(status: ucs_status_t) -> bool {
    (status as i8) < 0
}

#[inline]
pub fn status_from_ptr(ptr: ucs_status_ptr_t) -> ucs_status_t {
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
