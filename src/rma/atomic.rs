use crate::ep::Ep;
use crate::ffi::*;
use crate::RequestParam;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Request;
use std::sync::Arc;

use super::{RemoteKey, FetchAmoRequest};

#[deprecated = "Use Ep::rma_put() instead"]
/// Put data to a remote memory location.
///
/// # Safety
/// Caller must ensure `buffer` is valid for `count` bytes and `rkey` is valid.
pub unsafe fn put_nbx(
    ep: &Ep,
    buffer: *const std::os::raw::c_void,
    count: usize,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    status_ptr_to_result(ucp_put_nbx(
        ep.handle,
        buffer,
        count,
        remote_addr,
        rkey.handle,
        &param.handle,
    ))
}

#[deprecated = "Use Ep::rma_get() instead"]
/// Get data from a remote memory location.
///
/// # Safety
/// Caller must ensure `buffer` has space for `count` bytes and `rkey` is valid.
pub unsafe fn get_nbx(
    ep: &Ep,
    buffer: *mut std::os::raw::c_void,
    count: usize,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    status_ptr_to_result(ucp_get_nbx(
        ep.handle,
        buffer,
        count,
        remote_addr,
        rkey.handle,
        &param.handle,
    ))
}

#[deprecated = "Use Ep::amo_add64/amo_xor64/amo_swap64/amo_and64/amo_or64/amo_cswap64 instead"]
/// Atomic fetch-and-add/subtract operation (nbx variant).
///
/// # Safety
/// Caller must ensure `ep` is a valid endpoint and `operand` points to valid memory.
#[allow(clippy::too_many_arguments)]
pub unsafe fn atomic_op_nbx(
    ep: &Ep,
    opcode: ucp_atomic_op_t,
    buffer: *const std::os::raw::c_void,
    count: usize,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    status_ptr_to_result(ucp_atomic_op_nbx(
        ep.handle,
        opcode,
        buffer,
        count,
        remote_addr,
        rkey.handle,
        &param.handle,
    ))
}

#[deprecated = "Use Ep::amo_fadd64/amo_fxor64/amo_fswap64/amo_fcswap64 instead"]
/// Atomic fetch-and-operate on remote memory.
///
/// Performs an atomic operation and stores the OLD value in `reply_buffer`.
/// For `UCP_ATOMIC_OP_ADD`, this is fetch-and-add (the old value before addition).
///
/// Example usage for fetch-and-add:
/// ```ignore
/// let mut reply: u64 = 0;
/// let operand: u64 = 1;
/// let param = crate::RequestParamBuilder::new()
///     .reply_buffer(&mut reply as *mut _ as *mut std::os::raw::c_void)
///     .build();
/// unsafe {
///     rma::atomic_fetch_nbx(ep, ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
///         &operand as *const _ as *const _, 8, remote_addr, rkey, &param);
/// }
/// ```
///
/// # Safety
/// Caller must ensure `operand` points to valid operand data, `reply_buffer`
/// has space for the result, and `rkey` is valid.
#[allow(clippy::too_many_arguments)]
pub unsafe fn atomic_fetch_nbx(
    ep: &Ep,
    opcode: ucp_atomic_op_t,
    operand: *const std::os::raw::c_void,
    _reply_buffer: *mut std::os::raw::c_void,
    count: usize,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    status_ptr_to_result(ucp_atomic_op_nbx(
        ep.handle,
        opcode,
        operand,
        count,
        remote_addr,
        rkey.handle,
        &param.handle,
    ))
}

#[deprecated = "Use RemoteKey::unpack() instead"]
/// Unpack a remote key from a packed buffer.
///
/// Returns the unpacked rkey handle.
///
/// # Safety
/// Caller must ensure `rkey_buffer` is valid and `ep` is a valid endpoint handle.
pub unsafe fn ep_rkey_unpack(
    ep: &Ep,
    rkey_buffer: *const std::os::raw::c_void,
) -> Result<RemoteKey, ucs_status_t> {
    let mut rkey: ucp_rkey_h = std::ptr::null_mut();
    status_to_result(ucp_ep_rkey_unpack(ep.handle, rkey_buffer, &mut rkey)).map(|()| RemoteKey {
        handle: rkey,
        worker_alive: Arc::clone(&ep.worker_alive),
    })
}

#[deprecated = "No safe replacement — use with caution"]
/// Get a local pointer to a remote memory region.
///
/// Returns a local pointer that can be used to access remote memory directly.
/// # Safety
/// Caller must ensure `rkey` is a valid, non-null remote key handle and `raddr`
/// points to valid remote memory.
///
/// **IMPORTANT:** The underlying UCX C function `ucp_rkey_ptr` does not validate
/// null rkey handles — it will segfault instead of returning an error. Always
/// use [`RemoteKey::remote_ptr`] for safe access.
pub unsafe fn rkey_ptr(
    rkey: &RemoteKey,
    raddr: u64,
) -> Result<*mut std::os::raw::c_void, ucs_status_t> {
    if rkey.handle.is_null() {
        return Err(ucs_status_t::UCS_ERR_INVALID_PARAM);
    }
    let mut addr: *mut std::os::raw::c_void = std::ptr::null_mut();
    status_to_result(ucp_rkey_ptr(rkey.handle, raddr, &mut addr)).map(|()| addr)
}

#[deprecated = "Use RemoteKey RAII wrapper instead (auto-destroy on drop)"]
/// Destroy a remote key.
///
/// # Safety
/// Caller must ensure `rkey` is a valid, non-duplicate remote key handle.
pub unsafe fn rkey_destroy(rkey: &RemoteKey) {
    ucp_rkey_destroy(rkey.handle);
}

// ---------------------------------------------------------------------------
// Typed convenience wrappers for atomic operations (GUPS-style)
// ---------------------------------------------------------------------------

#[deprecated = "Use Ep safe AMO methods instead (e.g., amo_fadd64 with reply_buffer on RequestParam)"]
#[allow(deprecated)]
/// Atomic fetch-and-add 32-bit.
///
/// # Safety
/// Caller must ensure `operand` points to a valid u32, `reply_buffer` has space for u32,
/// and `rkey` is valid.
pub unsafe fn atomic_fadd32(
    ep: &Ep,
    operand: u32,
    _reply_buffer: *mut u32,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u32>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep safe AMO methods instead"]
#[allow(deprecated)]
/// Atomic fetch-and-add 64-bit.
///
/// # Safety
/// Caller must ensure `operand` points to a valid u64, `reply_buffer` has space for u64,
/// and `rkey` is valid.
pub unsafe fn atomic_fadd64(
    ep: &Ep,
    operand: u64,
    _reply_buffer: *mut u64,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u64>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep safe AMO methods instead"]
#[allow(deprecated)]
/// Atomic fetch-and-swap 32-bit.
///
/// # Safety
/// Caller must ensure `operand` points to a valid u32, `reply_buffer` has space for u32,
/// and `rkey` is valid.
pub unsafe fn atomic_fswap32(
    ep: &Ep,
    operand: u32,
    _reply_buffer: *mut u32,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u32>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep safe AMO methods instead"]
#[allow(deprecated)]
/// Atomic fetch-and-swap 64-bit.
///
/// # Safety
/// Caller must ensure `operand` points to a valid u64, `reply_buffer` has space for u64,
/// and `rkey` is valid.
pub unsafe fn atomic_fswap64(
    ep: &Ep,
    operand: u64,
    _reply_buffer: *mut u64,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u64>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep safe AMO methods instead"]
#[allow(deprecated)]
/// Atomic compare-and-swap 32-bit.
///
/// Operand layout: `[expected, replacement]` as two consecutive u32 values.
///
/// # Safety
/// Caller must ensure `reply_buffer` has space for u32 and `rkey` is valid.
pub unsafe fn atomic_fcswap32(
    ep: &Ep,
    expected: u32,
    replacement: u32,
    _reply_buffer: *mut u32,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    let operand = [expected, replacement];
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
        operand.as_ptr() as *const std::os::raw::c_void,
        std::mem::size_of::<[u32; 2]>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep safe AMO methods instead"]
#[allow(deprecated)]
/// Atomic compare-and-swap 64-bit.
///
/// Operand layout: `[expected, replacement]` as two consecutive u64 values.
///
/// # Safety
/// Caller must ensure `reply_buffer` has space for u64 and `rkey` is valid.
pub unsafe fn atomic_fcswap64(
    ep: &Ep,
    expected: u64,
    replacement: u64,
    _reply_buffer: *mut u64,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    let operand = [expected, replacement];
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
        operand.as_ptr() as *const std::os::raw::c_void,
        std::mem::size_of::<[u64; 2]>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep::amo_add32 instead"]
#[allow(deprecated)]
/// Atomic add 32-bit (no fetch of old value).
///
/// # Safety
/// Caller must ensure `rkey` is valid.
pub unsafe fn atomic_add32(
    ep: &Ep,
    operand: u32,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u32>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep::amo_add64 instead"]
#[allow(deprecated)]
/// Atomic add 64-bit (no fetch of old value).
///
/// # Safety
/// Caller must ensure `rkey` is valid.
pub unsafe fn atomic_add64(
    ep: &Ep,
    operand: u64,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u64>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep::amo_swap32 instead"]
#[allow(deprecated)]
/// Atomic swap 32-bit (no fetch of old value).
///
/// # Safety
/// Caller must ensure `rkey` is valid.
pub unsafe fn atomic_swap32(
    ep: &Ep,
    operand: u32,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u32>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep::amo_swap64 instead"]
#[allow(deprecated)]
/// Atomic swap 64-bit (no fetch of old value).
///
/// # Safety
/// Caller must ensure `rkey` is valid.
pub unsafe fn atomic_swap64(
    ep: &Ep,
    operand: u64,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u64>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep safe AMO methods instead"]
#[allow(deprecated)]
/// Atomic fetch-and-xor 32-bit.
///
/// # Safety
/// Caller must ensure `operand` is valid, `reply_buffer` has space for u32,
/// and `rkey` is valid.
pub unsafe fn atomic_fxor32(
    ep: &Ep,
    operand: u32,
    _reply_buffer: *mut u32,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u32>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep safe AMO methods instead"]
#[allow(deprecated)]
/// Atomic fetch-and-xor 64-bit.
///
/// # Safety
/// Caller must ensure `operand` is valid, `reply_buffer` has space for u64,
/// and `rkey` is valid.
pub unsafe fn atomic_fxor64(
    ep: &Ep,
    operand: u64,
    _reply_buffer: *mut u64,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u64>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep::amo_xor32 instead"]
#[allow(deprecated)]
/// Atomic xor 32-bit (no fetch of old value).
///
/// # Safety
/// Caller must ensure `rkey` is valid.
pub unsafe fn atomic_xor32(
    ep: &Ep,
    operand: u32,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u32>(),
        remote_addr,
        rkey,
        param,
    )
}

#[deprecated = "Use Ep::amo_xor64 instead"]
#[allow(deprecated)]
/// Atomic xor 64-bit (no fetch of old value).
///
/// # Safety
/// Caller must ensure `rkey` is valid.
pub unsafe fn atomic_xor64(
    ep: &Ep,
    operand: u64,
    remote_addr: u64,
    rkey: &RemoteKey,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    atomic_op_nbx(
        ep,
        ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
        &operand as *const _ as *const std::os::raw::c_void,
        std::mem::size_of::<u64>(),
        remote_addr,
        rkey,
        param,
    )
}

#[cfg(test)]
#[allow(
    deprecated,
    clippy::let_unit_value,
    clippy::missing_transmute_annotations
)]
mod tests {
    use super::*;

    #[test]
    fn rkey_framing_round_trip_preserves_payload() {
        let payload = [0x12, 0x34, 0xab, 0xcd];
        let framed = frame_rkey_payload(&payload).unwrap();
        assert_eq!(&framed[..4], &(payload.len() as u32).to_le_bytes());
        assert_eq!(unframe_rkey_payload(&framed).unwrap(), payload);
    }

    #[test]
    fn rkey_unpack_accepts_public_framed_bytes() {
        let payload = [0x12, 0x34, 0xab, 0xcd];
        let framed = frame_rkey_payload(&payload).unwrap();
        assert_eq!(unframe_rkey_payload(&framed).unwrap(), payload);
        let unpack: fn(&Ep, &[u8]) -> Result<RemoteKey, ucs_status_t> = RemoteKey::unpack;
        let _ = unpack;
    }

    #[test]
    fn remote_key_compare_api_signature() {
        let _: for<'a> fn(&'a RemoteKey, &'a RemoteKey, &'a Worker) -> Result<bool, ucs_status_t> =
            RemoteKey::compare;
    }

    #[test]
    fn remote_key_compare_rejects_dead_worker_before_ffi() {
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let first = RemoteKey {
            handle: std::ptr::null_mut(),
            worker_alive: Arc::clone(&alive),
        };
        let second = RemoteKey {
            handle: std::ptr::null_mut(),
            worker_alive: Arc::clone(&alive),
        };
        let worker = Worker {
            am_handlers: Vec::new(),
            handle: std::ptr::null_mut(),
            alive,
            #[cfg(debug_assertions)]
            progressing: std::sync::atomic::AtomicBool::new(false),
        };

        assert_eq!(
            first.compare(&second, &worker),
            Err(ucs_status_t::UCS_ERR_INVALID_PARAM)
        );
        std::mem::forget(worker);
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn fetch_amo_signatures_require_reply_output() {
        let _: for<'w, 'a> fn(
            &Ep,
            &'w Worker,
            u64,
            u64,
            &RemoteKey,
            &'a mut u64,
        ) -> Result<FetchAmoRequest<'w, 'a, u64>, ucs_status_t> = Ep::amo_fadd64;
        let _: for<'w, 'a> fn(
            &Ep,
            &'w Worker,
            u64,
            u64,
            &RemoteKey,
            &'a mut u64,
        ) -> Result<FetchAmoRequest<'w, 'a, u64>, ucs_status_t> = Ep::amo_fxor64;
        let _: for<'w, 'a> fn(
            &Ep,
            &'w Worker,
            u64,
            u64,
            &RemoteKey,
            &'a mut u64,
        ) -> Result<FetchAmoRequest<'w, 'a, u64>, ucs_status_t> = Ep::amo_fswap64;
        let _: for<'w, 'a> fn(
            &Ep,
            &'w Worker,
            u64,
            u64,
            u64,
            &RemoteKey,
            &'a mut u64,
        ) -> Result<FetchAmoRequest<'w, 'a, u64>, ucs_status_t> = Ep::amo_fcswap64;
        let _: for<'w, 'a> fn(
            &Ep,
            &'w Worker,
            u32,
            u64,
            &RemoteKey,
            &'a mut u32,
        ) -> Result<FetchAmoRequest<'w, 'a, u32>, ucs_status_t> = Ep::amo_fadd32;
        let _: for<'w, 'a> fn(
            &Ep,
            &'w Worker,
            u32,
            u64,
            &RemoteKey,
            &'a mut u32,
        ) -> Result<FetchAmoRequest<'w, 'a, u32>, ucs_status_t> = Ep::amo_fxor32;
        let _: for<'w, 'a> fn(
            &Ep,
            &'w Worker,
            u32,
            u64,
            &RemoteKey,
            &'a mut u32,
        ) -> Result<FetchAmoRequest<'w, 'a, u32>, ucs_status_t> = Ep::amo_fswap32;
        let _: for<'w, 'a> fn(
            &Ep,
            &'w Worker,
            u32,
            u32,
            u64,
            &RemoteKey,
            &'a mut u32,
        ) -> Result<FetchAmoRequest<'w, 'a, u32>, ucs_status_t> = Ep::amo_fcswap32;
    }

    /// Test with invalid rkey — this segfaults on some UCX versions instead of
    /// returning an error. The UCX library calls into the rkey internals without
    /// Regression test: calling rkey_ptr with null rkey now returns an error
    /// instead of segfaulting. The Rust wrapper guards against null rkeys
    /// before calling the C library.
    ///
    /// Root cause: `ucp_rkey_ptr` dereferences the rkey handle before validating it.
    /// The Rust wrapper now checks `rkey.is_null()` and returns `UCS_ERR_INVALID_PARAM`.
    #[test]
    fn test_rkey_ptr_invalid() {
        let result = unsafe { rkey_ptr(std::ptr::null_mut(), 0) };
        assert!(
            result.is_err(),
            "Expected error for null rkey, got {:?}",
            result
        );
        assert_eq!(
            result.unwrap_err(),
            ucs_status_t::UCS_ERR_INVALID_PARAM,
            "Expected UCS_ERR_INVALID_PARAM for null rkey"
        );
    }

    /// Structural test: verify rkey_ptr function exists in FFI.
    #[test]
    fn test_rkey_ptr_signature() {
        let _: for<'a> fn(&'a mut RemoteKey, u64, usize) -> Result<&'a mut [u8], ucs_status_t> =
            RemoteKey::rkey_ptr;
        // Verify the FFI function is accessible — just check it compiles
        extern "C" {
            fn ucp_rkey_ptr(
                rkey: &RemoteKey,
                raddr: u64,
                addr_p: *mut *mut std::os::raw::c_void,
            ) -> ucs_status_t;
        }
        // Function exists and has correct signature
        let _ = unsafe { std::mem::transmute::<_, ()>(ucp_rkey_ptr) };
    }
}
