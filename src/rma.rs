//! UCP Remote Memory Access (RMA) bindings.
//!
//! Wraps `ucp_put_nbx`, `ucp_get_nbx`, `ucp_atomic_op_nbx`,
//! `ucp_ep_rkey_unpack`, `ucp_rkey_ptr`, and `ucp_rkey_destroy`.

use crate::context::Context;
use crate::ffi::*;
use crate::memh::MemHandle;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Request;
use crate::RequestParam;

/// Re-export the remote key handle type for external callers.
#[allow(non_camel_case_types)]
pub type ucp_rkey_h = crate::ffi::ucp_rkey_h;

use crate::ep::Ep;
use std::marker::PhantomData;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// A fetch-AMO completion tied to both its worker and reply buffer.
///
/// The reply buffer must remain valid until the request is resolved. Dropping
/// a pending value progresses its worker to completion before releasing the
/// reply borrow, unless the bounded progress loop times out; in that
/// pathological case the request is deliberately leaked and the reply buffer
/// must not be reused.
pub struct FetchAmoRequest<'w, 'a> {
    request: Option<crate::Request>,
    worker: &'w Worker,
    _reply: PhantomData<&'a mut u64>,
}

impl<'w, 'a> FetchAmoRequest<'w, 'a> {
    /// Extract the underlying UCX request without completing it.
    ///
    /// # Safety
    /// The returned request may be in flight and still own `reply_buffer` for
    /// the caller-provided reply buffer. The caller must keep the reply buffer
    /// valid until the returned request reaches completion
    /// (`check_finished() == Ok(true)`) and must not drop it (which calls
    /// `ucp_request_free`) while in flight. Prefer `check_finished()` + `free()`
    /// on the wrapper, or let `Drop` handle it.
    pub unsafe fn into_inner(mut self) -> Option<crate::Request> {
        self.request.take()
    }
    pub fn check_finished(&self) -> Result<bool, ucs_status_t> {
        self.request
            .as_ref()
            .map_or(Ok(true), crate::Request::check_finished)
    }
    /// Progress the request to completion before releasing its reply borrow.
    ///
    /// If completion cannot be observed within the bounded progress loop, the
    /// request is deliberately leaked rather than freed while it may still be
    /// in flight. This preserves the same safety contract as [`Drop`].
    pub fn free(mut self) {
        if let Some(request) = self.request.take() {
            const MAX_PROGRESS: usize = 1_000_000;
            for _ in 0..MAX_PROGRESS {
                match request.check_finished() {
                    Ok(true) | Err(_) => {
                        request.free();
                        return;
                    }
                    Ok(false) => {
                        self.worker.progress();
                    }
                }
            }
            // SAFETY: The request is still in flight. It must not be freed;
            // the caller must not reuse the reply buffer after this timeout.
            std::mem::forget(request);
        }
    }
}

impl Drop for FetchAmoRequest<'_, '_> {
    fn drop(&mut self) {
        if let Some(request) = self.request.take() {
            const MAX_PROGRESS: usize = 1_000_000;
            for _ in 0..MAX_PROGRESS {
                match request.check_finished() {
                    Ok(true) => {
                        request.free();
                        return;
                    }
                    Ok(false) => {
                        self.worker.progress();
                    }
                    Err(_) => {
                        request.free();
                        return;
                    }
                }
            }
            // SAFETY: The request is still in flight. It must not be freed;
            // the caller must not reuse the reply buffer after this timeout.
            std::mem::forget(request);
        }
    }
}

fn fetch_amo_result<'w, 'a>(
    request: Option<crate::Request>,
    worker: &'w Worker,
) -> FetchAmoRequest<'w, 'a> {
    FetchAmoRequest {
        request,
        worker,
        _reply: PhantomData,
    }
}

/// RAII wrapper around a UCP remote key handle (`ucp_rkey_h`).
/// The rkey is automatically destroyed when dropped.
pub struct RemoteKey {
    handle: ucp_rkey_h,
    worker_alive: Arc<std::sync::atomic::AtomicBool>,
}

fn frame_rkey_payload(payload: &[u8]) -> Result<Vec<u8>, ucs_status_t> {
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| ucs_status_t::UCS_ERR_OUT_OF_RANGE)?;
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&payload_len.to_le_bytes());
    framed.extend_from_slice(payload);
    Ok(framed)
}

fn unframe_rkey_payload(buffer: &[u8]) -> Result<&[u8], ucs_status_t> {
    if buffer.len() < 4 {
        return Err(ucs_status_t::UCS_ERR_INVALID_PARAM);
    }
    let payload_len = u32::from_le_bytes(buffer[..4].try_into().unwrap()) as usize;
    if payload_len != buffer.len() - 4 {
        return Err(ucs_status_t::UCS_ERR_INVALID_PARAM);
    }
    Ok(&buffer[4..])
}

impl RemoteKey {
    /// Pack a memory handle using the framed format consumed by [`Self::unpack`].
    ///
    /// The wire format is exactly `[4-byte little-endian payload length][payload]`.
    /// The payload is the opaque byte sequence returned by UCX's `ucp_rkey_pack`.
    pub fn pack(context: &Context, memh: &MemHandle) -> Result<Vec<u8>, ucs_status_t> {
        let mut buffer = std::ptr::null_mut();
        let mut size = 0usize;
        status_to_result(unsafe {
            ucp_rkey_pack(context.handle, memh.as_raw(), &mut buffer, &mut size)
        })?;
        let result =
            frame_rkey_payload(unsafe { std::slice::from_raw_parts(buffer as *const u8, size) });
        unsafe { ucp_rkey_buffer_release(buffer) };
        result
    }

    /// Unpack `[4-byte little-endian payload length][opaque UCX payload]`.
    pub fn unpack(ep: &Ep, rkey_buffer: &[u8]) -> Result<RemoteKey, ucs_status_t> {
        let payload = unframe_rkey_payload(rkey_buffer)?;
        let mut rkey: ucp_rkey_h = std::ptr::null_mut();
        status_to_result(unsafe {
            ucp_ep_rkey_unpack(ep.handle, payload.as_ptr() as *const _, &mut rkey)
        })
        .map(|()| RemoteKey {
            handle: rkey,
            worker_alive: Arc::clone(&ep.worker_alive),
        })
    }

    /// Get a local pointer to remote memory for intra-node one-sided access.
    ///
    /// UCX returns only a pointer, so `len` must be the caller's independently
    /// known valid length. The slice is valid only while this key and its remote
    /// allocation remain valid. The returned slice must not overlap any other
    /// live reference to that memory; the caller must ensure no other
    /// `rkey_ptr` slice or local reference aliases it. Remote writes through
    /// UCX RMA/AMO while the slice is borrowed are outside Rust's aliasing model;
    /// synchronize externally and do not hold overlapping `&mut` references
    /// across such operations.
    pub fn rkey_ptr(&mut self, remote_addr: u64, len: usize) -> Result<&mut [u8], ucs_status_t> {
        let mut addr = std::ptr::null_mut();
        status_to_result(unsafe { ucp_rkey_ptr(self.handle, remote_addr, &mut addr) })?;
        if addr.is_null() {
            return Err(ucs_status_t::UCS_ERR_INVALID_ADDR);
        }
        Ok(unsafe { std::slice::from_raw_parts_mut(addr as *mut u8, len) })
    }

    /// Get the raw rkey handle.
    #[inline]
    pub fn as_raw(&self) -> ucp_rkey_h {
        self.handle
    }

    /// Compare this key with another key belonging to the same worker.
    /// UCX returns zero when the keys refer to the same memory region.
    pub fn compare(&self, other: &RemoteKey, worker: &Worker) -> Result<bool, ucs_status_t> {
        if !self.worker_alive.load(Ordering::Acquire)
            || !other.worker_alive.load(Ordering::Acquire)
            || !Arc::ptr_eq(&self.worker_alive, &other.worker_alive)
            || !Arc::ptr_eq(&self.worker_alive, &worker.alive)
        {
            return Err(ucs_status_t::UCS_ERR_INVALID_PARAM);
        }
        let params = ucp_rkey_compare_params_t { field_mask: 0 };
        let mut result = 0;
        // SAFETY: the alive flags establish that both keys came from this live
        // worker, and the parameter and result storage live for the call.
        status_to_result(unsafe {
            ucp_rkey_compare(
                worker.handle,
                self.handle,
                other.handle,
                &params,
                &mut result,
            )
        })
        .map(|()| result == 0)
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

    // ── AMO — fetch variants ──

    fn fetch_param<T>(reply: &mut T) -> RequestParam {
        crate::RequestParamBuilder::new()
            .reply_buffer(reply as *mut T as *mut std::os::raw::c_void)
            .build()
    }

    /// Atomic fetch-and-add 64-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fadd64<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u64,
    ) -> Result<FetchAmoRequest<'w, 'a>, ucs_status_t> {
        let param = Self::fetch_param(reply);
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
        .map(|request| fetch_amo_result(request, worker))
    }

    /// Atomic fetch-and-xor 64-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fxor64<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u64,
    ) -> Result<FetchAmoRequest<'w, 'a>, ucs_status_t> {
        let param = Self::fetch_param(reply);
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
        .map(|request| fetch_amo_result(request, worker))
    }

    /// Atomic fetch-and-swap 64-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fswap64<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u64,
    ) -> Result<FetchAmoRequest<'w, 'a>, ucs_status_t> {
        let param = Self::fetch_param(reply);
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
        .map(|request| fetch_amo_result(request, worker))
    }

    /// Atomic fetch compare-and-swap 64-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fcswap64<'w, 'a>(
        &self,
        worker: &'w Worker,
        compare: u64,
        swap: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u64,
    ) -> Result<FetchAmoRequest<'w, 'a>, ucs_status_t> {
        let operand = [compare, swap];
        let param = Self::fetch_param(reply);
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
        .map(|request| fetch_amo_result(request, worker))
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
            handle: std::ptr::null_mut(),
            alive,
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
        ) -> Result<FetchAmoRequest<'w, 'a>, ucs_status_t> = Ep::amo_fadd64;
        let _: for<'w, 'a> fn(
            &Ep,
            &'w Worker,
            u64,
            u64,
            &RemoteKey,
            &'a mut u64,
        ) -> Result<FetchAmoRequest<'w, 'a>, ucs_status_t> = Ep::amo_fxor64;
        let _: for<'w, 'a> fn(
            &Ep,
            &'w Worker,
            u64,
            u64,
            &RemoteKey,
            &'a mut u64,
        ) -> Result<FetchAmoRequest<'w, 'a>, ucs_status_t> = Ep::amo_fswap64;
        let _: for<'w, 'a> fn(
            &Ep,
            &'w Worker,
            u64,
            u64,
            u64,
            &RemoteKey,
            &'a mut u64,
        ) -> Result<FetchAmoRequest<'w, 'a>, ucs_status_t> = Ep::amo_fcswap64;
    }

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
        let _: for<'a> fn(&'a mut RemoteKey, u64, usize) -> Result<&'a mut [u8], ucs_status_t> =
            RemoteKey::rkey_ptr;
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
