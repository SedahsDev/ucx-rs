use crate::ep::Ep;
use crate::ep::EpHandle;
use crate::ffi::*;

use crate::completion::{owned_op_param, status_ptr_with_data};
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::Worker;
use crate::RequestParam;
use crate::Status;
use crate::{Completion, OwnedBuf, OwningRequest};
use bitflags::bitflags;
use std::sync::{Arc, Mutex};

/// Raw active-message receive callback. It returns a [`Status`]; `Status` is
/// `#[repr(transparent)]` over the raw status, so this matches UCX's C callback type.
pub type AmRecvCb = unsafe extern "C" fn(
    arg: *mut ::std::os::raw::c_void,
    header: *const ::std::os::raw::c_void,
    header_length: usize,
    data: *mut ::std::os::raw::c_void,
    length: usize,
    param: *const AmRecvParam,
) -> Status;

const AM_RECV_ATTR_FIELD_REPLY_EP: u64 = ucp_am_recv_attr_t::UCP_AM_RECV_ATTR_FIELD_REPLY_EP as u64;
const AM_RECV_ATTR_FLAG_DATA: u64 = ucp_am_recv_attr_t::UCP_AM_RECV_ATTR_FLAG_DATA as u64;
const AM_RECV_ATTR_FLAG_RNDV: u64 = ucp_am_recv_attr_t::UCP_AM_RECV_ATTR_FLAG_RNDV as u64;

bitflags! {
    /// Receive attributes UCX reports to an [`AmRecvCb`] through [`AmRecvParam::recv_attr`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct AmRecvAttr: u64 {
        /// `UCP_AM_RECV_ATTR_FIELD_REPLY_EP`: UCX provided a reply endpoint.
        const REPLY_EP = AM_RECV_ATTR_FIELD_REPLY_EP;
        /// `UCP_AM_RECV_ATTR_FLAG_DATA`: `data` is a descriptor the handler may keep.
        const DATA = AM_RECV_ATTR_FLAG_DATA;
        /// `UCP_AM_RECV_ATTR_FLAG_RNDV`: `data` is a rendezvous descriptor.
        const RNDV = AM_RECV_ATTR_FLAG_RNDV;
    }
}

/// Receive parameters UCX passes to an [`AmRecvCb`].
///
/// UCX owns this value. A callback receives it as `*const AmRecvParam`, valid only for the
/// duration of the call. `#[repr(transparent)]` gives it exactly the layout of UCX's C struct.
#[repr(transparent)]
pub struct AmRecvParam(ucp_am_recv_param_t);

// `AmRecvCb` receives `*const AmRecvParam` in place of UCX's `*const ucp_am_recv_param_t`.
const _: () = {
    assert!(std::mem::size_of::<AmRecvParam>() == std::mem::size_of::<ucp_am_recv_param_t>());
    assert!(std::mem::align_of::<AmRecvParam>() == std::mem::align_of::<ucp_am_recv_param_t>());
};

impl AmRecvParam {
    /// The receive attributes UCX reported for this message. Unknown bits are kept.
    pub fn recv_attr(&self) -> AmRecvAttr {
        AmRecvAttr::from_bits_retain(self.0.recv_attr)
    }

    /// The endpoint to reply on, if UCX provided one: [`AmRecvAttr::REPLY_EP`] is set and the
    /// handle is non-null. The handle is borrowed from UCX; never close it.
    pub fn reply_ep(&self) -> Option<EpHandle> {
        if self.recv_attr().contains(AmRecvAttr::REPLY_EP) && !self.0.reply_ep.is_null() {
            Some(EpHandle(self.0.reply_ep))
        } else {
            None
        }
    }
}

impl std::fmt::Debug for AmRecvParam {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AmRecvParam")
            .field("recv_attr", &self.recv_attr())
            .field("reply_ep", &self.reply_ep())
            .finish()
    }
}

type AmCallback = Box<dyn FnMut(&[u8], &[u8]) -> Result<(), Status> + Send + 'static>;

/// The Rust state retained by [`Worker::am_register_handler`].
pub struct AmHandler {
    inner: Mutex<AmCallback>,
    /// Worker the handler is registered on, for rendezvous receives started
    /// from the trampoline. Valid while the handler is: `Worker` retains every
    /// `AmHandler` until after `ucp_worker_destroy`.
    worker: ucp_worker_h,
}

// SAFETY: `worker` is only dereferenced by UCX, from the trampoline and the
// rendezvous completion callback, which run in that worker's progress context.
unsafe impl Send for AmHandler {}
unsafe impl Sync for AmHandler {}

impl std::fmt::Debug for AmHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AmHandler").finish_non_exhaustive()
    }
}

/// Invoke the safe closure, containing panics. Returns the closure's status
/// mapped to what UCX accepts (an error code, or `OK`).
fn call_handler(handler: &AmHandler, header: &[u8], data: &[u8]) -> Status {
    let mut callback = match handler.inner.lock() {
        Ok(callback) => callback,
        // A panic poisons the mutex. Do not invoke a possibly inconsistent
        // handler again; report the callback failure to UCX instead.
        Err(_) => return Status::IO_ERROR,
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(header, data))) {
        Ok(Ok(())) => Status::OK,
        Ok(Err(status)) if status.is_err() => status,
        // A non-error status in `Err` (for example `IN_PROGRESS`) must never reach UCX
        Ok(Err(_)) => Status::INVALID_PARAM,
        // Handler panics are contained here and reported as a UCX error; they
        // never unwind through this extern "C" trampoline into UCX.
        Err(_) => Status::IO_ERROR,
    }
}

/// A rendezvous receive started by the trampoline: owns the copied header and
/// the payload buffer UCX writes into, until the receive completes.
struct RndvRecv {
    handler: Arc<AmHandler>,
    header: Vec<u8>,
    buffer: Vec<u8>,
}

/// Completion of a trampoline-started rendezvous receive (`cb.recv_am`).
unsafe extern "C" fn am_rndv_done(
    request: *mut std::os::raw::c_void,
    status: ucs_status_t,
    length: usize,
    user_data: *mut std::os::raw::c_void,
) {
    // SAFETY: `user_data` is the `Box<RndvRecv>` leaked by `am_trampoline`
    // for this request; UCX calls this exactly once.
    let mut recv = unsafe { Box::from_raw(user_data as *mut RndvRecv) };
    if status == ucs_status_t::UCS_OK {
        recv.buffer.truncate(length);
        // The sender already completed (ATS), so the status has nowhere to go.
        let _ = call_handler(&recv.handler, &recv.header, &recv.buffer);
    }
    // SAFETY: `request` is the live handle UCX returned to the trampoline,
    // which did not free it; it is complete now.
    unsafe { ucp_request_free(request) };
    drop(recv);
}

/// Start receiving a rendezvous message into an owned buffer. The closure runs
/// when the payload has arrived (possibly before this returns).
fn am_rndv_start(
    handler: &AmHandler,
    arg: *const AmHandler,
    header: &[u8],
    desc: *mut std::os::raw::c_void,
    length: usize,
) -> Status {
    let mut buffer = Vec::new();
    if buffer.try_reserve_exact(length).is_err() {
        // recv_data not called: UCX drops the descriptor and completes the
        // sender with this status.
        return Status::NO_MEMORY;
    }
    buffer.resize(length, 0);
    // SAFETY: `arg` is the `Arc<AmHandler>` pointer installed by
    // `am_register_handler`; Worker holds a strong reference for its lifetime.
    let handler_arc = unsafe {
        Arc::increment_strong_count(arg);
        Arc::from_raw(arg)
    };
    let recv = Box::into_raw(Box::new(RndvRecv {
        handler: handler_arc,
        header: header.to_vec(),
        buffer,
    }));
    // SAFETY: zeroed POD; only the fields named in op_attr_mask are read.
    let mut param: ucp_request_param_t = unsafe { std::mem::zeroed() };
    param.op_attr_mask = ucp_op_attr_t::UCP_OP_ATTR_FIELD_CALLBACK as u32
        | ucp_op_attr_t::UCP_OP_ATTR_FIELD_USER_DATA as u32;
    param.cb.recv_am = Some(am_rndv_done);
    param.user_data = recv as *mut std::os::raw::c_void;
    // SAFETY: `desc` is the rendezvous descriptor of this callback invocation;
    // the buffer is heap storage owned by `recv` until completion. UCX takes
    // ownership of `desc` whatever the outcome.
    let ptr = unsafe {
        let buffer = &mut (*recv).buffer;
        ucp_am_recv_data_nbx(
            handler.worker,
            desc,
            buffer.as_mut_ptr() as *mut std::os::raw::c_void,
            length,
            &param,
        )
    };
    if ptr.is_null() {
        // Completed in the call; the callback is not invoked.
        // SAFETY: not handed to UCX as a pending request.
        let recv = unsafe { Box::from_raw(recv) };
        let _ = call_handler(&recv.handler, &recv.header, &recv.buffer);
    } else if crate::request::status_ptr_is_err(ptr) {
        // Never started: no callback. The data is lost; UCX owns `desc`.
        // SAFETY: not handed to UCX as a pending request.
        drop(unsafe { Box::from_raw(recv) });
    }
    // Pending: `am_rndv_done` frees `recv` and the request. If the worker is
    // destroyed first and UCX never completes it, `recv` leaks (never freed
    // while UCX may still write the buffer).
    //
    // recv_data was called: UCX now tracks the descriptor itself and ignores
    // anything but OK here (ucp_am.c, RNDV RTS handling).
    Status::OK
}

unsafe extern "C" fn am_trampoline(
    arg: *mut std::os::raw::c_void,
    header: *const std::os::raw::c_void,
    header_length: usize,
    data: *mut std::os::raw::c_void,
    length: usize,
    param: *const AmRecvParam,
) -> Status {
    // SAFETY: `arg` is an Arc<AmHandler> pointer installed by
    // am_register_handler and retained by Worker until after UCX destroys the
    // worker. UCX owns the callback buffers for this invocation; null pointers
    // are permitted for zero-length messages.
    let handler = unsafe { &*(arg as *const AmHandler) };
    let header = if header.is_null() && header_length == 0 {
        &[]
    } else if header.is_null() {
        return Status::INVALID_PARAM;
    } else {
        unsafe { std::slice::from_raw_parts(header as *const u8, header_length) }
    };
    // SAFETY: UCX passes a valid param pointer (or null) for this invocation.
    let rndv = unsafe { param.as_ref() }
        .map(|param| param.recv_attr().contains(AmRecvAttr::RNDV))
        .unwrap_or(false);
    if rndv {
        // `data` is a rendezvous descriptor, not `length` bytes of payload
        // (#214): fetch the payload with ucp_am_recv_data_nbx first.
        if data.is_null() {
            return Status::INVALID_PARAM;
        }
        return am_rndv_start(handler, arg as *const AmHandler, header, data, length);
    }
    let data = if data.is_null() && length == 0 {
        &[]
    } else if data.is_null() {
        return Status::INVALID_PARAM;
    } else {
        unsafe { std::slice::from_raw_parts(data as *const u8, length) }
    };
    call_handler(handler, header, data)
}

impl Worker {
    /// Register a safe AM receive callback. Only one callback is retained by
    /// this wrapper for a worker; registering again replaces the Rust closure.
    /// UCX invokes it in the progress context, so it must not block or call
    /// back into this worker; send heavy work to an application channel.
    /// The closure returns `Ok(())` on success (reported to UCX as `UCS_OK`); an `Err(status)` is
    /// reported to UCX as that status if it is an error code, otherwise as `Status::INVALID_PARAM`;
    /// `UCS_INPROGRESS` is never returned to UCX.
    ///
    /// Rendezvous messages (large payloads, or `UCP_AM_SEND_FLAG_RNDV`) are
    /// received into an owned buffer first; the closure then sees the payload
    /// like any other message, from the progress call that completes the
    /// receive (possibly a later one). For those, the closure's status cannot
    /// reach the sender (it has already completed) and is ignored; if the
    /// buffer cannot be allocated the sender completes with
    /// `UCS_ERR_NO_MEMORY`. A rendezvous receive still pending when the worker
    /// is destroyed leaks its buffer rather than freeing memory UCX may write.
    /// Use [`Self::am_unregister`] to remove the handler again.
    pub fn am_register_handler<F>(
        &mut self,
        id: u32,
        flags: CbFlags,
        handler: F,
    ) -> Result<(), Status>
    where
        F: FnMut(&[u8], &[u8]) -> Result<(), Status> + Send + 'static,
    {
        let handler = Arc::new(AmHandler {
            inner: Mutex::new(Box::new(handler)),
            worker: self.handle,
        });
        let params = HandlerParamsBuilder::new()
            .id(id)
            .flags(flags)
            .cb(am_trampoline)
            .arg(Arc::as_ptr(&handler) as *mut std::ffi::c_void)
            .build();
        status_to_result(unsafe { ucp_worker_set_am_recv_handler(self.handle, &params.handle) })?;
        // Keep replaced and unregistered handlers (see `Worker::am_unregister`) alive until
        // worker destruction because UCX may still dispatch an in-flight callback using the
        // previous opaque argument (`arg` pointer).
        //
        // Cleanup strategy (Issue #100):
        // - When a new handler is registered, the old `AmHandler` Arc is pushed to
        //   `Worker::am_handlers` vector and retained.
        // - The old handler's memory remains valid because the UCX callback pointer
        //   still references it; dropping it prematurely would cause use-after-free.
        // - When the worker is destroyed via `ucp_worker_destroy`, UCX will eventually
        //   destroy all pending callbacks. By then, all Rust-side `AmHandler` instances
        //   are still alive and will be dropped safely when `Worker`'s Drop impl
        //   destroys the `am_handlers` vector.
        //
        // This ensures no dangling callbacks and no FFI leaks.
        self.am_handlers.push(handler);
        Ok(())
    }

    #[inline]
    /// Register a raw AM callback. The callback runs in the progress context:
    /// the thread calling `Worker::progress()`, or UCX-internal progress under
    /// MULTI. Do not block or call back into the same worker; hop heavy work to
    /// an application thread or channel. See `THREADING.md` section 4.
    ///
    /// UCX stores the callback pointer and the `arg` pointer, not a copy of the data `arg`
    /// points to. The caller owns that data and must keep it valid until the handler for this
    /// id is replaced, removed with [`Self::am_unregister`], or the worker is dropped.
    pub fn am_register(&self, am_param: &HandlerParams) -> Result<(), Status> {
        status_to_result(unsafe { ucp_worker_set_am_recv_handler(self.handle, &am_param.handle) })
    }

    /// Remove the active-message handler registered for `id`, whether it was installed with
    /// [`Self::am_register`] or [`Self::am_register_handler`].
    ///
    /// UCX clears a handler when it is set again with a null callback, which is what this
    /// does. Removing an id that has no handler succeeds. Rust state of a closure registered
    /// with [`Self::am_register_handler`] stays allocated until the worker is dropped, because
    /// under `ThreadMode::Multi` UCX may still be running the old callback on another
    /// thread when this returns. Do not send messages to an id that has no handler (see #111).
    pub fn am_unregister(&self, id: u32) -> Result<(), Status> {
        // SAFETY: UCX parameter structs are valid when zeroed; the field mask controls reads.
        let mut param: ucp_am_handler_param_t = unsafe { std::mem::zeroed() };
        param.field_mask = ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ID as u64
            | ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_CB as u64;
        param.id = id;
        // A null callback is UCX's documented way to clear the handler for `id`.
        param.cb = None;
        // SAFETY: self.handle is a live worker and `param` is fully initialized.
        status_to_result(unsafe { ucp_worker_set_am_recv_handler(self.handle, &param) })
    }
}

impl Ep {
    /// Send an active message.
    ///
    /// # Precondition
    ///
    /// An AM handler must be registered on the receiving worker for the
    /// given `id` before calling this method. For self endpoints, the
    /// receiving worker is the caller's own worker. Calling `am_send` with
    /// an unregistered `id` on a self endpoint will cause undefined behavior
    /// (SIGSEGV in UCX).
    ///
    /// The header and payload are moved in and come back together as
    /// `(header, data)` from [`Completion::Immediate`] or
    /// [`OwningRequest::try_complete`]; neither can be touched while UCX may
    /// read it. Use `b""` for an empty header or payload. `worker`
    /// must be the worker this endpoint was created on. `params` contributes
    /// flags (including `UCP_AM_SEND_FLAG_*`), callback, and user data; the
    /// datatype (contiguous bytes), reply buffer, receive info, memory handle,
    /// memory type, and request allocation fields are controlled here.
    #[inline]
    #[allow(clippy::type_complexity)]
    pub fn am_send<'w, H: OwnedBuf, P: OwnedBuf>(
        &self,
        worker: &'w Worker,
        id: u32,
        header: H,
        data: P,
        params: &RequestParam,
    ) -> Result<Completion<(H, P), OwningRequest<'w, (H, P)>>, Status> {
        debug_assert!(
            std::sync::Arc::ptr_eq(&self.worker_alive, &worker.alive),
            "am_send: the passed worker is not the endpoint's worker"
        );
        let raw = owned_op_param(params);
        // SAFETY: the request (if any) reads only the header's and payload's
        // stable storage (`OwnedBuf`), which the returned value owns until
        // UCX is done with both.
        unsafe {
            let ptr = ucp_am_send_nbx(
                self.handle,
                id,
                header.as_ptr() as _,
                header.len(),
                data.as_ptr() as _,
                data.len(),
                &raw,
            );
            status_ptr_with_data(ptr, (header, data), worker)
        }
    }
}

const AM_CB_FLAG_WHOLE_MSG: u32 = ucp_am_cb_flags::UCP_AM_FLAG_WHOLE_MSG as u32;
const AM_CB_FLAG_PERSISTENT_DATA: u32 = ucp_am_cb_flags::UCP_AM_FLAG_PERSISTENT_DATA as u32;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct CbFlags: u32 {
        const WholeMsg = AM_CB_FLAG_WHOLE_MSG;
        const PersistentData = AM_CB_FLAG_PERSISTENT_DATA;
    }
}

impl From<ucp_am_cb_flags> for CbFlags {
    fn from(flag: ucp_am_cb_flags) -> Self {
        CbFlags::from_bits_truncate(flag as u32)
    }
}

#[derive(Debug, Clone)]
pub struct HandlerParamsBuilder {
    uninit_handle: std::mem::MaybeUninit<ucp_am_handler_param_t>,
    flags: u64,
}

impl Default for HandlerParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl HandlerParamsBuilder {
    #[inline]
    pub fn new() -> HandlerParamsBuilder {
        // SAFETY: UCX parameter structs are valid when zeroed; the field mask controls reads.
        let uninit_params = std::mem::MaybeUninit::new(unsafe { std::mem::zeroed() });
        HandlerParamsBuilder {
            uninit_handle: uninit_params,
            flags: 0,
        }
    }

    #[inline]
    pub fn id(&mut self, id: u32) -> &mut HandlerParamsBuilder {
        self.flags |= ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ID as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.id = id;
        self
    }

    #[inline]
    pub fn flags(&mut self, flags: CbFlags) -> &mut HandlerParamsBuilder {
        self.flags |= ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_FLAGS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.flags = flags.bits();
        self
    }

    #[inline]
    pub fn cb(&mut self, cb: AmRecvCb) -> &mut HandlerParamsBuilder {
        self.flags |= ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_CB as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        // SAFETY: `Status` is `#[repr(transparent)]` over `ucs_status_t`, so `AmRecvCb` and UCX's
        // `ucp_am_recv_callback_t` have ABI-compatible signatures.
        params.cb =
            unsafe { std::mem::transmute::<Option<AmRecvCb>, ucp_am_recv_callback_t>(Some(cb)) };
        self
    }

    #[inline]
    pub fn arg(&mut self, arg: *mut std::os::raw::c_void) -> &mut HandlerParamsBuilder {
        self.flags |= ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ARG as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.arg = arg;
        self
    }

    #[inline]
    pub fn build(&mut self) -> HandlerParams {
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.field_mask = self.flags;

        HandlerParams {
            handle: unsafe { self.uninit_handle.assume_init() },
        }
    }
}

pub struct HandlerParams {
    pub(crate) handle: ucp_am_handler_param_t,
}

/// Receive active message data.
///
/// # Safety
/// Caller must ensure `data_desc` is a valid data descriptor from the AM handler.
#[deprecated(since = "0.1.0", note = "Use Worker::am_recv_data instead")]
pub unsafe fn am_recv_data_nbx(
    worker: &crate::worker::Worker,
    data_desc: *mut std::os::raw::c_void,
    buffer: *mut std::os::raw::c_void,
    count: usize,
) -> crate::Request {
    let ptr = ucp_am_recv_data_nbx(worker.handle, data_desc, buffer, count, std::ptr::null());
    crate::Request::from_raw(ptr)
}

/// Release active message data.
///
/// # Safety
/// Caller must ensure `data` was obtained from an AM receive handler.
#[deprecated(since = "0.1.0", note = "Use Worker::am_data_release() instead")]
pub unsafe fn am_data_release(worker: &crate::worker::Worker, data: *mut std::os::raw::c_void) {
    ucp_am_data_release(worker.handle, data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Config, Context, Flags, ParamsBuilder as ContextParamsBuilder};
    use crate::ep::ParamsBuilder as EpParamsBuilder;
    use crate::worker::{ParamsBuilder as WorkerParamsBuilder, RemoteWorkerAddress};
    use std::sync::atomic::{AtomicU32, Ordering};

    type AmRecvDataFn =
        for<'w> unsafe fn(
            &'w Worker,
            std::ptr::NonNull<std::ffi::c_void>,
            Vec<u8>,
            &RequestParam,
        )
            -> Result<Completion<Vec<u8>, OwningRequest<'w, Vec<u8>>>, Status>;

    #[test]
    fn test_worker_am_receive_api_signatures() {
        let _recv: AmRecvDataFn = Worker::am_recv_data;
        let _release: fn(&Worker, std::ptr::NonNull<std::ffi::c_void>) = Worker::am_data_release;
    }

    #[test]
    fn safe_handler_receives_self_am() {
        let context_params = ContextParamsBuilder::new()
            .features(Flags::Am)
            .mt_workers_shared(1)
            .build();
        let mut context = Context::new(&Config::read("", "").unwrap(), &context_params).unwrap();
        let worker_params = WorkerParamsBuilder::new().build();
        let mut worker = context.worker_create(&worker_params).unwrap();
        let packed = worker.pack_address().unwrap();
        let address = RemoteWorkerAddress::new(packed.to_vec());
        drop(packed);
        let endpoint = worker
            .create_ep(EpParamsBuilder::new().address(&address).build())
            .unwrap();
        let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let seen = Arc::clone(&count);
        worker
            .am_register_handler(23, CbFlags::WholeMsg, move |header, data| {
                assert_eq!(header, b"h");
                assert_eq!(data, b"d");
                seen.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
            .unwrap();
        let request_param = crate::RequestParamBuilder::new().no_imm_cmpl().build();
        let request = endpoint
            .am_send(&worker, 23, b"h", b"d", &request_param)
            .unwrap();
        for _ in 0..1000 {
            worker.progress();
            if count.load(Ordering::Relaxed) == 1 {
                break;
            }
        }
        drop(request);
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    /// `am_send` hands back both owned buffers through the deferred path.
    /// Eager-sized on purpose: the safe handler trampoline does not handle
    /// rendezvous descriptors yet (#214).
    #[test]
    fn owned_am_send_returns_header_and_payload() {
        let context_params = ContextParamsBuilder::new()
            .features(Flags::Am)
            .mt_workers_shared(1)
            .build();
        let mut context = Context::new(&Config::read("", "").unwrap(), &context_params).unwrap();
        let mut worker = context
            .worker_create(&WorkerParamsBuilder::new().build())
            .unwrap();
        let packed = worker.pack_address().unwrap();
        let address = RemoteWorkerAddress::new(packed.to_vec());
        drop(packed);
        let endpoint = worker
            .create_ep(EpParamsBuilder::new().address(&address).build())
            .unwrap();
        let received = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = Arc::clone(&received);
        worker
            .am_register_handler(25, CbFlags::WholeMsg, move |header, data| {
                assert_eq!(header, b"hdr");
                assert!(data.iter().all(|&byte| byte == 0x5A));
                seen.store(data.len(), Ordering::Release);
                Ok(())
            })
            .unwrap();
        let len = 1024;
        // EAGER keeps this test on the in-callback payload path; rendezvous is
        // covered by `safe_handler_receives_rendezvous_payload`.
        let param = crate::RequestParamBuilder::new()
            .no_imm_cmpl()
            .flags(ucp_send_am_flags::UCP_AM_SEND_FLAG_EAGER as u32)
            .build();
        let completion = endpoint
            .am_send(&worker, 25, b"hdr".to_vec(), vec![0x5Au8; len], &param)
            .unwrap();
        assert!(completion.is_pending(), "NO_IMM_CMPL must defer");
        let mut request = completion.into_pending().unwrap();
        let mut done = None;
        for _ in 0..1_000_000 {
            worker.progress();
            match request.try_complete() {
                Ok(result) => {
                    done = Some(result.expect("am_send"));
                    break;
                }
                Err(still) => request = still,
            }
        }
        let (header, data) = done.expect("am_send did not complete");
        assert_eq!(header, b"hdr");
        assert_eq!(data, vec![0x5Au8; len], "payload handed back unchanged");
        for _ in 0..1000 {
            if received.load(Ordering::Acquire) == len {
                break;
            }
            worker.progress();
        }
        assert_eq!(received.load(Ordering::Acquire), len);
    }

    /// Safe-handler AM test fixture: self endpoint with `Flags::Am`.
    fn am_self() -> (Context, Worker, crate::ep::Ep) {
        let context_params = ContextParamsBuilder::new()
            .features(Flags::Am)
            .mt_workers_shared(1)
            .build();
        let mut context = Context::new(&Config::read("", "").unwrap(), &context_params).unwrap();
        let worker = context
            .worker_create(&WorkerParamsBuilder::new().build())
            .unwrap();
        let packed = worker.pack_address().unwrap();
        let address = RemoteWorkerAddress::new(packed.to_vec());
        drop(packed);
        let endpoint = worker
            .create_ep(EpParamsBuilder::new().address(&address).build())
            .unwrap();
        (context, worker, endpoint)
    }

    /// #214: a rendezvous message reaches the safe closure as its real
    /// payload, not as descriptor memory read out of bounds.
    #[test]
    fn safe_handler_receives_rendezvous_payload() {
        let (_context, mut worker, endpoint) = am_self();
        let len = 4 << 20;
        let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let expected = payload.clone();
        let received = Arc::new(Mutex::new(None::<(Vec<u8>, bool)>));
        let seen = Arc::clone(&received);
        worker
            .am_register_handler(27, CbFlags::WholeMsg, move |header, data| {
                let ok = header == b"rndv-hdr" && data == expected.as_slice();
                *seen.lock().unwrap() = Some((header.to_vec(), ok));
                Ok(())
            })
            .unwrap();
        let param = crate::RequestParamBuilder::new()
            .flags(ucp_send_am_flags::UCP_AM_SEND_FLAG_RNDV as u32)
            .build();
        let mut send = Some(
            endpoint
                .am_send(&worker, 27, b"rndv-hdr".to_vec(), payload, &param)
                .unwrap(),
        );
        let mut sent = false;
        for _ in 0..1_000_000 {
            match send.take() {
                Some(Completion::Immediate(_)) => sent = true,
                Some(Completion::Pending(request)) => match request.try_complete() {
                    Ok(result) => {
                        if let Err(failed) = result {
                            panic!("am_send: {failed}");
                        }
                        sent = true;
                    }
                    Err(still) => send = Some(Completion::Pending(still)),
                },
                None => {}
            }
            if sent && received.lock().unwrap().is_some() {
                break;
            }
            worker.progress();
        }
        assert!(sent, "am_send did not complete");
        let (header, ok) = received
            .lock()
            .unwrap()
            .take()
            .expect("handler not invoked");
        assert_eq!(header, b"rndv-hdr");
        assert!(ok, "rendezvous payload differs from what was sent");
    }

    /// A safe closure that panics on a rendezvous message is contained in the
    /// completion callback (no unwind into UCX) and the sender still completes.
    #[test]
    fn safe_handler_rendezvous_panic_is_contained() {
        let (_context, mut worker, endpoint) = am_self();
        let calls = Arc::new(AtomicU32::new(0));
        let seen = Arc::clone(&calls);
        worker
            .am_register_handler(28, CbFlags::WholeMsg, move |_header, _data| {
                seen.fetch_add(1, Ordering::SeqCst);
                panic!("handler panic (expected by test)");
            })
            .unwrap();
        let param = crate::RequestParamBuilder::new()
            .flags(ucp_send_am_flags::UCP_AM_SEND_FLAG_RNDV as u32)
            .build();
        let mut send = Some(
            endpoint
                .am_send(&worker, 28, b"".as_slice(), vec![1u8; 1 << 20], &param)
                .unwrap(),
        );
        let mut sent = false;
        for _ in 0..1_000_000 {
            match send.take() {
                Some(Completion::Immediate(_)) => sent = true,
                Some(Completion::Pending(request)) => match request.try_complete() {
                    Ok(_) => sent = true,
                    Err(still) => send = Some(Completion::Pending(still)),
                },
                None => {}
            }
            if sent && calls.load(Ordering::SeqCst) == 1 {
                break;
            }
            worker.progress();
        }
        assert!(sent, "am_send did not complete");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// Descriptor + length recorded by `keep_rndv_desc` (test only).
    struct RndvSeen {
        desc: usize,
        length: usize,
        rndv: bool,
    }

    unsafe extern "C" fn keep_rndv_desc(
        arg: *mut std::os::raw::c_void,
        _header: *const std::os::raw::c_void,
        _header_length: usize,
        data: *mut std::os::raw::c_void,
        length: usize,
        param: *const AmRecvParam,
    ) -> Status {
        // SAFETY: `arg` points at the test's RndvSeen, alive for the test.
        let seen = unsafe { &mut *(arg as *mut RndvSeen) };
        seen.desc = data as usize;
        seen.length = length;
        seen.rndv = unsafe { param.as_ref() }
            .map(|param| param.recv_attr().contains(AmRecvAttr::RNDV))
            .unwrap_or(false);
        // Keep the rendezvous descriptor; it is received outside the callback.
        Status::IN_PROGRESS
    }

    /// `am_recv_data` owns its buffer: a rendezvous descriptor kept by the
    /// callback is received afterwards into an owned `Vec`, which comes back
    /// filled from the completion.
    #[test]
    fn owned_am_recv_data_rendezvous_round_trip() {
        let context_params = ContextParamsBuilder::new()
            .features(Flags::Am)
            .mt_workers_shared(1)
            .build();
        let mut context = Context::new(&Config::read("", "").unwrap(), &context_params).unwrap();
        let worker = context
            .worker_create(&WorkerParamsBuilder::new().build())
            .unwrap();
        let packed = worker.pack_address().unwrap();
        let address = RemoteWorkerAddress::new(packed.to_vec());
        drop(packed);
        let endpoint = worker
            .create_ep(EpParamsBuilder::new().address(&address).build())
            .unwrap();
        let mut seen = Box::new(RndvSeen {
            desc: 0,
            length: 0,
            rndv: false,
        });
        let handler = HandlerParamsBuilder::new()
            .id(26)
            .cb(keep_rndv_desc)
            .arg(&mut *seen as *mut RndvSeen as *mut std::os::raw::c_void)
            .build();
        worker.am_register(&handler).unwrap();
        let len = 1 << 20;
        let payload: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
        let send_param = crate::RequestParamBuilder::new()
            .flags(ucp_send_am_flags::UCP_AM_SEND_FLAG_RNDV as u32)
            .build();
        let mut send = Some(
            endpoint
                .am_send(&worker, 26, b"", payload.clone(), &send_param)
                .unwrap(),
        );
        for _ in 0..1_000_000 {
            if seen.desc != 0 {
                break;
            }
            worker.progress();
        }
        assert!(seen.rndv, "RNDV flag must make UCX hand over a descriptor");
        assert_eq!(seen.length, len);
        let desc = std::ptr::NonNull::new(seen.desc as *mut std::ffi::c_void).unwrap();
        // SAFETY: `desc` is the RNDV descriptor kept by `keep_rndv_desc`
        // (returned IN_PROGRESS) and is received exactly once.
        let recv = unsafe {
            worker.am_recv_data(
                desc,
                vec![0u8; len],
                &crate::RequestParamBuilder::new().build(),
            )
        }
        .expect("am_recv_data");
        let mut received = None;
        let mut recv = Some(recv);
        for _ in 0..1_000_000 {
            if let Some(Completion::Pending(request)) = send.take() {
                match request.try_complete() {
                    Ok(result) => drop(result.expect("am_send")),
                    Err(still) => send = Some(Completion::Pending(still)),
                }
            }
            match recv.take() {
                Some(Completion::Immediate(buffer)) => received = Some(buffer),
                Some(Completion::Pending(request)) => match request.try_complete() {
                    Ok(result) => received = Some(result.expect("am_recv_data")),
                    Err(still) => recv = Some(Completion::Pending(still)),
                },
                None => {}
            }
            if received.is_some() && !matches!(send, Some(Completion::Pending(_))) {
                break;
            }
            worker.progress();
        }
        let received = received.expect("am_recv_data did not complete");
        assert!(received == payload, "received bytes differ");
    }

    #[test]
    fn am_recv_param_reply_ep_requires_field_bit() {
        let fake = 0x40usize as ucp_ep_h;
        // SAFETY: the raw receive-param struct is plain C data; all-zero is valid.
        let mut raw: ucp_am_recv_param_t = unsafe { std::mem::zeroed() };
        raw.reply_ep = fake;
        let without_bit = AmRecvParam(raw);
        assert!(without_bit.recv_attr().is_empty());
        assert!(without_bit.reply_ep().is_none());

        raw.recv_attr = AmRecvAttr::REPLY_EP.bits() | AmRecvAttr::DATA.bits();
        let with_bit = AmRecvParam(raw);
        assert!(with_bit.recv_attr().contains(AmRecvAttr::DATA));
        assert_eq!(with_bit.reply_ep().map(|ep| ep.0), Some(fake));

        raw.reply_ep = std::ptr::null_mut();
        let null_ep = AmRecvParam(raw);
        assert!(null_ep.reply_ep().is_none());
    }

    unsafe extern "C" fn record_recv_param(
        arg: *mut std::os::raw::c_void,
        header: *const std::os::raw::c_void,
        header_length: usize,
        _data: *mut std::os::raw::c_void,
        _length: usize,
        param: *const AmRecvParam,
    ) -> Status {
        let seen = &mut *(arg as *mut [u8; 2]);
        if header_length == 1 && !header.is_null() {
            seen[0] = *(header as *const u8);
        }
        seen[1] = match param.as_ref() {
            Some(param) if param.reply_ep().is_none() => 1,
            Some(_) => 2,
            None => 3,
        };
        Status::OK
    }

    #[test]
    fn raw_callback_receives_native_recv_param() {
        let comms = crate::tests::setup_default();
        let mut seen = [0u8; 2];
        let handler = HandlerParamsBuilder::new()
            .id(24)
            .cb(record_recv_param)
            .arg(seen.as_mut_ptr() as *mut std::os::raw::c_void)
            .build();
        comms.worker.am_register(&handler).unwrap();
        let request_param = crate::RequestParamBuilder::new().no_imm_cmpl().build();
        let request = comms
            .ep
            .am_send(&comms.worker, 24, b"q", b"", &request_param)
            .unwrap();
        for _ in 0..1000 {
            if seen[0] != 0 {
                break;
            }
            comms.worker.progress();
        }
        drop(request);
        // No reply flag was sent, so UCX must not report a reply endpoint.
        assert_eq!(seen, [b'q', 1]);
    }
}
