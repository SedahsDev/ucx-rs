use crate::completion::{owned_op_param, owned_recv_param, sealed, status_ptr_with_data, RecvInfo};
use crate::ep::Ep;
use crate::ffi::*;
use crate::request::Pending;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Request;
use crate::RequestParam;
use crate::Status;
use crate::{Completion, OwnedBuf, OwnedBufMut, OwningRequest, TagRecvRequest};
use bitflags::bitflags;

impl Ep {
    /// Send a tagged message, taking ownership of `data`.
    ///
    /// Returns [`Completion::Immediate`] with the buffer when the send
    /// finishes inline, or [`Completion::Pending`] with an [`OwningRequest`]
    /// that hands it back from [`OwningRequest::try_complete`]. `worker` must
    /// be the worker this endpoint was created on.
    ///
    /// `param` contributes flags, callback, and user data. The datatype
    /// (always contiguous bytes), reply buffer, receive info, memory handle,
    /// memory type, and request allocation fields are controlled here.
    ///
    /// The buffer is moved in, so it cannot be touched while UCX reads it
    /// (the use-after-free from #112 is a compile error):
    ///
    /// ```compile_fail
    /// # use ucx_sys::{ep::Ep, worker::Worker, RequestParam};
    /// fn reuse(ep: &Ep, worker: &Worker, param: &RequestParam) {
    ///     let data = vec![0u8; 64];
    ///     let pending = ep.tag_send(worker, data, 7, param).unwrap();
    ///     drop(data);
    ///     drop(pending);
    /// }
    /// ```
    pub fn tag_send<'w, D: OwnedBuf>(
        &self,
        worker: &'w Worker,
        data: D,
        tag: u64,
        param: &RequestParam,
    ) -> Result<Completion<D, OwningRequest<'w, D>>, Status> {
        debug_assert!(
            std::sync::Arc::ptr_eq(&self.worker_alive, &worker.alive),
            "tag_send: the passed worker is not the endpoint's worker"
        );
        let raw = owned_op_param(param);
        // SAFETY: the request (if any) reads only `data`'s stable storage
        // (`OwnedBuf`), which the returned value owns until UCX is done.
        unsafe {
            let ptr = ucp_tag_send_nbx(self.handle, data.as_ptr() as _, data.len(), tag, &raw);
            status_ptr_with_data(ptr, data, worker)
        }
    }

    /// Synchronous-mode tag send, taking ownership of `data`: the request
    /// completes only after the receiver has matched the message.
    ///
    /// Same ownership and `param` rules as [`Ep::tag_send`].
    pub fn tag_send_sync<'w, D: OwnedBuf>(
        &self,
        worker: &'w Worker,
        data: D,
        tag: u64,
        param: &RequestParam,
    ) -> Result<Completion<D, OwningRequest<'w, D>>, Status> {
        debug_assert!(
            std::sync::Arc::ptr_eq(&self.worker_alive, &worker.alive),
            "tag_send_sync: the passed worker is not the endpoint's worker"
        );
        let raw = owned_op_param(param);
        // SAFETY: as in `tag_send`.
        unsafe {
            let ptr = ucp_tag_send_sync_nbx(self.handle, data.as_ptr() as _, data.len(), tag, &raw);
            status_ptr_with_data(ptr, data, worker)
        }
    }

    /// Send `len` bytes from an arbitrary address, including device (GPU) memory.
    ///
    /// The slice-based [`Ep::tag_send`] cannot express accelerator buffers: a device pointer
    /// must never be turned into a host `&[u8]`, because the host cannot dereference it. This
    /// variant takes the address and length separately so UCX can pick a CUDA-aware transport
    /// (`cuda_copy`, `cuda_ipc`, `gdr_copy`). Pair it with
    /// `RequestParamBuilder::memory_type(MemoryType::Cuda)` so UCX does not probe the buffer.
    ///
    /// # Safety
    ///
    /// - `ptr` must be valid for reads of `len` bytes and readable by UCX for the whole
    ///   operation. For device memory it must be a device allocation on the current device,
    ///   or managed memory.
    /// - The memory must stay alive and unpublished (no host reallocation, no `free`) until the
    ///   returned request completes; with a blocking wait that means until the wait returns.
    /// - `len` must not exceed the underlying allocation.
    pub unsafe fn tag_send_ptr(
        &self,
        ptr: *const u8,
        len: usize,
        tag: u64,
        param: &RequestParam,
    ) -> Result<Pending, Status> {
        status_ptr_to_result(unsafe {
            ucp_tag_send_nbx(self.handle, ptr as _, len, tag, &param.handle)
        })
    }
}

#[derive(Debug)]
pub struct MessageHandle {
    pub(crate) handle: ucp_tag_message_h,
    pub(crate) info: TagInfo,
    removed: bool,
}

impl MessageHandle {
    pub fn len(&self) -> usize {
        self.info.len()
    }

    pub fn is_empty(&self) -> bool {
        self.info.is_empty()
    }

    pub fn sender_tag(&self) -> u64 {
        self.info.sender_tag()
    }
}

/// Receive metadata for a completed tag receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagInfo {
    length: usize,
    sender_tag: u64,
}

impl sealed::Sealed for TagInfo {}

impl RecvInfo for TagInfo {
    fn read(request: &Request) -> Self {
        let mut info = std::mem::MaybeUninit::<ucp_tag_recv_info_t>::uninit();
        // SAFETY: `request` is a live, completed tag receive request (the
        // caller reads info before freeing it); UCX fills `info` on UCS_OK.
        let status = unsafe { ucp_tag_recv_request_test(request.as_ptr(), info.as_mut_ptr()) };
        match map_tag_recv_test_status(status, info) {
            Ok(Some(info)) => info,
            other => unreachable!("completed tag receive reported {other:?}"),
        }
    }
}

impl TagInfo {
    /// Construct a `TagInfo` from the raw FFI struct.
    pub(crate) fn from_ffi(ffi: ucp_tag_recv_info_t) -> Self {
        TagInfo {
            length: ffi.length,
            sender_tag: ffi.sender_tag,
        }
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub fn sender_tag(&self) -> u64 {
        self.sender_tag
    }
}

impl Worker {
    /// Post a tagged receive into `data`, taking ownership of it.
    ///
    /// Completes with the filled buffer and its [`TagInfo`] (sender tag and
    /// received length), either immediately or through
    /// [`TagRecvRequest::try_complete`] after the worker is progressed. A
    /// receive with no matching message stays pending until matched or
    /// canceled ([`TagRecvRequest::cancel`], which hands the buffer back as
    /// [`crate::Failed`] with `UCS_ERR_CANCELED`).
    ///
    /// `param` contributes flags only (e.g. `NO_IMM_CMPL`); a callback or user
    /// data set on it is ignored, because the receive member of UCX's callback
    /// union has a different signature than `RequestParamBuilder::send_callback`.
    pub fn tag_recv<D: OwnedBufMut>(
        &self,
        mut data: D,
        tag: u64,
        mask: u64,
        param: &RequestParam,
    ) -> Result<Completion<(D, TagInfo), TagRecvRequest<'_, D>>, Status> {
        let mut info = std::mem::MaybeUninit::<ucp_tag_recv_info_t>::zeroed();
        let mut raw = owned_recv_param(param);
        // UCX fills `info` only if the receive completes inside this call.
        raw.op_attr_mask |= ucp_op_attr_t::UCP_OP_ATTR_FIELD_RECV_INFO as u32;
        raw.recv_info.tag_info = info.as_mut_ptr();
        // SAFETY: the request (if any) writes only into `data`'s stable
        // storage, which the returned value owns until UCX is done; `info` is
        // written synchronously and is not retained.
        let completion = unsafe {
            let ptr = ucp_tag_recv_nbx(
                self.handle,
                data.as_mut_ptr() as _,
                data.len(),
                tag,
                mask,
                &raw,
            );
            status_ptr_with_data(ptr, data, self)?
        };
        Ok(match completion {
            Completion::Immediate(data) => {
                // SAFETY: zero-initialized, and filled by UCX on immediate completion.
                Completion::Immediate((data, TagInfo::from_ffi(unsafe { info.assume_init() })))
            }
            Completion::Pending(request) => Completion::Pending(TagRecvRequest::new(request)),
        })
    }

    /// Receive into an arbitrary address, including device (GPU) memory.
    ///
    /// Counterpart of [`Ep::tag_send_ptr`]. Note the asymmetry in UCX itself: tag *send* is an
    /// endpoint operation while tag *receive* is a worker operation, so the two pointer variants
    /// live on different types.
    ///
    /// # Safety
    ///
    /// - `ptr` must be valid for writes of `len` bytes and writable by UCX for the whole
    ///   operation (device allocation on the current device, or managed memory).
    /// - The memory must stay alive and unpublished until the returned request completes.
    /// - `len` must not exceed the underlying allocation.
    pub unsafe fn tag_recv_ptr(
        &self,
        ptr: *mut u8,
        len: usize,
        tag: u64,
        mask: u64,
        param: &RequestParam,
    ) -> Result<Pending, Status> {
        status_ptr_to_result(unsafe {
            ucp_tag_recv_nbx(self.handle, ptr as _, len, tag, mask, &param.handle)
        })
    }

    pub fn tag_probe(&self, tag: u64, tag_mask: u64, remove: bool) -> Option<MessageHandle> {
        let mut info = std::mem::MaybeUninit::<ucp_tag_recv_info_t>::uninit();
        let handle = unsafe {
            ucp_tag_probe_nb(self.handle, tag, tag_mask, remove as i32, info.as_mut_ptr())
        };

        if !handle.is_null() {
            Some(MessageHandle {
                handle,
                info: TagInfo::from_ffi(unsafe { info.assume_init() }),
                removed: remove,
            })
        } else {
            None
        }
    }

    /// Receive a message previously removed with [`Worker::tag_probe`]
    /// (`remove = true`), taking ownership of `data`.
    ///
    /// Consumes the handle: UCX allows each probed message to be received
    /// once. Completes with the filled buffer and the message's [`TagInfo`].
    /// `param` is handled as in [`Worker::tag_recv`] (flags only).
    ///
    /// The error hands the message back **only** when UCX provably did not
    /// consume it: `UCS_ERR_NO_RESOURCE` with `FORCE_IMM_CMPL` (UCX checks the
    /// flag before matching a non-eager-only message, `tag_recv.c`). Every
    /// other error may come after UCX released the descriptor (rendezvous
    /// `CANCELED`/`NO_MEMORY`, eager `MESSAGE_TRUNCATED`), so the message is
    /// not returned: a retry would be a use-after-free. In the rare pre-match
    /// failures (`NO_MEMORY` request allocation, a worker without the tag
    /// feature) the payload then stays with the worker until it is destroyed,
    /// which is a leak, not unsoundness.
    ///
    /// # Panics
    ///
    /// If `message` was probed without removing it from the unexpected queue.
    pub fn tag_msg_recv<D: OwnedBufMut>(
        &self,
        mut data: D,
        message: MessageHandle,
        param: &RequestParam,
    ) -> Result<Completion<(D, TagInfo), TagRecvRequest<'_, D>>, TagMsgRecvError> {
        if !message.removed {
            panic!("Tried to call tag_msg_recv() on a MessageHandle that didn't remove the entry!");
        }
        let info = message.info;
        let raw = owned_recv_param(param);
        let forced = raw.op_attr_mask & ucp_op_attr_t::UCP_OP_ATTR_FLAG_FORCE_IMM_CMPL as u32 != 0;
        // SAFETY: as in `tag_recv`; `message` is a removed, not yet received
        // message handle.
        let completion = unsafe {
            let ptr = ucp_tag_msg_recv_nbx(
                self.handle,
                data.as_mut_ptr() as _,
                data.len(),
                message.handle,
                &raw,
            );
            status_ptr_with_data(ptr, data, self)
        };
        match completion {
            // The immediate arm reports the probe-time info; the pending arm
            // re-reads it from the request. They agree on UCS_OK, but keep the
            // pending path reading the request (it is authoritative).
            Ok(Completion::Immediate(data)) => Ok(Completion::Immediate((data, info))),
            Ok(Completion::Pending(request)) => {
                Ok(Completion::Pending(TagRecvRequest::new(request)))
            }
            Err(status) if msg_recv_error_returns_message(forced, status) => Err(TagMsgRecvError {
                status,
                message: Some(message),
            }),
            Err(status) => Err(TagMsgRecvError {
                status,
                message: None,
            }),
        }
    }
}

/// Whether a `tag_msg_recv` posting error proves UCX did not consume the
/// probed descriptor (see [`Worker::tag_msg_recv`]).
fn msg_recv_error_returns_message(forced: bool, status: Status) -> bool {
    forced && status == Status::NO_RESOURCE
}

/// Error from [`Worker::tag_msg_recv`].
#[derive(Debug)]
pub struct TagMsgRecvError {
    /// UCX status.
    pub status: Status,
    /// The probed message, only when UCX provably did not consume it
    /// (`NO_RESOURCE` under `FORCE_IMM_CMPL`); `None` otherwise.
    pub message: Option<MessageHandle>,
}

impl std::fmt::Display for TagMsgRecvError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "tag_msg_recv failed: {}", self.status)
    }
}

impl std::error::Error for TagMsgRecvError {}

fn map_tag_recv_test_status(
    status: ucs_status_t,
    info: std::mem::MaybeUninit<ucp_tag_recv_info_t>,
) -> Result<Option<TagInfo>, Status> {
    match status {
        ucs_status_t::UCS_OK => Ok(Some(TagInfo::from_ffi(unsafe { info.assume_init() }))),
        ucs_status_t::UCS_INPROGRESS => Ok(None),
        _ => Err(crate::Status::from(status)),
    }
}

impl Request {
    /// Test a tagged receive request and return its receive information when complete.
    ///
    /// `Ok(Some(info))` means the receive completed and `info` contains the sender tag
    /// and length reported by UCX. `Ok(None)` means the request is still in progress.
    /// Receive information is only available while the request remains allocated.
    ///
    /// Callers should not use this method after the request has completed and the
    /// request has been freed.
    pub fn tag_recv_test(&mut self) -> Result<Option<TagInfo>, Status> {
        let h = self.handle;
        let mut info = std::mem::MaybeUninit::<ucp_tag_recv_info_t>::uninit();
        let status = unsafe { ucp_tag_recv_request_test(h.as_ptr(), info.as_mut_ptr()) };
        map_tag_recv_test_status(status, info)
    }
}

/// Tag send with synchronous completion.
///
/// # Safety
/// Caller must ensure `buffer` is valid for `count` bytes.
#[deprecated(since = "0.1.0", note = "Use Ep::tag_send_sync() instead")]
pub unsafe fn tag_send_sync_nbx(
    ep: &crate::ep::Ep,
    buffer: *const std::os::raw::c_void,
    count: usize,
    tag: u64,
) -> crate::Request {
    let ptr = ucp_tag_send_sync_nbx(ep.handle, buffer, count, tag, std::ptr::null());
    crate::Request::from_raw(ptr)
}

/// Legacy tag message receive (non-nbx variant).
///
/// # Safety
/// Caller must ensure `buffer` has space for `count` elements of `datatype`.
pub unsafe fn tag_msg_recv_nb(
    worker: &crate::worker::Worker,
    buffer: *mut std::os::raw::c_void,
    count: usize,
    datatype: u64,
    message: &MessageHandle,
) -> crate::Request {
    let ptr = ucp_tag_msg_recv_nb(
        worker.handle,
        buffer,
        count,
        datatype as ucp_datatype_t,
        message.handle,
        None,
    );
    crate::Request::from_raw(ptr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context;
    use crate::context::Context;
    use crate::ep;
    use crate::tests::setup_default;
    use crate::tests::CommsContext;
    use crate::worker;
    use crate::worker::RemoteWorkerAddress;
    use crate::RequestParamBuilder;
    use std::rc::Rc;

    const TAG_FULL: u64 = u64::MAX;

    /// Test-only driver: progress until an owned completion resolves.
    fn drive<D, R, F>(worker: &worker::Worker, completion: Completion<D, R>, mut poll: F) -> D
    where
        F: FnMut(R) -> Result<D, R>,
    {
        let mut request = match completion {
            Completion::Immediate(data) => return data,
            Completion::Pending(request) => request,
        };
        for _ in 0..1_000_000 {
            worker.progress();
            match poll(request) {
                Ok(data) => return data,
                Err(still) => request = still,
            }
        }
        panic!("request did not complete");
    }

    fn send_done<D>(r: OwningRequest<'_, D>) -> Result<D, OwningRequest<'_, D>> {
        r.try_complete()
            .map(|result| result.unwrap_or_else(|failed| panic!("send: {failed}")))
    }

    fn recv_done<D>(r: TagRecvRequest<'_, D>) -> Result<(D, TagInfo), TagRecvRequest<'_, D>> {
        r.try_complete()
            .map(|result| result.unwrap_or_else(|failed| panic!("recv: {failed}")))
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn tag_owned_api_signatures() {
        let _: for<'w> fn(
            &Ep,
            &'w worker::Worker,
            Vec<u8>,
            u64,
            &RequestParam,
        )
            -> Result<Completion<Vec<u8>, OwningRequest<'w, Vec<u8>>>, Status> = Ep::tag_send_sync;
        let _: for<'w> fn(
            &'w worker::Worker,
            Vec<u8>,
            u64,
            u64,
            &RequestParam,
        ) -> Result<
            Completion<(Vec<u8>, TagInfo), TagRecvRequest<'w, Vec<u8>>>,
            Status,
        > = worker::Worker::tag_recv;
    }

    #[test]
    fn tag_owned_immediate_recv_reports_info() {
        let comms = setup_default();
        let param = RequestParamBuilder::new().build();
        let sent = comms
            .ep
            .tag_send(&comms.worker, b"hey".to_vec(), 0x77, &param)
            .expect("send");
        assert_eq!(drive(&comms.worker, sent, send_done), b"hey");
        // The message is now unexpected, so the receive matches inside the
        // call and UCX fills the receive info supplied by `tag_recv`.
        let received = comms
            .worker
            .tag_recv(vec![0u8; 8], 0x77, u64::MAX, &param)
            .expect("recv");
        assert!(
            received.is_immediate(),
            "unexpected eager message must match inline"
        );
        let (buffer, info) = received.into_immediate().expect("completed");
        assert_eq!(&buffer[..3], b"hey");
        assert_eq!((info.len(), info.sender_tag()), (3, 0x77));
    }

    #[test]
    fn tag_owned_pending_round_trip_returns_buffers_and_info() {
        let comms = setup_default();
        let param = RequestParamBuilder::new().no_imm_cmpl().build();
        let len = 8 << 20; // rendezvous: completes in progress, not inline
        let recv = comms
            .worker
            .tag_recv(vec![0u8; len], 0x99, u64::MAX, &param)
            .expect("recv");
        assert!(recv.is_pending());
        let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let send = comms
            .ep
            .tag_send(&comms.worker, payload.clone(), 0x99, &param)
            .expect("send");
        assert!(send.is_pending());
        let sent = drive(&comms.worker, send, send_done);
        let (received, info) = drive(&comms.worker, recv, recv_done);
        assert!(sent == payload && received == payload);
        assert_eq!((info.len(), info.sender_tag()), (len, 0x99));
    }

    #[test]
    fn tag_owned_recv_cancel_returns_buffer() {
        let comms = setup_default();
        let param = RequestParamBuilder::new().build();
        let mut recv = comms
            .worker
            .tag_recv(vec![1u8; 16], 0xC0FFEE, u64::MAX, &param)
            .expect("recv")
            .into_pending()
            .expect("no message: pending");
        recv.cancel();
        assert!(recv.is_canceled());
        let failed = drive(
            &comms.worker,
            Completion::Pending(recv),
            |r: TagRecvRequest<'_, Vec<u8>>| {
                r.try_complete()
                    .map(|result| result.expect_err("must be canceled"))
            },
        );
        assert!(failed.is_canceled());
        assert_eq!(failed.data, vec![1u8; 16]);
    }

    #[test]
    fn tag_owned_send_sync_completes_after_match() {
        let comms = setup_default();
        let param = RequestParamBuilder::new().build();
        let recv = comms
            .worker
            .tag_recv(vec![0u8; 4], 0x55, u64::MAX, &param)
            .expect("recv");
        let send = comms
            .ep
            .tag_send_sync(&comms.worker, vec![4u8; 4], 0x55, &param)
            .expect("send_sync");
        assert_eq!(drive(&comms.worker, send, send_done), vec![4u8; 4]);
        let (received, info) = drive(&comms.worker, recv, recv_done);
        assert_eq!((received, info.len()), (vec![4u8; 4], 4));
    }

    #[test]
    fn tag_recv_test_maps_completed_status_to_info() {
        let info = ucp_tag_recv_info_t {
            sender_tag: 0xfeed,
            length: 37,
        };

        let result =
            map_tag_recv_test_status(ucs_status_t::UCS_OK, std::mem::MaybeUninit::new(info));

        let info = result
            .expect("completed status should succeed")
            .expect("completed status should include info");
        assert_eq!(info.sender_tag(), 0xfeed);
        assert_eq!(info.len(), 37);
    }

    #[test]
    fn tag_recv_test_maps_inprogress_status_to_none() {
        let result = map_tag_recv_test_status(
            ucs_status_t::UCS_INPROGRESS,
            std::mem::MaybeUninit::uninit(),
        );

        assert!(result.expect("in-progress status should succeed").is_none());
    }

    #[test]
    fn tag_recv_test_maps_error_status_to_error() {
        let result = map_tag_recv_test_status(
            ucs_status_t::UCS_ERR_IO_ERROR,
            std::mem::MaybeUninit::uninit(),
        );

        assert!(matches!(
            result,
            Err(Status(ucs_status_t::UCS_ERR_IO_ERROR))
        ));
    }

    static SEND_CB_CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    unsafe extern "C" fn counting_send_cb(
        _request: *mut std::os::raw::c_void,
        _status: Status,
        _user_data: *mut std::os::raw::c_void,
    ) {
        SEND_CB_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// A send-typed callback on a receive param must not reach UCX: the
    /// receive member of the callback union has a different signature.
    #[test]
    fn tag_recv_ignores_send_callback() {
        let comms = setup_default();
        let recv_param = RequestParamBuilder::new()
            .no_imm_cmpl()
            .send_callback(Some(counting_send_cb))
            .build();
        let recv = comms
            .worker
            .tag_recv(vec![0u8; 8 << 20], 0x5151, u64::MAX, &recv_param)
            .expect("recv");
        let send_param = RequestParamBuilder::new().build();
        let send = comms
            .ep
            .tag_send(&comms.worker, vec![9u8; 8 << 20], 0x5151, &send_param)
            .expect("send");
        let (received, info) = drive(&comms.worker, recv, recv_done);
        drive(&comms.worker, send, send_done);
        assert_eq!((info.len(), received[0]), (8 << 20, 9));
        assert_eq!(SEND_CB_CALLS.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// Context + worker for the close tests (tag feature only), pinned to the
    /// tcp transport: on `self`/`shm` a flush-close completes inline even with
    /// sends in flight and peer error handling is unavailable, so neither the
    /// deferred-close nor the forced-close path would be exercised.
    fn close_fixture() -> (crate::context::Context, crate::worker::Worker) {
        let params = crate::context::ParamsBuilder::new()
            .features(crate::context::Flags::Tag)
            .mt_workers_shared(1)
            .build();
        let config = crate::context::Config::read("", "").expect("config");
        // SAFETY: live config handle; NUL-terminated name and value.
        let status = unsafe { ucp_config_modify(config.handle, c"TLS".as_ptr(), c"tcp".as_ptr()) };
        assert_eq!(status, ucs_status_t::UCS_OK, "pin TLS=tcp");
        let mut context = crate::context::Context::new(&config, &params).expect("context");
        let worker = context
            .worker_create(&crate::worker::ParamsBuilder::new().build())
            .expect("worker");
        (context, worker)
    }

    fn self_ep(
        worker: &crate::worker::Worker,
        mode: crate::ErrorHandlerMode,
    ) -> Result<crate::ep::Ep, Status> {
        let packed = worker.pack_address().expect("address");
        let remote = crate::worker::RemoteWorkerAddress::new(packed.to_vec());
        drop(packed);
        worker.create_ep(
            crate::ep::ParamsBuilder::new()
                .address(&remote)
                .err_mode(mode)
                .build(),
        )
    }

    /// #208 acceptance: in-flight sends, their receives, and a *pending*
    /// `close(Flush)` all complete through ONE shared progress loop. The close
    /// cannot finish until the sends drain, and the sends need the receives
    /// progressed, so a loop that only drove the close would never end.
    #[test]
    fn flush_close_and_ops_complete_in_one_progress_loop() {
        let (_context, worker) = close_fixture();
        let worker = &worker;
        let ep = self_ep(worker, crate::ErrorHandlerMode::None).expect("endpoint");
        let (n, len) = (4u64, 1 << 20); // rendezvous: needs the receiver
        let param = RequestParamBuilder::new().no_imm_cmpl().build();
        let mut sends: Vec<_> = (0..n)
            .map(|i| {
                ep.tag_send(worker, vec![i as u8; len], 0x7100 + i, &param)
                    .expect("post send")
                    .into_pending()
                    .expect("NO_IMM_CMPL send is pending")
            })
            .collect();
        let mut recvs: Vec<_> = (0..n)
            .map(|i| {
                worker
                    .tag_recv(vec![0u8; len], 0x7100 + i, u64::MAX, &param)
                    .expect("post recv")
                    .into_pending()
                    .expect("NO_IMM_CMPL recv is pending")
            })
            .collect();
        let mut close = match ep.close(worker, crate::ep::CloseMode::Flush) {
            Ok(Completion::Pending(request)) => Some(request),
            Ok(Completion::Immediate(())) => panic!("close with pending sends must defer"),
            Err(error) => panic!("{error}"),
        };
        let (mut sent, mut received, mut closed) = (0, 0, false);
        for _ in 0..1_000_000 {
            if sends.is_empty() && recvs.is_empty() && closed {
                break;
            }
            worker.progress();
            sends = sends
                .into_iter()
                .filter_map(|request| match request.try_complete() {
                    Ok(result) => {
                        let buffer = result.unwrap_or_else(|failed| panic!("send: {failed}"));
                        assert_eq!(buffer.len(), len);
                        sent += 1;
                        None
                    }
                    Err(pending) => Some(pending),
                })
                .collect();
            recvs = recvs
                .into_iter()
                .filter_map(|request| match request.try_complete() {
                    Ok(result) => {
                        let (buffer, info) =
                            result.unwrap_or_else(|failed| panic!("recv: {failed}"));
                        let i = (info.sender_tag() - 0x7100) as u8;
                        assert!(buffer.iter().all(|&byte| byte == i), "payload");
                        received += 1;
                        None
                    }
                    Err(pending) => Some(pending),
                })
                .collect();
            if let Some(request) = close.take() {
                match request.try_complete() {
                    Ok(result) => {
                        result.unwrap_or_else(|failed| panic!("close: {failed}"));
                        assert_eq!(sent, n, "close completed before the sends drained");
                        closed = true;
                    }
                    Err(pending) => close = Some(pending),
                }
            }
        }
        assert_eq!((sent, received), (n, n));
        assert!(closed, "close completed");
    }

    /// `close(Force)` cancels the endpoint's outstanding sends: each comes
    /// back `CANCELED` with its buffer. Needs peer error handling (skips on
    /// transports without it, e.g. `self`/`shm`).
    #[test]
    fn force_close_cancels_outstanding_sends() {
        let (_context, worker) = close_fixture();
        let worker = &worker;
        let ep = match self_ep(worker, crate::ErrorHandlerMode::Peer) {
            Ok(ep) => ep,
            Err(status) if status == Status::UNREACHABLE => {
                eprintln!("skipping: no transport with peer error handling ({status})");
                return;
            }
            Err(status) => panic!("endpoint: {status}"),
        };
        let (n, len) = (4u64, 1 << 20); // rendezvous: waits for a receive that never comes
        let param = RequestParamBuilder::new().no_imm_cmpl().build();
        let mut sends: Vec<_> = (0..n)
            .map(|i| {
                ep.tag_send(worker, vec![i as u8; len], 0x7000 + i, &param)
                    .expect("post send")
                    .into_pending()
                    .expect("NO_IMM_CMPL send is pending")
            })
            .collect();
        match ep.close(worker, crate::ep::CloseMode::Force) {
            // UCX discards the lanes inline for a forced close.
            Ok(Completion::Immediate(())) => {}
            Ok(Completion::Pending(_)) => panic!("forced close unexpectedly deferred"),
            Err(error) => panic!("{error}"),
        }
        let mut canceled = 0;
        for _ in 0..1_000_000 {
            if sends.is_empty() {
                break;
            }
            worker.progress();
            sends = sends
                .into_iter()
                .filter_map(|request| match request.try_complete() {
                    Ok(Err(crate::Failed { status, data })) => {
                        assert_eq!(status, Status::CANCELED);
                        assert_eq!(data.len(), len, "buffer handed back");
                        canceled += 1;
                        None
                    }
                    Ok(Ok(_)) => panic!("send without a receiver must not succeed"),
                    Err(pending) => Some(pending),
                })
                .collect();
        }
        assert_eq!(canceled, n, "every send canceled by close(Force)");
    }

    /// UCX refuses `close(Force)` on an endpoint without peer error handling
    /// before consuming it: the endpoint comes back intact and closes
    /// normally afterwards. Runs on every transport.
    #[test]
    fn force_close_without_peer_returns_endpoint() {
        let (_context, worker) = close_fixture();
        let ep = self_ep(&worker, crate::ErrorHandlerMode::None).expect("endpoint");
        let error = match ep.close(&worker, crate::ep::CloseMode::Force) {
            Err(error) => error,
            Ok(_) => panic!("UCX must refuse Force without peer error handling"),
        };
        assert_eq!(error.status, Status::INVALID_PARAM);
        let ep = error.ep.expect("endpoint handed back, not leaked");
        let close = ep
            .close(&worker, crate::ep::CloseMode::Flush)
            .expect("graceful close");
        match close {
            Completion::Immediate(()) => {}
            Completion::Pending(mut request) => {
                let mut done = false;
                for _ in 0..1_000_000 {
                    worker.progress();
                    match request.try_complete() {
                        Ok(result) => {
                            result.unwrap_or_else(|failed| panic!("close: {failed}"));
                            done = true;
                            break;
                        }
                        Err(still) => request = still,
                    }
                }
                assert!(done, "close did not complete");
            }
        };
    }

    /// An endpoint whose worker was destroyed must not accept a new worker
    /// that happens to reuse the old `ucp_worker_h` address.
    #[test]
    fn endpoint_rejects_worker_created_after_its_own_died() {
        let (mut context, worker) = close_fixture();
        let old_handle = worker.handle;
        let ep = self_ep(&worker, crate::ErrorHandlerMode::None).expect("endpoint");
        drop(worker);
        let fresh = context
            .worker_create(&crate::worker::ParamsBuilder::new().build())
            .expect("fresh worker");
        eprintln!("handle reused: {}", fresh.handle == old_handle);
        assert!(matches!(
            ep.flush(&fresh, &RequestParamBuilder::new().build()),
            Err(status) if status == Status::INVALID_PARAM
        ));
        let error = match ep.close(&fresh, crate::ep::CloseMode::Flush) {
            Err(error) => error,
            Ok(_) => panic!("close on a different worker must fail"),
        };
        assert!(error.ep.is_some(), "endpoint handed back");
        // Dropping it skips UCX: its worker is dead.
    }

    #[test]
    fn msg_recv_error_returns_message_only_when_provably_unconsumed() {
        assert!(msg_recv_error_returns_message(true, Status::NO_RESOURCE));
        for status in [
            Status::CANCELED,
            Status::NO_MEMORY,
            Status::MESSAGE_TRUNCATED,
            Status::INVALID_PARAM,
            Status::IO_ERROR,
        ] {
            assert!(!msg_recv_error_returns_message(true, status), "{status}");
            assert!(!msg_recv_error_returns_message(false, status), "{status}");
        }
        assert!(!msg_recv_error_returns_message(false, Status::NO_RESOURCE));
    }

    /// A receive UCX could not start hands the probed message back.
    #[test]
    fn tag_msg_recv_not_started_returns_message() {
        let comms = setup_default();
        let len = 8 << 20; // rendezvous: cannot complete inside the call
        let send = comms
            .ep
            .tag_send(
                &comms.worker,
                vec![6u8; len],
                0x6161,
                &RequestParamBuilder::new().build(),
            )
            .expect("send");
        let mut msg = comms.worker.tag_probe(0x6161, u64::MAX, true);
        for _ in 0..100_000 {
            if msg.is_some() {
                break;
            }
            comms.worker.progress();
            msg = comms.worker.tag_probe(0x6161, u64::MAX, true);
        }
        let forced = RequestParamBuilder::new().force_imm_cmpl().build();
        let err = comms
            .worker
            .tag_msg_recv(vec![0u8; len], msg.expect("probed"), &forced)
            .expect_err("rendezvous cannot complete immediately");
        assert_eq!(err.status, Status::NO_RESOURCE);
        let message = err.message.expect("not started: message handed back");
        let recv = comms
            .worker
            .tag_msg_recv(vec![0u8; len], message, &RequestParamBuilder::new().build())
            .expect("retry");
        let (received, info) = drive(&comms.worker, recv, recv_done);
        drive(&comms.worker, send, send_done);
        assert_eq!((info.len(), received[len - 1]), (len, 6));
    }

    /// Real request round trip; `TagInfo` comes from `ucp_tag_recv_request_test`
    /// via `TagRecvRequest::try_complete` (the `RecvInfo` impl).
    #[test]
    fn tag_recv_test_real_completed_request() {
        let comms = setup_default();
        let tag = 0x1234;
        let param = RequestParamBuilder::new().no_imm_cmpl().build();
        let recv = comms
            .worker
            .tag_recv(vec![0u8; 5], tag, u64::MAX, &param)
            .expect("post tag receive");
        let sent = comms
            .ep
            .tag_send(&comms.worker, b"hello".to_vec(), tag, &param)
            .expect("post tag send");
        let (buffer, info) = drive(&comms.worker, recv, recv_done);
        drive(&comms.worker, sent, send_done);
        assert_eq!(info.sender_tag(), tag);
        assert_eq!(info.len(), 5);
        assert_eq!(buffer, b"hello");
    }

    #[test]
    fn tag_send() {
        let comms = setup_default();
        let tag_flags = RequestParamBuilder::new().no_imm_cmpl().build();
        let send = comms
            .ep
            .tag_send(&comms.worker, vec![32u8], TAG_FULL, &tag_flags)
            .unwrap();
        let recv = comms
            .worker
            .tag_recv(vec![0u8], TAG_FULL, TAG_FULL, &tag_flags)
            .unwrap();
        let (received, _) = drive(&comms.worker, recv, recv_done);
        drive(&comms.worker, send, send_done);
        assert_eq!(received, vec![32u8]);
    }

    #[test]
    fn tag_probe() {
        let comms = setup_default();
        let tag_flags = RequestParamBuilder::new().no_imm_cmpl().build();
        let send = comms
            .ep
            .tag_send(&comms.worker, vec![32u8], TAG_FULL, &tag_flags)
            .unwrap();
        let mut msg = comms.worker.tag_probe(TAG_FULL, TAG_FULL, true);
        while msg.is_none() {
            comms.worker.progress();
            msg = comms.worker.tag_probe(TAG_FULL, TAG_FULL, true);
        }
        let recv = comms
            .worker
            .tag_msg_recv(vec![0u8], msg.unwrap(), &tag_flags)
            .unwrap();
        let (received, info) = drive(&comms.worker, recv, recv_done);
        drive(&comms.worker, send, send_done);
        assert_eq!((received, info.len()), (vec![32u8], 1));
    }
}
