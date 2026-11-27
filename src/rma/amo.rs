//! Ownership-safe atomic memory operations.
//!
//! UCX reads the AMO operand and (for fetching and swapping operations) writes
//! the reply buffer until the operation completes ("the user should not
//! modify any part of the buffer ... until the operation completes",
//! `ucp_atomic_op_nbx`). Both therefore live in one heap allocation owned by
//! the request ([`FetchAmoRequest`] / [`AmoRequest`]); it is never on the
//! caller's stack and does not move when the request moves.

use crate::completion::{owned_op_param, status_ptr_with_data};
use crate::dt::dt_make_contig;
use crate::ep::Ep;
use crate::ffi::*;
use crate::worker::Worker;
use crate::{Completion, OwningRequest, RequestParam, Status};

use super::RemoteKey;

mod sealed {
    pub trait Sealed {}
    impl Sealed for u32 {}
    impl Sealed for u64 {}
}

/// An AMO operand width UCX supports (`u32` or `u64`).
pub trait AmoWord: Copy + sealed::Sealed + 'static {}
impl AmoWord for u32 {}
impl AmoWord for u64 {}

/// Heap slots for one AMO: `[0]` operand (UCX `buffer`), `[1]` reply
/// (`reply_buffer`). `u64` slots are 8-byte aligned, which covers both widths.
type AmoSlots = Box<[u64; 2]>;

fn operand_ptr(slots: &mut AmoSlots) -> *mut u64 {
    slots.as_mut_ptr()
}

fn reply_ptr(slots: &mut AmoSlots) -> *mut u64 {
    // SAFETY: index 1 is in bounds of the two-element array.
    unsafe { slots.as_mut_ptr().add(1) }
}

fn read_reply<T: AmoWord>(slots: &AmoSlots) -> T {
    // SAFETY: slot 1 is 8-byte aligned, at least `size_of::<T>()` bytes, and
    // was initialized with a `T` before posting (UCX overwrites it with the
    // old remote value of the same width).
    unsafe { (slots.as_ptr().add(1) as *const T).read() }
}

/// A fetching AMO in flight; completes with the old remote value.
///
/// Owns the operand and reply slots until UCX is done with them. Same rules
/// as [`OwningRequest`]: non-blocking [`Self::try_complete`], mark-only
/// [`Self::cancel`], `Drop` fail-safe, never outlives its worker.
pub struct FetchAmoRequest<'w, T: AmoWord> {
    inner: OwningRequest<'w, AmoSlots>,
    _word: std::marker::PhantomData<fn() -> T>,
}

impl<T: AmoWord> std::fmt::Debug for FetchAmoRequest<'_, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FetchAmoRequest")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<'w, T: AmoWord> FetchAmoRequest<'w, T> {
    /// Non-blocking poll. `Ok(Ok(old))` on success, `Ok(Err(status))` on
    /// failure (including `UCS_ERR_CANCELED`), `Err(self)` while in flight.
    pub fn try_complete(self) -> Result<Result<T, Status>, Self> {
        match self.inner.try_complete() {
            Ok(Ok(slots)) => Ok(Ok(read_reply(&slots))),
            Ok(Err(failed)) => Ok(Err(failed.status)),
            Err(inner) => Err(Self {
                inner,
                _word: std::marker::PhantomData,
            }),
        }
    }

    /// Mark the operation canceled; see [`OwningRequest::cancel`].
    pub fn cancel(&mut self) {
        self.inner.cancel();
    }

    /// Whether [`Self::cancel`] has been called.
    pub fn is_canceled(&self) -> bool {
        self.inner.is_canceled()
    }

    /// The worker this request belongs to.
    pub fn worker(&self) -> &'w Worker {
        self.inner.worker()
    }
}

/// A non-fetching AMO in flight. Owns the operand (and, for swap/cswap, the
/// discarded reply slot) until UCX is done with them.
pub struct AmoRequest<'w> {
    inner: OwningRequest<'w, AmoSlots>,
}

impl std::fmt::Debug for AmoRequest<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AmoRequest")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<'w> AmoRequest<'w> {
    /// Non-blocking poll. `Ok(Ok(()))` on success, `Ok(Err(status))` on
    /// failure (including `UCS_ERR_CANCELED`), `Err(self)` while in flight.
    pub fn try_complete(self) -> Result<Result<(), Status>, Self> {
        match self.inner.try_complete() {
            Ok(Ok(_slots)) => Ok(Ok(())),
            Ok(Err(failed)) => Ok(Err(failed.status)),
            Err(inner) => Err(Self { inner }),
        }
    }

    /// Mark the operation canceled; see [`OwningRequest::cancel`].
    pub fn cancel(&mut self) {
        self.inner.cancel();
    }

    /// Whether [`Self::cancel`] has been called.
    pub fn is_canceled(&self) -> bool {
        self.inner.is_canceled()
    }

    /// The worker this request belongs to.
    pub fn worker(&self) -> &'w Worker {
        self.inner.worker()
    }
}

/// Completion of a fetching AMO: the old remote value, now or later.
pub type FetchAmoCompletion<'w, T> = Completion<T, FetchAmoRequest<'w, T>>;
/// Completion of a non-fetching AMO.
pub type AmoCompletion<'w> = Completion<(), AmoRequest<'w>>;

impl Ep {
    /// Post `op` with `operand` in heap slot 0 and, when `reply` is `Some`,
    /// that initial value in heap slot 1 as the reply buffer.
    #[allow(clippy::too_many_arguments)]
    fn post_amo<'w, T: AmoWord>(
        &self,
        worker: &'w Worker,
        op: ucp_atomic_op_t,
        operand: T,
        reply: Option<T>,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Completion<AmoSlots, OwningRequest<'w, AmoSlots>>, Status> {
        debug_assert!(
            std::sync::Arc::ptr_eq(&self.worker_alive, &worker.alive),
            "AMO: the passed worker is not the endpoint's worker"
        );
        let mut slots: AmoSlots = Box::new([0; 2]);
        let operand_ptr = operand_ptr(&mut slots);
        // SAFETY: slot 0 is 8-byte aligned and large enough for any AmoWord.
        unsafe { (operand_ptr as *mut T).write(operand) };
        let mut raw = owned_op_param(param);
        // AMOs require an explicit element datatype (contig 4 or 8), unlike
        // byte transfers where owned_op_param leaves the field clear.
        raw.op_attr_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_DATATYPE as u32;
        raw.datatype = dt_make_contig(std::mem::size_of::<T>());
        if let Some(reply) = reply {
            let reply_ptr = reply_ptr(&mut slots);
            // SAFETY: as for slot 0.
            unsafe { (reply_ptr as *mut T).write(reply) };
            raw.op_attr_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_REPLY_BUFFER as u32;
            raw.reply_buffer = reply_ptr.cast();
        }
        // SAFETY: UCX reads slot 0 and writes slot 1 of `slots` only; the
        // returned value owns `slots` (heap, stable across moves) until UCX
        // is done; count = 1 element of `size_of::<T>()` bytes.
        unsafe {
            let ptr = ucp_atomic_op_nbx(
                self.handle,
                op,
                operand_ptr as *const _,
                1,
                remote_addr,
                rkey.handle,
                &raw,
            );
            status_ptr_with_data(ptr, slots, worker)
        }
    }

    fn fetch_amo<'w, T: AmoWord>(
        &self,
        worker: &'w Worker,
        op: ucp_atomic_op_t,
        operand: T,
        reply: T,
        remote_addr: u64,
        rkey: &RemoteKey,
    ) -> Result<FetchAmoCompletion<'w, T>, Status> {
        let param = crate::RequestParamBuilder::new().build();
        Ok(
            match self.post_amo(worker, op, operand, Some(reply), remote_addr, rkey, &param)? {
                Completion::Immediate(slots) => Completion::Immediate(read_reply(&slots)),
                Completion::Pending(inner) => Completion::Pending(FetchAmoRequest {
                    inner,
                    _word: std::marker::PhantomData,
                }),
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn no_fetch_amo<'w, T: AmoWord>(
        &self,
        worker: &'w Worker,
        op: ucp_atomic_op_t,
        operand: T,
        reply: Option<T>,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        Ok(
            match self.post_amo(worker, op, operand, reply, remote_addr, rkey, param)? {
                Completion::Immediate(_slots) => Completion::Immediate(()),
                Completion::Pending(inner) => Completion::Pending(AmoRequest { inner }),
            },
        )
    }

    /// Atomic ADD 64-bit on remote memory (no fetch of the old value).
    ///
    /// The operand is owned by the returned request until UCX completes the
    /// operation. `param` contributes flags, callback, and user data; the
    /// datatype, reply buffer, receive info, memory handle, memory type, and
    /// request allocation fields are controlled here.
    pub fn amo_add64<'w>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
            operand,
            None,
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic XOR 64-bit on remote memory (no fetch of the old value).
    ///
    /// The operand is owned by the returned request until UCX completes the
    /// operation. `param` contributes flags, callback, and user data; the
    /// datatype, reply buffer, receive info, memory handle, memory type, and
    /// request allocation fields are controlled here.
    pub fn amo_xor64<'w>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
            operand,
            None,
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic SWAP 64-bit on remote memory (no fetch of the old value). UCX requires a reply buffer for SWAP; it is owned by the request and discarded.
    ///
    /// The operand is owned by the returned request until UCX completes the
    /// operation. `param` contributes flags, callback, and user data; the
    /// datatype, reply buffer, receive info, memory handle, memory type, and
    /// request allocation fields are controlled here.
    pub fn amo_swap64<'w>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
            operand,
            Some(0),
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic AND 64-bit on remote memory (no fetch of the old value).
    ///
    /// The operand is owned by the returned request until UCX completes the
    /// operation. `param` contributes flags, callback, and user data; the
    /// datatype, reply buffer, receive info, memory handle, memory type, and
    /// request allocation fields are controlled here.
    pub fn amo_and64<'w>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_AND,
            operand,
            None,
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic OR 64-bit on remote memory (no fetch of the old value).
    ///
    /// The operand is owned by the returned request until UCX completes the
    /// operation. `param` contributes flags, callback, and user data; the
    /// datatype, reply buffer, receive info, memory handle, memory type, and
    /// request allocation fields are controlled here.
    pub fn amo_or64<'w>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_OR,
            operand,
            None,
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic compare-and-swap 64-bit (no fetch; use [`Ep::amo_fcswap64`] for
    /// the old value).
    ///
    /// The remote value is replaced with `replacement` only when it equals
    /// `expected`. UCX CSWAP reads the compare value from `buffer` and the swap
    /// value from `reply_buffer` (which then receives the old value); both are
    /// owned by the request and the reply is discarded. `param` is handled as
    /// in [`Ep::amo_add64`].
    pub fn amo_cswap64<'w>(
        &self,
        worker: &'w Worker,
        expected: u64,
        replacement: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
            expected,
            Some(replacement),
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic fetch-and-add 64-bit; completes with the old remote value.
    pub fn amo_fadd64<'w>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
    ) -> Result<FetchAmoCompletion<'w, u64>, Status> {
        self.fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
            operand,
            0,
            remote_addr,
            rkey,
        )
    }

    /// Atomic fetch-and-xor 64-bit; completes with the old remote value.
    pub fn amo_fxor64<'w>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
    ) -> Result<FetchAmoCompletion<'w, u64>, Status> {
        self.fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
            operand,
            0,
            remote_addr,
            rkey,
        )
    }

    /// Atomic fetch-and-swap 64-bit; completes with the old remote value.
    pub fn amo_fswap64<'w>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
    ) -> Result<FetchAmoCompletion<'w, u64>, Status> {
        self.fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
            operand,
            0,
            remote_addr,
            rkey,
        )
    }

    /// Atomic fetch compare-and-swap 64-bit; completes with the old remote
    /// value whether or not the swap happened.
    ///
    /// The remote value is replaced with `swap` only when it equals `compare`.
    /// UCX CSWAP uses `buffer` for the compare value and `reply_buffer` for the
    /// swap value on input and the old remote value on output; both are owned
    /// by the request.
    pub fn amo_fcswap64<'w>(
        &self,
        worker: &'w Worker,
        compare: u64,
        swap: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
    ) -> Result<FetchAmoCompletion<'w, u64>, Status> {
        self.fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
            compare,
            swap,
            remote_addr,
            rkey,
        )
    }

    /// Atomic ADD 32-bit on remote memory (no fetch of the old value).
    ///
    /// The operand is owned by the returned request until UCX completes the
    /// operation. `param` contributes flags, callback, and user data; the
    /// datatype, reply buffer, receive info, memory handle, memory type, and
    /// request allocation fields are controlled here.
    pub fn amo_add32<'w>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
            operand,
            None,
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic XOR 32-bit on remote memory (no fetch of the old value).
    ///
    /// The operand is owned by the returned request until UCX completes the
    /// operation. `param` contributes flags, callback, and user data; the
    /// datatype, reply buffer, receive info, memory handle, memory type, and
    /// request allocation fields are controlled here.
    pub fn amo_xor32<'w>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
            operand,
            None,
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic SWAP 32-bit on remote memory (no fetch of the old value). UCX requires a reply buffer for SWAP; it is owned by the request and discarded.
    ///
    /// The operand is owned by the returned request until UCX completes the
    /// operation. `param` contributes flags, callback, and user data; the
    /// datatype, reply buffer, receive info, memory handle, memory type, and
    /// request allocation fields are controlled here.
    pub fn amo_swap32<'w>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
            operand,
            Some(0),
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic AND 32-bit on remote memory (no fetch of the old value).
    ///
    /// The operand is owned by the returned request until UCX completes the
    /// operation. `param` contributes flags, callback, and user data; the
    /// datatype, reply buffer, receive info, memory handle, memory type, and
    /// request allocation fields are controlled here.
    pub fn amo_and32<'w>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_AND,
            operand,
            None,
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic OR 32-bit on remote memory (no fetch of the old value).
    ///
    /// The operand is owned by the returned request until UCX completes the
    /// operation. `param` contributes flags, callback, and user data; the
    /// datatype, reply buffer, receive info, memory handle, memory type, and
    /// request allocation fields are controlled here.
    pub fn amo_or32<'w>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_OR,
            operand,
            None,
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic compare-and-swap 32-bit (no fetch; use [`Ep::amo_fcswap32`] for
    /// the old value).
    ///
    /// The remote value is replaced with `replacement` only when it equals
    /// `expected`. UCX CSWAP reads the compare value from `buffer` and the swap
    /// value from `reply_buffer` (which then receives the old value); both are
    /// owned by the request and the reply is discarded. `param` is handled as
    /// in [`Ep::amo_add32`].
    pub fn amo_cswap32<'w>(
        &self,
        worker: &'w Worker,
        expected: u32,
        replacement: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<AmoCompletion<'w>, Status> {
        self.no_fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
            expected,
            Some(replacement),
            remote_addr,
            rkey,
            param,
        )
    }

    /// Atomic fetch-and-add 32-bit; completes with the old remote value.
    pub fn amo_fadd32<'w>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
    ) -> Result<FetchAmoCompletion<'w, u32>, Status> {
        self.fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
            operand,
            0,
            remote_addr,
            rkey,
        )
    }

    /// Atomic fetch-and-xor 32-bit; completes with the old remote value.
    pub fn amo_fxor32<'w>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
    ) -> Result<FetchAmoCompletion<'w, u32>, Status> {
        self.fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
            operand,
            0,
            remote_addr,
            rkey,
        )
    }

    /// Atomic fetch-and-swap 32-bit; completes with the old remote value.
    pub fn amo_fswap32<'w>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
    ) -> Result<FetchAmoCompletion<'w, u32>, Status> {
        self.fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
            operand,
            0,
            remote_addr,
            rkey,
        )
    }

    /// Atomic fetch compare-and-swap 32-bit; completes with the old remote
    /// value whether or not the swap happened.
    ///
    /// The remote value is replaced with `swap` only when it equals `compare`.
    /// UCX CSWAP uses `buffer` for the compare value and `reply_buffer` for the
    /// swap value on input and the old remote value on output; both are owned
    /// by the request.
    pub fn amo_fcswap32<'w>(
        &self,
        worker: &'w Worker,
        compare: u32,
        swap: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
    ) -> Result<FetchAmoCompletion<'w, u32>, Status> {
        self.fetch_amo(
            worker,
            ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
            compare,
            swap,
            remote_addr,
            rkey,
        )
    }
}
