use crate::ep::Ep;
use crate::worker::Worker;
use crate::ffi::*;
use crate::Request;
use crate::RequestParam;
use crate::status_ptr_to_result;

use super::RemoteKey;

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

    /// Put `len` bytes from an arbitrary address, including device (GPU) memory.
    ///
    /// The slice-based [`Ep::rma_put`] cannot express accelerator buffers — a device pointer
    /// must never be turned into a host `&[u8]`. See [`Ep::tag_send_ptr`] for the rationale.
    /// Set `RequestParamBuilder::memory_type(UCS_MEMORY_TYPE_CUDA)` for device buffers.
    ///
    /// # Safety
    ///
    /// - `ptr` must be valid for reads of `len` bytes and readable by UCX for the whole
    ///   operation (device allocation on the current device, or managed memory).
    /// - The memory must stay alive and unpublished until the request completes.
    /// - `len` must not exceed the underlying allocation.
    pub unsafe fn rma_put_ptr(
        &self,
        ptr: *const u8,
        len: usize,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_put_nbx(
                self.handle,
                ptr as _,
                len,
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Get `len` bytes into an arbitrary address, including device (GPU) memory.
    ///
    /// Counterpart of [`Ep::rma_put_ptr`].
    ///
    /// # Safety
    ///
    /// - `ptr` must be valid for writes of `len` bytes and writable by UCX for the whole
    ///   operation (device allocation on the current device, or managed memory).
    /// - The memory must stay alive and unpublished until the request completes.
    pub unsafe fn rma_get_ptr(
        &self,
        ptr: *mut u8,
        len: usize,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_get_nbx(
                self.handle,
                ptr as _,
                len,
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

    /// Atomic fetch-and-add 64-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fadd64<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u64,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, ucs_status_t> {
        let param = RequestParam::fetch_params(reply);
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
        .map(|request| super::fetch_amo_result(request, worker))
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
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, ucs_status_t> {
        let param = RequestParam::fetch_params(reply);
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
        .map(|request| super::fetch_amo_result(request, worker))
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
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, ucs_status_t> {
        let param = RequestParam::fetch_params(reply);
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
        .map(|request| super::fetch_amo_result(request, worker))
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
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, ucs_status_t> {
        let operand = [compare, swap];
        let param = RequestParam::fetch_params(reply);
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
        .map(|request| super::fetch_amo_result(request, worker))
    }

    /// Atomic fetch-and-add 32-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fadd32<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u32,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, ucs_status_t> {
        let param = RequestParam::fetch_params(reply);
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
        .map(|request| super::fetch_amo_result(request, worker))
    }

    /// Atomic fetch-and-xor 32-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fxor32<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u32,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, ucs_status_t> {
        let param = RequestParam::fetch_params(reply);
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
        .map(|request| super::fetch_amo_result(request, worker))
    }

    /// Atomic fetch-and-swap 32-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fswap32<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u32,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, ucs_status_t> {
        let param = RequestParam::fetch_params(reply);
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
        .map(|request| super::fetch_amo_result(request, worker))
    }

    /// Atomic fetch compare-and-swap 32-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fcswap32<'w, 'a>(
        &self,
        worker: &'w Worker,
        expected: u32,
        swap: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u32,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, ucs_status_t> {
        let operand = [expected, swap];
        let param = RequestParam::fetch_params(reply);
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
        .map(|request| super::fetch_amo_result(request, worker))
    }
}
