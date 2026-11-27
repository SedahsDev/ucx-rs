//! Ownership-transferring completion model for non-blocking UCX operations.
//!
//! A non-blocking operation takes **ownership** of its data and returns
//! [`Completion`]:
//!
//! * [`Completion::Immediate`]: the operation finished inline and the data is
//!   handed straight back.
//! * [`Completion::Pending`]: an [`OwningRequest`] holds the data until UCX has
//!   finished with it. The data comes back from [`OwningRequest::try_complete`].
//!
//! # Rules
//!
//! * **A request never outlives its worker.** [`OwningRequest`] stores
//!   `&'w Worker`, so the borrow checker enforces this.
//! * **Many requests, one progress loop.** No public API waits on a single
//!   request. Call [`Worker::progress`] once, then poll each outstanding
//!   request with [`OwningRequest::try_complete`]. UCX already serializes
//!   `ucp_worker_progress` internally.
//! * **Never progress from a UCX callback**, and never drop an
//!   [`OwningRequest`] inside one: its `Drop` fail-safe progresses the worker.
//! * **`cancel` only marks the request.** The data stays owned by the request
//!   until a later `try_complete` reports completion (usually
//!   `UCS_ERR_CANCELED`). `ucp_request_free` on an in-flight request is
//!   defined (#68) but does not make the buffer safe to drop, so it is never
//!   done here.
//! * **`Drop` is a last-resort fail-safe.** It cancels a live request and runs
//!   a bounded progress loop ([`DROP_PROGRESS_BUDGET`] rounds). On timeout it
//!   **leaks** both the request and the data rather than freeing memory UCX may
//!   still touch.
//! * **Data must have a stable address** ([`OwnedBuf`]): the request is moved
//!   by `try_complete`, `Vec::push`, etc., while UCX keeps the raw pointer.
//!
//! ```no_run
//! # use ucx_sys::{Completion, OwningRequest, worker::Worker};
//! # fn handle<D>(_: Result<D, ucx_sys::Failed<D>>) {}
//! # fn demo<'w>(worker: &'w Worker, mut pending: Vec<OwningRequest<'w, Vec<u8>>>) {
//! while !pending.is_empty() {
//!     worker.progress();
//!     let mut still = Vec::with_capacity(pending.len());
//!     for request in pending.drain(..) {
//!         match request.try_complete() {
//!             Ok(result) => handle(result),
//!             Err(request) => still.push(request),
//!         }
//!     }
//!     pending = still;
//! }
//! # }
//! ```

use crate::ffi::*;
use crate::request::status_ptr_is_err;
use crate::status::status_from_ptr;
use crate::worker::Worker;
use crate::Request;
use crate::Status;

/// Progress rounds the [`OwningRequest`] `Drop` fail-safe runs before it gives
/// up and leaks the request and its data.
pub const DROP_PROGRESS_BUDGET: usize = 1_000_000;

/// A byte buffer whose storage address is stable while it is owned.
///
/// # Safety
///
/// Implementors guarantee that, for as long as the value is alive and not
/// mutated through its own API:
///
/// * [`OwnedBuf::as_ptr`] and [`OwnedBuf::len`] return the same values on
///   every call, and
/// * the pointed-to storage does **not** move when the value itself is moved
///   (heap or otherwise externally owned storage; never inline storage such as
///   `[u8; N]` or a bare integer).
///
/// UCX keeps the raw pointer while an operation is in flight, and the owning
/// value is moved around (into [`OwningRequest`], out of `try_complete`, into
/// a `Vec`) during that time.
///
/// Inline storage is rejected:
///
/// ```compile_fail
/// fn needs<D: ucx_sys::OwnedBuf>() {}
/// needs::<[u8; 64]>();
/// ```
///
/// ```compile_fail
/// fn needs<D: ucx_sys::OwnedBuf>() {}
/// needs::<u64>();
/// ```
pub unsafe trait OwnedBuf {
    /// Start of the buffer.
    fn as_ptr(&self) -> *const u8;
    /// Length of the buffer in bytes.
    fn len(&self) -> usize;
    /// Whether the buffer is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A mutable [`OwnedBuf`] that UCX may write into (receive / get).
///
/// # Safety
///
/// Same contract as [`OwnedBuf`]; additionally [`OwnedBufMut::as_mut_ptr`]
/// returns the same address as [`OwnedBuf::as_ptr`] and grants write access to
/// all [`OwnedBuf::len`] bytes.
pub unsafe trait OwnedBufMut: OwnedBuf {
    /// Writable start of the buffer.
    fn as_mut_ptr(&mut self) -> *mut u8;
}

// SAFETY: a Vec's heap allocation does not move when the Vec is moved, and the
// owner exposes no API that could reallocate it while it is held here.
unsafe impl OwnedBuf for Vec<u8> {
    fn as_ptr(&self) -> *const u8 {
        Vec::as_ptr(self)
    }
    fn len(&self) -> usize {
        Vec::len(self)
    }
}
// SAFETY: see the `OwnedBuf` impl.
unsafe impl OwnedBufMut for Vec<u8> {
    fn as_mut_ptr(&mut self) -> *mut u8 {
        Vec::as_mut_ptr(self)
    }
}

// SAFETY: a boxed slice's heap allocation does not move with the box.
unsafe impl OwnedBuf for Box<[u8]> {
    fn as_ptr(&self) -> *const u8 {
        <[u8]>::as_ptr(self)
    }
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }
}
// SAFETY: see the `OwnedBuf` impl.
unsafe impl OwnedBufMut for Box<[u8]> {
    fn as_mut_ptr(&mut self) -> *mut u8 {
        <[u8]>::as_mut_ptr(self)
    }
}

// SAFETY: a boxed array's heap allocation does not move with the box.
unsafe impl<const N: usize> OwnedBuf for Box<[u8; N]> {
    fn as_ptr(&self) -> *const u8 {
        <[u8]>::as_ptr(&**self)
    }
    fn len(&self) -> usize {
        N
    }
}
// SAFETY: see the `OwnedBuf` impl.
unsafe impl<const N: usize> OwnedBufMut for Box<[u8; N]> {
    fn as_mut_ptr(&mut self) -> *mut u8 {
        <[u8]>::as_mut_ptr(&mut **self)
    }
}

/// Outcome of posting an ownership-transferring non-blocking operation.
#[derive(Debug)]
#[must_use = "a pending completion owns the data until it is completed"]
pub enum Completion<D, R> {
    /// Completed immediately. Ownership of the data is returned.
    Immediate(D),
    /// In flight. `R` owns the data until completion or cancel-completion.
    Pending(R),
}

impl<D, R> Completion<D, R> {
    /// True if the operation completed inline.
    #[inline]
    pub fn is_immediate(&self) -> bool {
        matches!(self, Completion::Immediate(_))
    }

    /// True if the operation is still in flight.
    #[inline]
    pub fn is_pending(&self) -> bool {
        matches!(self, Completion::Pending(_))
    }

    /// The returned data if the operation completed inline.
    #[inline]
    pub fn into_immediate(self) -> Option<D> {
        match self {
            Completion::Immediate(data) => Some(data),
            Completion::Pending(_) => None,
        }
    }

    /// The owning request if the operation is still in flight.
    #[inline]
    pub fn into_pending(self) -> Option<R> {
        match self {
            Completion::Immediate(_) => None,
            Completion::Pending(request) => Some(request),
        }
    }
}

/// A request that completed with an error (including `UCS_ERR_CANCELED`).
///
/// UCX is finished with the data, so it is handed back for reuse. `Debug`
/// and `Display` show only the status (buffers can be large), so
/// `result.expect(..)` works for any `D`.
pub struct Failed<D> {
    /// The UCX completion status.
    pub status: Status,
    /// The data the request owned.
    pub data: D,
}

impl<D> Failed<D> {
    /// True if the request completed because it was canceled.
    #[inline]
    pub fn is_canceled(&self) -> bool {
        self.status == Status::CANCELED
    }

    /// Discard the status and keep the data.
    #[inline]
    pub fn into_data(self) -> D {
        self.data
    }
}

impl<D> std::fmt::Debug for Failed<D> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Failed")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl<D> std::fmt::Display for Failed<D> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "UCX request failed: {}", self.status)
    }
}

impl<D> std::error::Error for Failed<D> {}

/// A UCX request that owns its data until the request completes (or is
/// canceled and the cancel completes).
///
/// Invariant: never outlives its worker.
///
/// ```compile_fail
/// use ucx_sys::{worker::Worker, OwningRequest};
/// fn escape(
///     worker: Worker,
///     post: impl for<'w> Fn(&'w Worker) -> OwningRequest<'w, Vec<u8>>,
/// ) -> OwningRequest<'static, Vec<u8>> {
///     let request = post(&worker);
///     drop(worker);
///     request
/// }
/// ```
pub struct OwningRequest<'w, D> {
    request: Option<Request>,
    data: Option<D>,
    worker: &'w Worker,
    canceled: bool,
}

impl<D> std::fmt::Debug for OwningRequest<'_, D> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OwningRequest")
            .field("request", &self.request)
            .field("canceled", &self.canceled)
            .finish_non_exhaustive()
    }
}

/// Request status as seen through `ucp_request_check_status`.
enum Poll {
    InProgress,
    Done(Result<(), Status>),
}

fn poll(request: &Request) -> Poll {
    // SAFETY: `request` is a live UCX request handle owned by the caller.
    let status = unsafe { ucp_request_check_status(request.as_ptr()) };
    if status == ucs_status_t::UCS_INPROGRESS {
        return Poll::InProgress;
    }
    let status_ptr = status as isize as usize as ucs_status_ptr_t;
    if status_ptr_is_err(status_ptr) {
        Poll::Done(Err(Status::from(status_from_ptr(status_ptr))))
    } else {
        Poll::Done(Ok(()))
    }
}

impl<'w, D> OwningRequest<'w, D> {
    /// Wrap a live, in-flight request that is using `data`.
    pub(crate) fn new(request: Request, data: D, worker: &'w Worker) -> Self {
        Self {
            request: Some(request),
            data: Some(data),
            worker,
            canceled: false,
        }
    }

    /// The worker this request belongs to.
    #[inline]
    pub fn worker(&self) -> &'w Worker {
        self.worker
    }

    /// Whether [`Self::cancel`] has been called.
    #[inline]
    pub fn is_canceled(&self) -> bool {
        self.canceled
    }

    /// Non-blocking poll. Never progresses the worker.
    ///
    /// * `Ok(Ok(data))`: completed successfully; the data is returned.
    /// * `Ok(Err(failed))`: completed with an error (including
    ///   `UCS_ERR_CANCELED`); the data is returned inside [`Failed`].
    /// * `Err(self)`: still in progress.
    pub fn try_complete(mut self) -> Result<Result<D, Failed<D>>, Self> {
        let Some(request) = self.request.as_ref() else {
            unreachable!("OwningRequest without a request");
        };
        match poll(request) {
            Poll::InProgress => Err(self),
            Poll::Done(result) => {
                // Completed: UCX no longer touches the data, so free the
                // request (Request::drop) and hand the data back.
                drop(self.request.take());
                let data = self.data.take().expect("OwningRequest without data");
                Ok(match result {
                    Ok(()) => Ok(data),
                    Err(status) => Err(Failed { status, data }),
                })
            }
        }
    }

    /// Ask UCX to cancel this request. Never progresses or frees.
    ///
    /// The outcome is observed through [`Self::try_complete`]: usually
    /// `UCS_ERR_CANCELED`, or success if the request was already finishing.
    /// Calling this more than once is a no-op.
    ///
    /// UCX (1.18–1.22) only acts on **tag receives**: a software tag receive
    /// is completed inside `ucp_request_cancel` itself (no progress needed),
    /// an offloaded one after progress. Sends, RMA, AM, stream, and flush
    /// requests ignore the cancel and complete normally; to abort those, close
    /// the endpoint with force. This is not a general abort.
    pub fn cancel(&mut self) {
        if self.canceled {
            return;
        }
        if let Some(request) = self.request.as_ref() {
            // SAFETY: the worker outlives `self` and the request is live;
            // ucp_request_cancel only marks the request.
            unsafe { ucp_request_cancel(self.worker.handle, request.as_ptr()) };
            self.canceled = true;
        }
    }

    /// Cancel, then progress until completion or `budget` rounds; free on
    /// completion, leak request and data on timeout.
    fn finish_or_leak(&mut self, budget: usize, cancel: bool) {
        let Some(request) = self.request.take() else {
            return;
        };
        if cancel && matches!(poll(&request), Poll::InProgress) && !self.canceled {
            // SAFETY: as in `cancel`.
            unsafe { ucp_request_cancel(self.worker.handle, request.as_ptr()) };
            self.canceled = true;
        }
        for round in 0..=budget {
            if !matches!(poll(&request), Poll::InProgress) {
                // Request::drop frees it; the data drops with `self`.
                return;
            }
            if round < budget {
                self.worker.progress();
            }
        }
        // Timed out: UCX may still access the data. Never free either. The
        // request stays queued on the worker after this borrow ends; a later
        // `ucp_worker_destroy` reclaims it. Leaking is the deliberate tradeoff
        // (#203): a stuck request must never let its buffer be reused.
        std::mem::forget(request);
        if let Some(data) = self.data.take() {
            std::mem::forget(data);
        }
    }
}

impl<D> Drop for OwningRequest<'_, D> {
    /// Fail-safe only: cancel a live request, progress for at most
    /// [`DROP_PROGRESS_BUDGET`] rounds, then free the request and drop the
    /// data. On timeout both are leaked. Must not run inside a UCX callback.
    fn drop(&mut self) {
        self.finish_or_leak(DROP_PROGRESS_BUDGET, true);
    }
}

/// `ucp_op_attr_t` fields an ownership-transferring operation must control
/// itself: a caller-supplied request allocation (`REQUEST`, which would make
/// `ucp_request_free` wrong), a datatype that would make UCX read or write
/// past the owned buffer (`DATATYPE`), pointers to memory the request does not
/// own (`REPLY_BUFFER`, `RECV_INFO`, `MEMH`), and a memory type that may not
/// match the owned buffer (`MEMORY_TYPE`; UCX detects it instead).
const OWNED_OP_RESERVED_FIELDS: u32 = ucp_op_attr_t::UCP_OP_ATTR_FIELD_REQUEST as u32
    | ucp_op_attr_t::UCP_OP_ATTR_FIELD_DATATYPE as u32
    | ucp_op_attr_t::UCP_OP_ATTR_FIELD_REPLY_BUFFER as u32
    | ucp_op_attr_t::UCP_OP_ATTR_FIELD_MEMORY_TYPE as u32
    | ucp_op_attr_t::UCP_OP_ATTR_FIELD_RECV_INFO as u32
    | ucp_op_attr_t::UCP_OP_ATTR_FIELD_MEMH as u32;

/// Copy `param` for an ownership-transferring operation over a contiguous
/// byte buffer, dropping every field the owned data model must control (see
/// [`OWNED_OP_RESERVED_FIELDS`]), then set the datatype explicitly to
/// contiguous bytes so `count` is the byte length. (UCX 1.16 transfers zero
/// bytes for a `NO_IMM_CMPL` put without an explicit datatype; openucx
/// ba426c890.) Callback, user data, and flags (including `NO_IMM_CMPL` /
/// `FORCE_IMM_CMPL`) are kept.
pub(crate) fn owned_op_param(param: &crate::RequestParam) -> ucp_request_param_t {
    let mut raw = param.handle;
    raw.op_attr_mask &= !OWNED_OP_RESERVED_FIELDS;
    raw.reply_buffer = std::ptr::null_mut();
    raw.op_attr_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_DATATYPE as u32;
    raw.datatype = crate::dt::dt_make_contig(1);
    raw
}

/// Classify a status pointer from an operation that is using `data`.
///
/// # Safety
///
/// `ptr` must be the return value of a `ucs_status_ptr_t` UCX API whose
/// in-flight request (if any) was posted on `worker`, has not been freed, and
/// accesses only memory owned by `data` **whose address is stable when `data`
/// is moved** (heap storage as in [`OwnedBuf`]; never inline storage such as
/// `[u8; N]` or a bare integer, which would move with the request and leave
/// UCX with a dangling pointer).
///
/// `D` is deliberately not bounded by [`OwnedBuf`] so composite owners (a
/// header + payload pair, an AMO operand/reply box) can be used; the
/// stable-address rule above still applies to every pointer given to UCX.
pub unsafe fn status_ptr_with_data<'w, D>(
    ptr: ucs_status_ptr_t,
    data: D,
    worker: &'w Worker,
) -> Result<Completion<D, OwningRequest<'w, D>>, Status> {
    match crate::status_ptr_to_result(ptr)? {
        crate::Pending::Complete => Ok(Completion::Immediate(data)),
        crate::Pending::InProgress(request) => Ok(Completion::Pending(OwningRequest::new(
            request, data, worker,
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::setup_default;
    use crate::RequestParamBuilder;
    use std::cell::Cell;
    use std::rc::Rc;

    type Owned<'w, D> = Completion<D, OwningRequest<'w, D>>;

    fn post_send<'w, D: OwnedBuf>(
        ep: &crate::ep::Ep,
        worker: &'w Worker,
        data: D,
        tag: u64,
        param: &crate::RequestParam,
    ) -> Result<Owned<'w, D>, Status> {
        // SAFETY: the request (if any) reads only `data`'s stable storage,
        // which the returned value owns until completion.
        unsafe {
            let ptr = ucp_tag_send_nbx(
                ep.handle,
                data.as_ptr() as _,
                data.len(),
                tag,
                &param.handle,
            );
            status_ptr_with_data(ptr, data, worker)
        }
    }

    fn post_recv<D: OwnedBufMut>(worker: &Worker, mut data: D, tag: u64) -> Owned<'_, D> {
        let param = RequestParamBuilder::new().build();
        // SAFETY: as in `post_send`, for writes into `data`.
        unsafe {
            let ptr = ucp_tag_recv_nbx(
                worker.handle,
                data.as_mut_ptr() as _,
                data.len(),
                tag,
                u64::MAX,
                &param.handle,
            );
            status_ptr_with_data(ptr, data, worker).expect("tag_recv_nbx")
        }
    }

    /// Test-only progress driver for a single completion.
    fn drive<D>(worker: &Worker, completion: Owned<'_, D>) -> Result<D, Failed<D>> {
        let mut request = match completion {
            Completion::Immediate(data) => return Ok(data),
            Completion::Pending(request) => request,
        };
        for _ in 0..100_000 {
            worker.progress();
            match request.try_complete() {
                Ok(result) => return result,
                Err(still) => request = still,
            }
        }
        panic!("request did not complete");
    }

    /// A buffer that counts how many times it has been dropped.
    struct CountingBuf {
        bytes: Vec<u8>,
        drops: Rc<Cell<u32>>,
    }
    impl Drop for CountingBuf {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }
    // SAFETY: delegates to the inner Vec, which is never reallocated.
    unsafe impl OwnedBuf for CountingBuf {
        fn as_ptr(&self) -> *const u8 {
            self.bytes.as_ptr()
        }
        fn len(&self) -> usize {
            self.bytes.len()
        }
    }
    // SAFETY: see above.
    unsafe impl OwnedBufMut for CountingBuf {
        fn as_mut_ptr(&mut self) -> *mut u8 {
            self.bytes.as_mut_ptr()
        }
    }

    #[test]
    fn failed_debug_and_display_do_not_need_debug_data() {
        struct Opaque;
        let failed = Failed {
            status: Status::CANCELED,
            data: Opaque,
        };
        assert!(format!("{failed:?}").contains("status"));
        assert!(format!("{failed}").starts_with("UCX request failed"));
        let result: Result<(), Failed<Opaque>> = Err(failed);
        assert!(result.is_err());
    }

    #[test]
    fn owned_op_param_strips_reserved_fields_and_keeps_flags() {
        let mut reply = 0u64;
        let mut param = RequestParamBuilder::new()
            .no_imm_cmpl()
            .datatype(crate::dt::dt_make_contig(8))
            .memory_type(crate::MemoryType::Cuda)
            .reply_buffer(&mut reply as *mut u64 as *mut _)
            .build();
        param.handle.op_attr_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_REQUEST as u32
            | ucp_op_attr_t::UCP_OP_ATTR_FIELD_RECV_INFO as u32
            | ucp_op_attr_t::UCP_OP_ATTR_FIELD_MEMH as u32
            | ucp_op_attr_t::UCP_OP_ATTR_FIELD_USER_DATA as u32;
        let raw = owned_op_param(&param);
        let datatype = ucp_op_attr_t::UCP_OP_ATTR_FIELD_DATATYPE as u32;
        assert_eq!(raw.op_attr_mask & OWNED_OP_RESERVED_FIELDS, datatype);
        assert_eq!(
            raw.datatype,
            crate::dt::dt_make_contig(1),
            "contiguous bytes"
        );
        assert!(raw.reply_buffer.is_null());
        let kept = ucp_op_attr_t::UCP_OP_ATTR_FLAG_NO_IMM_CMPL as u32
            | ucp_op_attr_t::UCP_OP_ATTR_FIELD_USER_DATA as u32;
        assert_eq!(raw.op_attr_mask & kept, kept);
    }

    #[test]
    fn completion_accessors() {
        let immediate: Completion<u8, ()> = Completion::Immediate(7);
        assert!(immediate.is_immediate() && !immediate.is_pending());
        assert_eq!(immediate.into_immediate(), Some(7));
        let pending: Completion<u8, ()> = Completion::Pending(());
        assert!(pending.is_pending() && !pending.is_immediate());
        assert_eq!(pending.into_pending(), Some(()));
        assert!(Completion::<u8, ()>::Immediate(1).into_pending().is_none());
        assert!(Completion::<u8, ()>::Pending(()).into_immediate().is_none());
    }

    #[test]
    fn owned_buf_impls_report_heap_storage() {
        let vec = vec![1u8, 2, 3];
        assert_eq!(OwnedBuf::as_ptr(&vec), vec.as_ptr());
        let boxed: Box<[u8]> = vec![0u8; 5].into_boxed_slice();
        let address = OwnedBuf::as_ptr(&boxed);
        let moved = boxed;
        assert_eq!(OwnedBuf::as_ptr(&moved), address, "address survives a move");
        let array: Box<[u8; 4]> = Box::new([9; 4]);
        assert_eq!(OwnedBuf::len(&array), 4);
        assert!(!OwnedBuf::is_empty(&array));
    }

    #[test]
    fn immediate_send_returns_data() {
        let comms = setup_default();
        let param = RequestParamBuilder::new().force_imm_cmpl().build();
        let sent = post_send(&comms.ep, &comms.worker, vec![5u8; 16], 0x10, &param)
            .expect("small eager send completes inline on self");
        assert!(sent.is_immediate());
        assert_eq!(sent.into_immediate().unwrap(), vec![5u8; 16]);
        // Consume the unexpected message so the worker is clean.
        let received = drive(&comms.worker, post_recv(&comms.worker, vec![0u8; 16], 0x10));
        assert_eq!(received.expect("recv"), vec![5u8; 16]);
    }

    #[test]
    fn pending_rendezvous_round_trip_returns_both_buffers() {
        let comms = setup_default();
        let len = 8 << 20;
        let recv = post_recv(&comms.worker, vec![0u8; len], 0x20);
        assert!(recv.is_pending(), "no message yet, recv must be pending");
        let payload: Vec<u8> = (0..len).map(|i| i as u8).collect();
        let param = RequestParamBuilder::new().build();
        let send = post_send(&comms.ep, &comms.worker, payload.clone(), 0x20, &param).unwrap();
        let sent = drive(&comms.worker, send).expect("send");
        let received = drive(&comms.worker, recv).expect("recv");
        assert_eq!(sent, payload);
        assert!(received == payload, "received bytes differ");
    }

    /// Without external progress, a rendezvous transfer that progress would
    /// complete stays pending across many `try_complete` polls: polling never
    /// drives the worker. One progress loop then completes both sides.
    #[test]
    fn try_complete_does_not_progress() {
        let comms = setup_default();
        let len = 8 << 20;
        let mut recv = post_recv(&comms.worker, vec![0u8; len], 0x30)
            .into_pending()
            .expect("pending recv");
        let param = RequestParamBuilder::new().build();
        let mut send = post_send(&comms.ep, &comms.worker, vec![3u8; len], 0x30, &param)
            .unwrap()
            .into_pending()
            .expect("rendezvous send is pending after posting");
        for _ in 0..1000 {
            // `match`, not `expect_err`: a failure must not Debug-print 8 MiB.
            send = match send.try_complete() {
                Err(still) => still,
                Ok(_) => panic!("send completed without progress"),
            };
            recv = match recv.try_complete() {
                Err(still) => still,
                Ok(_) => panic!("recv completed without progress"),
            };
        }
        let sent = drive(&comms.worker, Completion::Pending(send)).expect("send");
        let received = drive(&comms.worker, Completion::Pending(recv)).expect("recv");
        assert_eq!((sent.len(), received[len - 1]), (len, 3));
    }

    #[test]
    fn cancel_marks_only_and_returns_buffer() {
        let comms = setup_default();
        let mut request = post_recv(&comms.worker, vec![0xAAu8; 32], 0x40)
            .into_pending()
            .expect("pending");
        assert!(!request.is_canceled());
        request.cancel();
        request.cancel(); // idempotent
        assert!(request.is_canceled());
        let failed = drive(&comms.worker, Completion::Pending(request)).expect_err("canceled");
        assert_eq!(failed.status, Status::CANCELED);
        assert_eq!(
            failed.into_data(),
            vec![0xAAu8; 32],
            "buffer handed back untouched"
        );
    }

    #[test]
    fn many_requests_one_progress_loop() {
        let comms = setup_default();
        let mut pending: Vec<OwningRequest<'_, Vec<u8>>> = (0..8u64)
            .map(|i| {
                post_recv(&comms.worker, vec![0u8; 4], 0x100 + i)
                    .into_pending()
                    .expect("pending")
            })
            .collect();
        let param = RequestParamBuilder::new().build();
        for i in 0..4u64 {
            let sent = post_send(
                &comms.ep,
                &comms.worker,
                vec![i as u8; 4],
                0x100 + i,
                &param,
            );
            let _ = drive(&comms.worker, sent.unwrap()).expect("send");
        }
        for request in pending.iter_mut().skip(4) {
            request.cancel();
        }
        let (mut ok, mut canceled) = (0, 0);
        for _ in 0..100_000 {
            if pending.is_empty() {
                break;
            }
            comms.worker.progress();
            let mut still = Vec::new();
            for request in pending.drain(..) {
                match request.try_complete() {
                    Ok(Ok(data)) => {
                        assert_eq!(data[0], data[3]);
                        ok += 1;
                    }
                    Ok(Err(failed)) => {
                        assert!(failed.is_canceled());
                        canceled += 1;
                    }
                    Err(request) => still.push(request),
                }
            }
            pending = still;
        }
        assert_eq!((ok, canceled), (4, 4));
    }

    #[test]
    fn drop_cancels_completes_then_drops_data() {
        let comms = setup_default();
        let drops = Rc::new(Cell::new(0));
        let buf = CountingBuf {
            bytes: vec![0u8; 8],
            drops: Rc::clone(&drops),
        };
        let request = post_recv(&comms.worker, buf, 0x50)
            .into_pending()
            .expect("pending");
        assert_eq!(drops.get(), 0);
        drop(request);
        assert_eq!(drops.get(), 1, "data dropped after cancel-completion");
    }

    #[test]
    fn drop_timeout_leaks_request_and_data() {
        let comms = setup_default();
        let drops = Rc::new(Cell::new(0));
        let buf = CountingBuf {
            bytes: vec![0u8; 8],
            drops: Rc::clone(&drops),
        };
        let mut request = post_recv(&comms.worker, buf, 0x60)
            .into_pending()
            .expect("pending");
        let raw = request.request.as_ref().unwrap().as_ptr();
        // No cancel, tiny budget: an unmatched receive cannot complete.
        request.finish_or_leak(4, false);
        assert!(request.request.is_none() && request.data.is_none());
        drop(request);
        assert_eq!(drops.get(), 0, "timed-out data must be leaked, not dropped");
        // Clean up the leaked UCX request so the worker shuts down quietly.
        // The leaked buffer stays allocated, so UCX completing into it is fine.
        // SAFETY: `raw` is the live, never-freed request leaked above.
        unsafe {
            ucp_request_cancel(comms.worker.handle, raw);
            for _ in 0..1000 {
                if ucp_request_check_status(raw) != ucs_status_t::UCS_INPROGRESS {
                    break;
                }
                comms.worker.progress();
            }
            ucp_request_free(raw);
        }
    }
}
