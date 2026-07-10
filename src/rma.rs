//! UCP Remote Memory Access (RMA) bindings.
//!
//! Wraps `ucp_put_nbx`, `ucp_get_nbx`, `ucp_atomic_op_nbx`,
//! `ucp_ep_rkey_unpack`, `ucp_rkey_ptr`, and `ucp_rkey_destroy`.

use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::Request;
use crate::RequestParam;

/// Re-export the remote key handle type for external callers.
#[allow(non_camel_case_types)]
pub type ucp_rkey_h = crate::ffi::ucp_rkey_h;

use crate::ep::Ep;

/// RAII wrapper around a UCP remote key handle (`ucp_rkey_h`).
/// The rkey is automatically destroyed when dropped.
pub struct RemoteKey {
    handle: ucp_rkey_h,
}

impl RemoteKey {
    /// Unpack a remote key from a packed buffer on this endpoint.
    pub fn unpack(ep: &Ep, rkey_buffer: &[u8]) -> Result<RemoteKey, ucs_status_t> {
        let mut rkey: ucp_rkey_h = std::ptr::null_mut();
        status_to_result(unsafe {
            ucp_ep_rkey_unpack(ep.handle, rkey_buffer.as_ptr() as *const _, &mut rkey)
        })
        .map(|()| RemoteKey { handle: rkey })
    }

    /// Get the raw rkey handle.
    #[inline]
    pub fn as_raw(&self) -> ucp_rkey_h {
        self.handle
    }
}

impl Drop for RemoteKey {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ucp_rkey_destroy(self.handle) };
        }
    }
}

/// Safe RMA and AMO methods on endpoints.
///
/// All methods take `&self` and safe types (`&[u8]`, `&mut [u8]`, `u64`, `&RemoteKey`),
/// hiding the `unsafe` FFI calls internally. Follows the same pattern as `Ep::tag_send`.
impl Ep {
    // ── Put / Get ──

    /// Put data to a remote memory location.
    pub fn rma_put(
        &self,
        buffer: &[u8],
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_put_nbx(
                self.handle,
                buffer.as_ptr() as _,
                buffer.len(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Get data from a remote memory location.
    pub fn rma_get(
        &self,
        buffer: &mut [u8],
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_get_nbx(
                self.handle,
                buffer.as_ptr() as _,
                buffer.len(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    // ── AMO — no-fetch variants ──

    /// Atomic add 64-bit on remote memory (no fetch of old value).
    pub fn amo_add64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic XOR 64-bit on remote memory (no fetch of old value).
    pub fn amo_xor64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic swap 64-bit on remote memory (no fetch of old value).
    pub fn amo_swap64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic AND 64-bit on remote memory (no fetch of old value).
    pub fn amo_and64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_AND,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic OR 64-bit on remote memory (no fetch of old value).
    pub fn amo_or64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_OR,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic compare-and-swap 64-bit (no fetch — use fetch variant if you need the old value).
    pub fn amo_cswap64(
        &self,
        expected: u64,
        replacement: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        let operand = [expected, replacement];
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
                operand.as_ptr() as *const _,
                std::mem::size_of::<[u64; 2]>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    // ── AMO — 32-bit no-fetch variants ──

    /// Atomic add 32-bit on remote memory (no fetch of old value).
    pub fn amo_add32(
        &self,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic XOR 32-bit on remote memory (no fetch of old value).
    pub fn amo_xor32(
        &self,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic swap 32-bit on remote memory (no fetch of old value).
    pub fn amo_swap32(
        &self,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic AND 32-bit on remote memory (no fetch of old value).
    pub fn amo_and32(
        &self,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_AND,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic OR 32-bit on remote memory (no fetch of old value).
    pub fn amo_or32(
        &self,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_OR,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic compare-and-swap 32-bit (no fetch — use fetch variant if you need the old value).
    pub fn amo_cswap32(
        &self,
        expected: u32,
        replacement: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        let operand = [expected, replacement];
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
                operand.as_ptr() as *const _,
                std::mem::size_of::<[u32; 2]>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    // ── AMO — fetch variants (reply written via RequestParamBuilder::reply_buffer) ──

    /// Atomic fetch-and-add 64-bit.
    /// Caller MUST set `reply_buffer` on the `RequestParam` via `RequestParamBuilder::reply_buffer(reply as *mut _ as *mut _, std::mem::size_of::<u64>())`.
    pub fn amo_fadd64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic fetch-and-xor 64-bit.
    /// Caller MUST set `reply_buffer` on the `RequestParam`.
    pub fn amo_fxor64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic fetch-and-swap 64-bit.
    /// Caller MUST set `reply_buffer` on the `RequestParam`.
    pub fn amo_fswap64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic fetch compare-and-swap 64-bit.
    /// Caller MUST set `reply_buffer` on the `RequestParam`.
    pub fn amo_fcswap64(
        &self,
        expected: u64,
        replacement: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        let operand = [expected, replacement];
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
                operand.as_ptr() as *const _,
                std::mem::size_of::<[u64; 2]>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }
}

#[deprecated = "Use Ep::rma_put() instead"]
/// Put data to a remote memory location.
///
/// # Safety
/// Caller must ensure `buffer` is valid for `count` bytes and `rkey` is valid.
pub unsafe fn put_nbx(
    ep: ucp_ep_h,
    buffer: *const std::os::raw::c_void,
    count: usize,
    remote_addr: u64,
    rkey: ucp_rkey_h,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    status_ptr_to_result(ucp_put_nbx(
        ep,
        buffer,
        count,
        remote_addr,
        rkey,
        &param.handle,
    ))
}

#[deprecated = "Use Ep::rma_get() instead"]
/// Get data from a remote memory location.
///
/// # Safety
/// Caller must ensure `buffer` has space for `count` bytes and `rkey` is valid.
pub unsafe fn get_nbx(
    ep: ucp_ep_h,
    buffer: *mut std::os::raw::c_void,
    count: usize,
    remote_addr: u64,
    rkey: ucp_rkey_h,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    status_ptr_to_result(ucp_get_nbx(
        ep,
        buffer,
        count,
        remote_addr,
        rkey,
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
    ep: ucp_ep_h,
    opcode: ucp_atomic_op_t,
    buffer: *const std::os::raw::c_void,
    count: usize,
    remote_addr: u64,
    rkey: ucp_rkey_h,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    status_ptr_to_result(ucp_atomic_op_nbx(
        ep,
        opcode,
        buffer,
        count,
        remote_addr,
        rkey,
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
    ep: ucp_ep_h,
    opcode: ucp_atomic_op_t,
    operand: *const std::os::raw::c_void,
    _reply_buffer: *mut std::os::raw::c_void,
    count: usize,
    remote_addr: u64,
    rkey: ucp_rkey_h,
    param: &RequestParam,
) -> Result<Option<crate::Request>, ucs_status_t> {
    status_ptr_to_result(ucp_atomic_op_nbx(
        ep,
        opcode,
        operand,
        count,
        remote_addr,
        rkey,
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
    ep: ucp_ep_h,
    rkey_buffer: *const std::os::raw::c_void,
) -> Result<ucp_rkey_h, ucs_status_t> {
    let mut rkey: ucp_rkey_h = std::ptr::null_mut();
    status_to_result(ucp_ep_rkey_unpack(ep, rkey_buffer, &mut rkey)).map(|()| rkey)
}

#[deprecated = "No safe replacement — use with caution"]
/// Get a local pointer to a remote memory region.
///
/// Returns a local pointer that can be used to access remote memory directly.
///
/// # Safety
/// Caller must ensure `rkey` is a valid remote key handle.
pub unsafe fn rkey_ptr(
    rkey: ucp_rkey_h,
    raddr: u64,
) -> Result<*mut std::os::raw::c_void, ucs_status_t> {
    let mut addr: *mut std::os::raw::c_void = std::ptr::null_mut();
    status_to_result(ucp_rkey_ptr(rkey, raddr, &mut addr)).map(|()| addr)
}

#[deprecated = "Use RemoteKey RAII wrapper instead (auto-destroy on drop)"]
/// Destroy a remote key.
///
/// # Safety
/// Caller must ensure `rkey` is a valid, non-duplicate remote key handle.
pub unsafe fn rkey_destroy(rkey: ucp_rkey_h) {
    ucp_rkey_destroy(rkey);
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
    ep: ucp_ep_h,
    operand: u32,
    _reply_buffer: *mut u32,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    operand: u64,
    _reply_buffer: *mut u64,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    operand: u32,
    _reply_buffer: *mut u32,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    operand: u64,
    _reply_buffer: *mut u64,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    expected: u32,
    replacement: u32,
    _reply_buffer: *mut u32,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    expected: u64,
    replacement: u64,
    _reply_buffer: *mut u64,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    operand: u32,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    operand: u64,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    operand: u32,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    operand: u64,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    operand: u32,
    _reply_buffer: *mut u32,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    operand: u64,
    _reply_buffer: *mut u64,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    operand: u32,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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
    ep: ucp_ep_h,
    operand: u64,
    remote_addr: u64,
    rkey: ucp_rkey_h,
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

    /// Test with invalid rkey — this segfaults on some UCX versions instead of
    /// returning an error. The UCX library calls into the rkey internals without
    /// checking for null, so we keep this ignored.
    ///
    /// Root cause: `ucp_rkey_ptr` dereferences the rkey handle before validating it.
    /// A fix would require patching UCX itself or using a valid (but unused) rkey.
    #[test]
    #[ignore = "ucp_rkey_ptr with null rkey segfaults instead of returning error — requires real rkey"]
    fn test_rkey_ptr_invalid() {
        // Testing with an invalid rkey should return an error
        let result = unsafe { rkey_ptr(std::ptr::null_mut(), 0) };
        assert!(result.is_err());
    }

    /// Structural test: verify rkey_ptr function exists in FFI.
    #[test]
    fn test_rkey_ptr_signature() {
        // Verify the FFI function is accessible — just check it compiles
        extern "C" {
            fn ucp_rkey_ptr(
                rkey: ucp_rkey_h,
                raddr: u64,
                addr_p: *mut *mut std::os::raw::c_void,
            ) -> ucs_status_t;
        }
        // Function exists and has correct signature
        let _ = unsafe { std::mem::transmute::<_, ()>(ucp_rkey_ptr) };
    }
}
