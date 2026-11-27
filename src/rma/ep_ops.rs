use crate::completion::{owned_op_param, status_ptr_with_data};
use crate::dt::dt_make_contig;
use crate::ep::Ep;
use crate::ffi::*;
use crate::request::Pending;
use crate::status_ptr_to_result;
use crate::worker::Worker;
use crate::RequestParam;
use crate::Status;
use crate::{Completion, OwnedBuf, OwnedBufMut, OwningRequest};

use super::RemoteKey;

/// Keep the required swap reply buffer alive until completion, discarding its value.
fn with_amo_reply<T>(
    param: &RequestParam,
    reply: T,
    post: impl FnOnce(&RequestParam) -> *mut std::ffi::c_void,
) -> Result<Pending, Status> {
    let mut reply = Box::new(reply);
    let mut inner = RequestParam::fetch_params(reply.as_mut());
    let reply_buffer = inner.handle.reply_buffer;
    inner.handle = param.handle;
    inner.handle.reply_buffer = reply_buffer;
    let callback = if param.handle.op_attr_mask & (1 << 1) != 0 {
        unsafe { param.handle.cb.send }
    } else {
        None
    };
    let user_data = if param.handle.op_attr_mask & (1 << 2) != 0 {
        param.handle.user_data
    } else {
        std::ptr::null_mut()
    };
    let completion = Box::new(move |request, status| {
        drop(reply);
        if let Some(callback) = callback {
            unsafe { callback(request, status, user_data) };
        }
    });
    let callback = amo_reply_callback(&*completion);
    let completion = Box::into_raw(completion);
    // UCP_OP_ATTR_FIELD_CALLBACK, USER_DATA, and REPLY_BUFFER.
    inner.handle.op_attr_mask |= (1 << 1) | (1 << 2) | (1 << 5);
    inner.handle.cb.send = Some(callback);
    inner.handle.user_data = completion.cast();
    let result = status_ptr_to_result(post(&inner));
    // UCX does not invoke the callback for immediate completion or posting errors.
    if !matches!(result, Ok(Pending::InProgress(_))) {
        unsafe { drop(Box::from_raw(completion)) };
    }
    result
}

/// Rust equivalent of UCX's `ucp_dt_make_contig` C macro.
fn ucp_dt_make_contig(size: usize) -> u64 {
    (size as u64) << 3
}

fn amo_reply_callback<S, F: FnOnce(*mut std::ffi::c_void, S)>(
    _: &F,
) -> unsafe extern "C" fn(*mut std::ffi::c_void, S, *mut std::ffi::c_void) {
    unsafe extern "C" fn complete<S, F: FnOnce(*mut std::ffi::c_void, S)>(
        request: *mut std::ffi::c_void,
        status: S,
        user_data: *mut std::ffi::c_void,
    ) {
        let callback = unsafe { Box::from_raw(user_data.cast::<F>()) };
        callback(request, status);
    }
    complete::<S, F>
}

/// Safe RMA and AMO methods on endpoints.
///
/// All methods take `&self` and safe types (`&[u8]`, `&mut [u8]`, `u64`, `&RemoteKey`),
/// hiding the `unsafe` FFI calls internally. Follows the same pattern as `Ep::tag_send`.
impl Ep {
    // ── Put / Get ──

    /// Put `buffer` to a remote memory location, taking ownership of it.
    ///
    /// Returns [`Completion::Immediate`] with the buffer when UCX finishes
    /// inline, or [`Completion::Pending`] with an [`OwningRequest`] that hands
    /// the buffer back from [`OwningRequest::try_complete`]. Local completion
    /// only means the source may be reused; flush before relying on remote
    /// visibility.
    ///
    /// `param` contributes flags, callback, and user data. The datatype
    /// (always contiguous bytes), reply buffer, memory handle, memory type, and
    /// request allocation fields are controlled by this method and ignored if
    /// set. A callback may run before this returns, must not progress the
    /// worker, and must not free the request. An empty buffer always completes
    /// immediately, even with `NO_IMM_CMPL`.
    ///
    /// ```no_run
    /// # use ucx_sys::{ep::Ep, rma::RemoteKey, worker::Worker, RequestParamBuilder, Completion};
    /// # fn demo(ep: &Ep, worker: &Worker, rkey: &RemoteKey, addr: u64) {
    /// let param = RequestParamBuilder::new().build();
    /// match ep.rma_put(worker, b"hello".to_vec(), addr, rkey, &param).unwrap() {
    ///     Completion::Immediate(buffer) => drop(buffer),
    ///     Completion::Pending(request) => { /* progress + try_complete */ }
    /// }
    /// # }
    /// ```
    pub fn rma_put<'w, D: OwnedBuf>(
        &self,
        worker: &'w Worker,
        buffer: D,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Completion<D, OwningRequest<'w, D>>, Status> {
        let raw = owned_op_param(param);
        // SAFETY: the request (if any) reads only `buffer`'s stable storage
        // (`OwnedBuf`), which the returned value owns until UCX is done; the
        // contiguous-bytes datatype bounds the access to `buffer.len()`.
        unsafe {
            let ptr = ucp_put_nbx(
                self.handle,
                buffer.as_ptr() as _,
                buffer.len(),
                remote_addr,
                rkey.handle,
                &raw,
            );
            status_ptr_with_data(ptr, buffer, worker)
        }
    }

    /// Get remote memory into `buffer`, taking ownership of it.
    ///
    /// The filled buffer is returned through [`Completion::Immediate`] or
    /// [`OwningRequest::try_complete`]. On failure it comes back inside
    /// [`crate::Failed`] with unspecified contents. `param` is handled as in
    /// [`Ep::rma_put`].
    pub fn rma_get<'w, D: OwnedBufMut>(
        &self,
        worker: &'w Worker,
        mut buffer: D,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Completion<D, OwningRequest<'w, D>>, Status> {
        let raw = owned_op_param(param);
        // SAFETY: as in `rma_put`, for writes into `buffer`'s stable storage.
        unsafe {
            let ptr = ucp_get_nbx(
                self.handle,
                buffer.as_mut_ptr() as _,
                buffer.len(),
                remote_addr,
                rkey.handle,
                &raw,
            );
            status_ptr_with_data(ptr, buffer, worker)
        }
    }

    /// Put `len` bytes from an arbitrary address, including device (GPU) memory.
    ///
    /// The slice-based [`Ep::rma_put`] cannot express accelerator buffers — a device pointer
    /// must never be turned into a host `&[u8]`. See [`Ep::tag_send_ptr`] for the rationale.
    /// Set `RequestParamBuilder::memory_type(MemoryType::Cuda)` for device buffers.
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
    ) -> Result<Pending, Status> {
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
    ) -> Result<Pending, Status> {
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
    ) -> Result<Pending, Status> {
        status_ptr_to_result(unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&operand));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
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
    ) -> Result<Pending, Status> {
        status_ptr_to_result(unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&operand));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
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
    ) -> Result<Pending, Status> {
        with_amo_reply(param, 0u64, |param| unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&operand));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
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
    ) -> Result<Pending, Status> {
        status_ptr_to_result(unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&operand));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_AND,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
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
    ) -> Result<Pending, Status> {
        status_ptr_to_result(unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&operand));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_OR,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
            )
        })
    }

    /// Atomic compare-and-swap 64-bit (no fetch — use fetch variant if you need the old value).
    ///
    /// The remote value is replaced with `replacement` only when it equals `expected`.
    /// UCX CSWAP uses `buffer` for the compare value (`expected`) and
    /// `reply_buffer` for the swap value (`replacement`) on input and the old value
    /// on completion. The reply buffer is kept alive until completion and discarded.
    pub fn amo_cswap64(
        &self,
        expected: u64,
        replacement: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Pending, Status> {
        with_amo_reply(param, replacement, |param| unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&expected));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
                &expected as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
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
    ) -> Result<Pending, Status> {
        status_ptr_to_result(unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&operand));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
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
    ) -> Result<Pending, Status> {
        status_ptr_to_result(unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&operand));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
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
    ) -> Result<Pending, Status> {
        with_amo_reply(param, 0u32, |param| unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&operand));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
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
    ) -> Result<Pending, Status> {
        status_ptr_to_result(unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&operand));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_AND,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
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
    ) -> Result<Pending, Status> {
        status_ptr_to_result(unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&operand));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_OR,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
            )
        })
    }

    /// Atomic compare-and-swap 32-bit (no fetch — use fetch variant if you need the old value).
    ///
    /// The remote value is replaced with `replacement` only when it equals `expected`.
    /// UCX CSWAP uses `buffer` for the compare value (`expected`) and
    /// `reply_buffer` for the swap value (`replacement`) on input and the old value
    /// on completion. The reply buffer is kept alive until completion and discarded.
    pub fn amo_cswap32(
        &self,
        expected: u32,
        replacement: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Pending, Status> {
        with_amo_reply(param, replacement, |param| unsafe {
            let datatype = ucp_dt_make_contig(std::mem::size_of_val(&expected));
            let mut param = param.handle;
            param.op_attr_mask |= 1 << 3; // UCP_OP_ATTR_FIELD_DATATYPE
            param.datatype = datatype;
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
                &expected as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param,
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
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, Status> {
        let mut param = RequestParam::fetch_params(reply);
        param.handle.op_attr_mask |= 1 << 3; // Datatype field.
        param.handle.datatype = dt_make_contig(std::mem::size_of::<u64>());
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| match request {
            Pending::InProgress(request) => super::fetch_amo_result(Some(request), worker),
            _ => super::fetch_amo_result(None, worker),
        })
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
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, Status> {
        let mut param = RequestParam::fetch_params(reply);
        param.handle.op_attr_mask |= 1 << 3; // Datatype field.
        param.handle.datatype = dt_make_contig(std::mem::size_of::<u64>());
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| match request {
            Pending::InProgress(request) => super::fetch_amo_result(Some(request), worker),
            _ => super::fetch_amo_result(None, worker),
        })
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
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, Status> {
        let mut param = RequestParam::fetch_params(reply);
        param.handle.op_attr_mask |= 1 << 3; // Datatype field.
        param.handle.datatype = dt_make_contig(std::mem::size_of::<u64>());
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| match request {
            Pending::InProgress(request) => super::fetch_amo_result(Some(request), worker),
            _ => super::fetch_amo_result(None, worker),
        })
    }

    /// Atomic fetch compare-and-swap 64-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    ///
    /// The remote value is replaced with `swap` only when it equals `compare`.
    /// UCX CSWAP uses `buffer` for the compare value and `reply_buffer` for
    /// the swap value on input and the old remote value on output. This method
    /// writes `swap` into `*reply` before posting; on completion, `reply` holds
    /// the old value whether or not the swap occurred.
    pub fn amo_fcswap64<'w, 'a>(
        &self,
        worker: &'w Worker,
        compare: u64,
        swap: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u64,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, Status> {
        *reply = swap;
        let mut param = RequestParam::fetch_params(reply);
        param.handle.op_attr_mask |= 1 << 3; // Datatype field.
        param.handle.datatype = dt_make_contig(std::mem::size_of::<u64>());
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
                &compare as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| match request {
            Pending::InProgress(request) => super::fetch_amo_result(Some(request), worker),
            _ => super::fetch_amo_result(None, worker),
        })
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
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, Status> {
        let mut param = RequestParam::fetch_params(reply);
        param.handle.op_attr_mask |= 1 << 3; // Datatype field.
        param.handle.datatype = dt_make_contig(std::mem::size_of::<u32>());
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| match request {
            Pending::InProgress(request) => super::fetch_amo_result(Some(request), worker),
            _ => super::fetch_amo_result(None, worker),
        })
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
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, Status> {
        let mut param = RequestParam::fetch_params(reply);
        param.handle.op_attr_mask |= 1 << 3; // Datatype field.
        param.handle.datatype = dt_make_contig(std::mem::size_of::<u32>());
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| match request {
            Pending::InProgress(request) => super::fetch_amo_result(Some(request), worker),
            _ => super::fetch_amo_result(None, worker),
        })
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
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, Status> {
        let mut param = RequestParam::fetch_params(reply);
        param.handle.op_attr_mask |= 1 << 3; // Datatype field.
        param.handle.datatype = dt_make_contig(std::mem::size_of::<u32>());
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
                &operand as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| match request {
            Pending::InProgress(request) => super::fetch_amo_result(Some(request), worker),
            _ => super::fetch_amo_result(None, worker),
        })
    }

    /// Atomic fetch compare-and-swap 32-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    ///
    /// The remote value is replaced with `swap` only when it equals `expected`.
    /// UCX CSWAP uses `buffer` for the compare value (`expected`) and `reply_buffer`
    /// for the swap value on input and the old remote value on output. This method
    /// writes `swap` into `*reply` before posting; on completion, `reply` holds
    /// the old value whether or not the swap occurred.
    pub fn amo_fcswap32<'w, 'a>(
        &self,
        worker: &'w Worker,
        expected: u32,
        swap: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u32,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, Status> {
        *reply = swap;
        let mut param = RequestParam::fetch_params(reply);
        param.handle.op_attr_mask |= 1 << 3; // Datatype field.
        param.handle.datatype = dt_make_contig(std::mem::size_of::<u32>());
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
                &expected as *const _ as *const _,
                1,
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| match request {
            Pending::InProgress(request) => super::fetch_amo_result(Some(request), worker),
            _ => super::fetch_amo_result(None, worker),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::context::{Config, Context, Flags, ParamsBuilder};
    use crate::ep::{Ep, ParamsBuilder as EpParamsBuilder};
    use crate::memh::MemHandle;
    use crate::request::Pending;
    use crate::rma::RemoteKey;
    use crate::worker::{ParamsBuilder as WorkerParamsBuilder, RemoteWorkerAddress, Worker};
    use crate::RequestParamBuilder;

    /// UCX version and transport selection, for failure messages.
    fn transport() -> String {
        format!(
            "UCX {}, UCX_TLS={:?}",
            crate::version::get_version_string(),
            std::env::var("UCX_TLS").ok()
        )
    }

    /// Progress `worker` until `request` completes. Bounded, so a transport that
    /// cannot complete the operation fails the test instead of hanging it.
    fn wait(worker: &Worker, request: Pending, what: &str) {
        let Pending::InProgress(request) = request else {
            return;
        };
        for _ in 0..1_000_000 {
            match request.check_finished() {
                Ok(true) => return,
                Ok(false) => {
                    worker.progress();
                }
                Err(status) => panic!("{what} failed with {status} ({})", transport()),
            }
        }
        panic!("{what} did not complete ({})", transport());
    }

    use crate::{Completion, OwningRequest};

    /// Test-only driver: progress until an owned RMA completion returns its data.
    fn drive_owned<D>(
        worker: &Worker,
        completion: Completion<D, OwningRequest<'_, D>>,
        what: &str,
    ) -> D {
        let mut request = match completion {
            Completion::Immediate(data) => return data,
            Completion::Pending(request) => request,
        };
        for _ in 0..1_000_000 {
            worker.progress();
            match request.try_complete() {
                Ok(Ok(data)) => return data,
                Ok(Err(failed)) => panic!("{what} failed with {} ({})", failed.status, transport()),
                Err(still) => request = still,
            }
        }
        panic!("{what} did not complete ({})", transport());
    }

    /// Self endpoint plus an rkey over `target`.
    fn rma_self_setup(
        target: &mut [u8],
    ) -> (
        Context,
        Worker,
        Ep,
        crate::memh::MemHandleGuard<'_>,
        Vec<u8>,
    ) {
        let params = ParamsBuilder::new()
            .features(Flags::Rma)
            .mt_workers_shared(1)
            .estimated_num_eps(1)
            .build();
        let mut context =
            Context::new(&Config::read("", "").expect("config"), &params).expect("context");
        let worker = context
            .worker_create(&WorkerParamsBuilder::new().build())
            .expect("worker");
        let address = worker.pack_address().expect("pack address");
        let remote = RemoteWorkerAddress::new(address.to_vec());
        drop(address);
        let ep = worker
            .create_ep(EpParamsBuilder::new().address(&remote).build())
            .expect("endpoint");
        let memh = MemHandle::map_slice(&context, target, 0).expect("map target");
        let packed = RemoteKey::pack(&context, memh.mem_handle())
            .expect("pack rkey")
            .to_vec();
        (context, worker, ep, memh, packed)
    }

    /// `NO_IMM_CMPL` forces the deferred path: both put and get return an
    /// `OwningRequest` that hands the buffer back from `try_complete`.
    #[test]
    fn rma_owned_put_get_pending_path_returns_buffers() {
        let mut target = vec![0u8; 64];
        let target_addr = target.as_mut_ptr() as u64;
        let (_context, worker, ep, memh, packed) = rma_self_setup(&mut target);
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().no_imm_cmpl().build();
        let payload: Vec<u8> = (0..64u8).collect();

        let put = ep
            .rma_put(
                &worker,
                payload.clone().into_boxed_slice(),
                target_addr,
                &rkey,
                &param,
            )
            .expect("post put");
        assert!(put.is_pending(), "NO_IMM_CMPL put must be deferred");
        assert_eq!(&*drive_owned(&worker, put, "put"), &payload[..]);
        let flush = ep
            .flush(&RequestParamBuilder::new().build())
            .expect("flush");
        wait(&worker, flush, "endpoint flush");

        let get = ep
            .rma_get(&worker, Box::new([0u8; 64]), target_addr, &rkey, &param)
            .expect("post get");
        assert!(get.is_pending(), "NO_IMM_CMPL get must be deferred");
        let fetched = drive_owned(&worker, get, "get");
        assert_eq!(&fetched[..], &payload[..]);
        drop(rkey);
        drop(memh);
        assert_eq!(target, payload);
        ep.close(&worker, 0).expect("close endpoint");
    }

    /// `FORCE_IMM_CMPL` must keep working: a small self put completes inline
    /// and returns the buffer (UCX's short-put path must not be disabled by
    /// the owned param handling).
    #[test]
    fn rma_owned_put_force_imm_cmpl_completes_inline() {
        let mut target = vec![0u8; 8];
        let target_addr = target.as_mut_ptr() as u64;
        let (_context, worker, ep, memh, packed) = rma_self_setup(&mut target);
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().force_imm_cmpl().build();
        let put = ep
            .rma_put(&worker, vec![0x42u8; 8], target_addr, &rkey, &param)
            .unwrap_or_else(|status| panic!("force_imm put: {status} ({})", transport()));
        assert_eq!(
            put.into_immediate().expect("must complete inline"),
            vec![0x42u8; 8]
        );
        let flush = ep
            .flush(&RequestParamBuilder::new().build())
            .expect("flush");
        wait(&worker, flush, "endpoint flush");
        drop(rkey);
        drop(memh);
        assert_eq!(target, vec![0x42u8; 8]);
        ep.close(&worker, 0).expect("close endpoint");
    }

    /// The owned API forces contiguous bytes: a caller datatype of 2-byte
    /// elements must not make UCX write `2 * len` bytes past the buffer.
    #[test]
    fn rma_owned_put_ignores_caller_datatype() {
        let mut target = vec![0xEEu8; 16];
        let target_addr = target.as_mut_ptr() as u64;
        let (_context, worker, ep, memh, packed) = rma_self_setup(&mut target);
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new()
            .datatype(crate::dt::dt_make_contig(2))
            .build();
        let put = ep
            .rma_put(&worker, vec![0x11u8; 8], target_addr, &rkey, &param)
            .expect("post put");
        drive_owned(&worker, put, "put");
        let flush = ep
            .flush(&RequestParamBuilder::new().build())
            .expect("flush");
        wait(&worker, flush, "endpoint flush");
        drop(rkey);
        drop(memh);
        assert_eq!(&target[..8], &[0x11u8; 8]);
        assert_eq!(
            &target[8..],
            &[0xEEu8; 8],
            "bytes past the buffer untouched"
        );
        ep.close(&worker, 0).expect("close endpoint");
    }

    /// Dropping a pending owned get is safe: the fail-safe completes it before
    /// the buffer is freed (no use-after-free from safe code, #112).
    #[test]
    fn rma_owned_get_drop_while_pending_is_safe() {
        let mut target = vec![7u8; 4096];
        let target_addr = target.as_mut_ptr() as u64;
        let (_context, worker, ep, memh, packed) = rma_self_setup(&mut target);
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().no_imm_cmpl().build();
        let get = ep
            .rma_get(&worker, vec![0u8; 4096], target_addr, &rkey, &param)
            .expect("post get");
        assert!(
            get.is_pending(),
            "NO_IMM_CMPL get must exercise the fail-safe"
        );
        drop(get);
        drop(rkey);
        drop(memh);
        ep.close(&worker, 0).expect("close endpoint");
    }

    fn setup_amo_test() -> (Context, Worker, Ep) {
        // UCP_FEATURE_AMO32 and UCP_FEATURE_AMO64.
        let params = ParamsBuilder::new()
            .features(Flags::Rma | Flags::Amo32 | Flags::Amo64)
            .mt_workers_shared(1)
            .estimated_num_eps(1)
            .build();
        let mut context =
            Context::new(&Config::read("", "").expect("config"), &params).expect("context");
        let worker = context
            .worker_create(&WorkerParamsBuilder::new().build())
            .expect("worker");
        let address = worker.pack_address().expect("pack address");
        let remote = RemoteWorkerAddress::new(address.to_vec());
        let ep = worker
            .create_ep(EpParamsBuilder::new().address(&remote).build())
            .expect("endpoint");
        drop(address);

        (context, worker, ep)
    }

    fn run_amo_test<T>(
        initial: T,
        expected: T,
        what: &str,
        post: impl FnOnce(&Ep, u64, &RemoteKey, &crate::RequestParam) -> Result<Pending, crate::Status>,
    ) where
        T: Copy + PartialEq + std::fmt::Debug,
    {
        if std::env::var("UCX_TLS").as_deref() != Ok("self") {
            eprintln!("skipping {what}: run with UCX_TLS=self");
            return;
        }
        let mut target = initial;
        let (context, worker, ep) = setup_amo_test();
        // The caller owns the aligned target until after the mapping is dropped.
        let remote_addr = &mut target as *mut T as u64;
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                &mut target as *mut T as *mut u8,
                std::mem::size_of::<T>(),
            )
        };
        let memh = MemHandle::map_slice(&context, bytes, 0).expect("map target");
        let packed = RemoteKey::pack(&context, memh.mem_handle()).expect("pack rkey");
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().build();

        let request = post(&ep, remote_addr, &rkey, &param)
            .unwrap_or_else(|status| panic!("post {what}: {status} ({})", transport()));
        wait(&worker, request, what);
        let flush = ep
            .flush(&param)
            .unwrap_or_else(|status| panic!("post endpoint flush: {status} ({})", transport()));
        wait(&worker, flush, "endpoint flush");

        let result = unsafe { *(remote_addr as *const T) };
        assert_eq!(result, expected, "{what} result mismatch ({})", transport());

        drop(rkey);
        drop(memh);
        ep.close(&worker, 0).expect("close endpoint");
    }

    #[test]
    fn amo_self_add64_round_trip() {
        run_amo_test(10u64, 15u64, "amo_add64", |ep, addr, rkey, param| {
            ep.amo_add64(5, addr, rkey, param)
        });
    }

    #[test]
    fn amo_self_add32_round_trip() {
        run_amo_test(10u32, 15u32, "amo_add32", |ep, addr, rkey, param| {
            ep.amo_add32(5, addr, rkey, param)
        });
    }

    #[test]
    fn amo_self_xor64_round_trip() {
        run_amo_test(
            0b1010u64,
            0b0110u64,
            "amo_xor64",
            |ep, addr, rkey, param| ep.amo_xor64(0b1100, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_xor32_round_trip() {
        run_amo_test(
            0b1010u32,
            0b0110u32,
            "amo_xor32",
            |ep, addr, rkey, param| ep.amo_xor32(0b1100, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_swap64_round_trip() {
        run_amo_test(10u64, 5u64, "amo_swap64", |ep, addr, rkey, param| {
            ep.amo_swap64(5, addr, rkey, param)
        });
    }

    #[test]
    fn amo_self_swap32_round_trip() {
        run_amo_test(10u32, 5u32, "amo_swap32", |ep, addr, rkey, param| {
            ep.amo_swap32(5, addr, rkey, param)
        });
    }

    #[test]
    fn amo_self_and64_round_trip() {
        run_amo_test(
            0b1010u64,
            0b1000u64,
            "amo_and64",
            |ep, addr, rkey, param| ep.amo_and64(0b1100, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_and32_round_trip() {
        run_amo_test(
            0b1010u32,
            0b1000u32,
            "amo_and32",
            |ep, addr, rkey, param| ep.amo_and32(0b1100, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_or64_round_trip() {
        run_amo_test(0b1010u64, 0b1110u64, "amo_or64", |ep, addr, rkey, param| {
            ep.amo_or64(0b1100, addr, rkey, param)
        });
    }

    #[test]
    fn amo_self_or32_round_trip() {
        run_amo_test(0b1010u32, 0b1110u32, "amo_or32", |ep, addr, rkey, param| {
            ep.amo_or32(0b1100, addr, rkey, param)
        });
    }

    #[test]
    fn amo_self_cswap64_round_trip() {
        run_amo_test(
            10u64,
            20u64,
            "amo_cswap64 match",
            |ep, addr, rkey, param| ep.amo_cswap64(10, 20, addr, rkey, param),
        );
        run_amo_test(
            10u64,
            10u64,
            "amo_cswap64 mismatch",
            |ep, addr, rkey, param| ep.amo_cswap64(11, 20, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_cswap32_round_trip() {
        run_amo_test(
            10u32,
            20u32,
            "amo_cswap32 match",
            |ep, addr, rkey, param| ep.amo_cswap32(10, 20, addr, rkey, param),
        );
        run_amo_test(
            10u32,
            10u32,
            "amo_cswap32 mismatch",
            |ep, addr, rkey, param| ep.amo_cswap32(11, 20, addr, rkey, param),
        );
    }

    fn run_fetch_amo_test<T>(
        initial: T,
        expected: T,
        what: &str,
        post: impl for<'w, 'a> FnOnce(
            &Ep,
            &'w Worker,
            u64,
            &RemoteKey,
            &'a mut T,
        )
            -> Result<crate::rma::FetchAmoRequest<'w, 'a, T>, crate::Status>,
    ) where
        T: Copy + PartialEq + std::fmt::Debug,
    {
        if std::env::var("UCX_TLS").as_deref() != Ok("self") {
            eprintln!("skipping {what}: run with UCX_TLS=self");
            return;
        }
        let mut target = initial;
        let (context, worker, ep) = setup_amo_test();
        // The caller owns the aligned target until after the mapping is dropped.
        let remote_addr = &mut target as *mut T as u64;
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                &mut target as *mut T as *mut u8,
                std::mem::size_of::<T>(),
            )
        };
        let memh = MemHandle::map_slice(&context, bytes, 0).expect("map target");
        let packed = RemoteKey::pack(&context, memh.mem_handle()).expect("pack rkey");
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().build();
        let mut reply = expected;

        let request = post(&ep, &worker, remote_addr, &rkey, &mut reply)
            .unwrap_or_else(|status| panic!("post {what}: {status} ({})", transport()));
        // Complete the fetch at the endpoint before releasing its reply borrow.
        let flush = ep
            .flush(&param)
            .unwrap_or_else(|status| panic!("post endpoint flush: {status} ({})", transport()));
        wait(&worker, flush, "endpoint flush");
        drop(request);

        assert_eq!(
            reply,
            initial,
            "{what} old value mismatch ({})",
            transport()
        );
        let result = unsafe { *(remote_addr as *const T) };
        assert_eq!(result, expected, "{what} result mismatch ({})", transport());

        drop(rkey);
        drop(memh);
        ep.close(&worker, 0).expect("close endpoint");
    }

    #[test]
    fn amo_fadd64_self_round_trip() {
        run_fetch_amo_test(
            10u64,
            15u64,
            "amo_fadd64",
            |ep, worker, addr, rkey, reply| ep.amo_fadd64(worker, 5, addr, rkey, reply),
        );
    }

    #[test]
    fn amo_fadd32_self_round_trip() {
        run_fetch_amo_test(
            10u32,
            15u32,
            "amo_fadd32",
            |ep, worker, addr, rkey, reply| ep.amo_fadd32(worker, 5, addr, rkey, reply),
        );
    }

    #[test]
    fn amo_fxor64_self_round_trip() {
        run_fetch_amo_test(
            0b1010u64,
            0b0110u64,
            "amo_fxor64",
            |ep, worker, addr, rkey, reply| ep.amo_fxor64(worker, 0b1100, addr, rkey, reply),
        );
    }

    #[test]
    fn amo_fxor32_self_round_trip() {
        run_fetch_amo_test(
            0b1010u32,
            0b0110u32,
            "amo_fxor32",
            |ep, worker, addr, rkey, reply| ep.amo_fxor32(worker, 0b1100, addr, rkey, reply),
        );
    }

    #[test]
    fn amo_fswap64_self_round_trip() {
        run_fetch_amo_test(
            10u64,
            5u64,
            "amo_fswap64",
            |ep, worker, addr, rkey, reply| ep.amo_fswap64(worker, 5, addr, rkey, reply),
        );
    }

    #[test]
    fn amo_fswap32_self_round_trip() {
        run_fetch_amo_test(
            10u32,
            5u32,
            "amo_fswap32",
            |ep, worker, addr, rkey, reply| ep.amo_fswap32(worker, 5, addr, rkey, reply),
        );
    }

    /// RMA put and get through an endpoint connected to its own worker, using a
    /// packed and unpacked rkey. CI also runs this test on its own with
    /// `UCX_TLS=self` (the "RMA smoke test (self TLS)" job selects it by its full
    /// path, `rma::ep_ops::tests::rma_self_put_get_round_trip`; keep them in sync).
    #[test]
    fn rma_self_put_get_round_trip() {
        let params = ParamsBuilder::new()
            .features(Flags::Rma)
            .mt_workers_shared(1)
            .estimated_num_eps(1)
            .build();
        let mut context =
            Context::new(&Config::read("", "").expect("config"), &params).expect("context");
        let worker = context
            .worker_create(&WorkerParamsBuilder::new().build())
            .expect("worker");
        let address = worker.pack_address().expect("pack address");
        let remote = RemoteWorkerAddress::new(address.to_vec());
        let ep = worker
            .create_ep(EpParamsBuilder::new().address(&remote).build())
            .expect("endpoint");
        drop(address);

        let mut target = [0u8; 8];
        let target_addr = target.as_mut_ptr() as u64;
        let memh = MemHandle::map_slice(&context, &mut target, 0).expect("map target");
        let packed = RemoteKey::pack(&context, memh.mem_handle()).expect("pack rkey");
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        // Default params on purpose: UCX 1.16 (Ubuntu 24.04's libucx) transfers zero
        // bytes for a put with NO_IMM_CMPL and no explicit datatype (fixed upstream in
        // openucx ba426c890, UCX 1.17). `wait` handles immediate completion.
        let param = RequestParamBuilder::new().build();

        let put = ep
            .rma_put(&worker, b"ucx-rs!!".to_vec(), target_addr, &rkey, &param)
            .unwrap_or_else(|status| panic!("post RMA put: {status} ({})", transport()));
        assert_eq!(drive_owned(&worker, put, "RMA put"), b"ucx-rs!!");
        // A completed put only means the source buffer may be reused. The data is
        // guaranteed to be at the target, and visible to a later get, only after a
        // flush (see the ucp_put_nbx docs).
        let flush = ep
            .flush(&param)
            .unwrap_or_else(|status| panic!("post endpoint flush: {status} ({})", transport()));
        wait(&worker, flush, "endpoint flush");

        let get = ep
            .rma_get(&worker, vec![0u8; 8], target_addr, &rkey, &param)
            .unwrap_or_else(|status| panic!("post RMA get: {status} ({})", transport()));
        let fetched = drive_owned(&worker, get, "RMA get");

        drop(rkey);
        drop(memh);
        assert_eq!(
            (&target[..], &fetched[..]),
            (&b"ucx-rs!!"[..], &b"ucx-rs!!"[..]),
            "(RMA put target, RMA get result) ({})",
            transport()
        );
        ep.close(&worker, 0).expect("close endpoint");
    }
}
