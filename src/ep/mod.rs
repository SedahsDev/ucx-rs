//! # Teardown
//!
//! [`Ep::close`] is the teardown path: it returns an owned close completion
//! ([`OpCompletion`]) driven by the same shared progress loop as data
//! operations; [`CloseMode::Force`] also cancels the endpoint's outstanding
//! operations. [`Drop`] performs a best-effort graceful close when the worker
//! is still alive. [`Ep::destroy`] is the error-path escape hatch; use it only
//! for broken endpoints and never while operations are pending on them.

pub mod attr;
pub mod modify;
pub mod params;
pub mod perf;

pub use attr::*;
pub use modify::*;
pub use params::*;
pub use perf::*;

/// Native alias for the endpoint error-handler callback. The error status arrives as
/// [`crate::Status`], which is `#[repr(transparent)]` over the raw status, so this matches
/// UCX's C callback type. (The raw `ep` handle parameter is still the C type.)
pub type ErrHandlerCb = Option<
    unsafe extern "C" fn(arg: *mut std::os::raw::c_void, ep: ucp_ep_h, status: crate::Status),
>;

use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::RemoteWorkerAddress;
use crate::worker::Worker;
use crate::Pending;
use crate::Status;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// UCX endpoint ownership wrapper.
///
/// This type is intentionally not `Clone`: cloning would create two owners of
/// one endpoint and cause a double close/use-after-free.
#[derive(Debug)]
pub struct Ep {
    pub(crate) handle: ucp_ep_h,
    pub(crate) worker_alive: Arc<AtomicBool>,
    /// The worker this endpoint was created on. Completion-returning calls
    /// check it so their requests are progressed on the right worker.
    pub(crate) worker: ucp_worker_h,
}

/// How [`Ep::close`] tears down the endpoint (`UCP_EP_CLOSE_FLAG_FORCE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloseMode {
    /// Flush outstanding operations, then disconnect (graceful).
    #[default]
    Flush,
    /// Disconnect without flushing. Outstanding operations on the endpoint
    /// complete with an error status (typically `UCS_ERR_CANCELED`).
    ///
    /// UCX accepts this only for endpoints created with
    /// [`crate::ErrorHandlerMode::Peer`], which needs a transport with peer
    /// failure support (tcp, IB/RoCE; not `self` or `shm`, where creating such
    /// an endpoint fails with `UCS_ERR_UNREACHABLE`). Otherwise `close` fails with
    /// `UCS_ERR_INVALID_PARAM` (and UCX has consumed nothing: the endpoint is
    /// handed back in [`CloseError::ep`]).
    Force,
}

impl CloseMode {
    fn flags(self) -> u32 {
        match self {
            CloseMode::Flush => 0,
            CloseMode::Force => ucp_ep_close_flags_t::UCP_EP_CLOSE_FLAG_FORCE as u32,
        }
    }
}

/// Completion of an operation that carries no data (flush, close, modify).
pub type OpCompletion<'w> = crate::Completion<(), crate::OwningRequest<'w, ()>>;

/// Native Rust wrapper for the raw `ucp_ep_h` handle.
#[derive(Debug, Clone, Copy)]
pub struct EpHandle(pub(crate) ucp_ep_h);

impl From<ucp_ep_h> for EpHandle {
    fn from(handle: ucp_ep_h) -> Self {
        EpHandle(handle)
    }
}

impl From<EpHandle> for ucp_ep_h {
    fn from(handle: EpHandle) -> Self {
        handle.0
    }
}

impl Ep {
    /// `INVALID_PARAM` unless `worker` is the worker this endpoint belongs to.
    fn check_worker(&self, worker: &Worker) -> Result<(), Status> {
        if worker.handle == self.worker {
            Ok(())
        } else {
            Err(Status(ucs_status_t::UCS_ERR_INVALID_PARAM))
        }
    }

    /// Expose the raw UCP endpoint handle for FFI callers.
    pub fn handle(&self) -> EpHandle {
        EpHandle(self.handle)
    }

    /// Print endpoint diagnostics to `fd`. Invalid descriptors are ignored.
    pub fn print_info(&self, fd: std::os::fd::RawFd) {
        let _ = crate::config::with_file_stream(fd, |stream| {
            // SAFETY: self owns a live endpoint and stream is valid for this call.
            unsafe { ucp_ep_print_info(self.handle, stream.cast()) };
        });
    }

    pub fn new(ep_params: Params<'_>, worker: &Worker) -> Result<Ep, Status> {
        let mut ep: ucp_ep_h = std::ptr::null_mut();
        let result =
            status_to_result(unsafe { ucp_ep_create(worker.handle, &ep_params.handle, &mut ep) });
        match result {
            Ok(()) => Ok(Ep {
                handle: ep,
                worker_alive: Arc::clone(&worker.alive),
                worker: worker.handle,
            }),
            Err(ucs_status_t) => Err(ucs_status_t),
        }
    }

    /// Flush all outstanding AMO and RMA operations on this endpoint.
    ///
    /// Completion guarantees that operations issued before the flush have
    /// completed at both the origin and target. This wrapper preserves
    /// immediate completion and UCX error statuses. UCX may make progress while
    /// servicing this call: concurrent calls are safe but contended for a
    /// `ThreadMode::Multi` worker in an MT-enabled UCX build, but are
    /// undefined behavior in a non-MT build or `ThreadMode::Single`. See
    /// `THREADING.md` §2.1. Prefer one thread to own each worker's progress
    /// loop, using `Worker::arm()` and `Worker::get_efd()` for wakeups.
    ///
    /// `worker` must be the endpoint's worker (else `UCS_ERR_INVALID_PARAM`).
    /// A deferred flush is an [`crate::OwningRequest`]: drive it with the same
    /// shared `worker.progress()` + `try_complete` loop as data operations.
    /// `params` contributes flags, callback and user data; request allocation
    /// is controlled here.
    pub fn flush<'w>(
        &self,
        worker: &'w Worker,
        params: &crate::RequestParam,
    ) -> Result<OpCompletion<'w>, Status> {
        self.check_worker(worker)?;
        let raw = crate::completion::owned_op_param(params);
        // SAFETY: live endpoint on `worker`; the request carries no buffer.
        unsafe {
            crate::completion::status_ptr_with_data(ucp_ep_flush_nbx(self.handle, &raw), (), worker)
        }
    }

    /// Immediately destroy a broken or erroring endpoint.
    ///
    /// Do not call this while operations are pending on the endpoint. This
    /// consumes the endpoint, so it cannot be used after destruction. If the
    /// worker has already been destroyed, the UCX call is skipped.
    pub fn destroy(self) {
        let this = std::mem::ManuallyDrop::new(self);
        if this.worker_alive.load(std::sync::atomic::Ordering::Acquire) {
            // SAFETY: the endpoint and its worker are alive and owned by this value.
            unsafe { ucp_ep_destroy(this.handle) };
        }
    }

    /// Modify endpoint error handling and user data.
    ///
    /// `ucp_ep_modify_nb` is an upstream deprecated compatibility API declared
    /// in `ucp_compat.h`.
    ///
    /// `worker` must be the endpoint's worker (else `UCS_ERR_INVALID_PARAM`).
    pub fn modify<'w>(
        &self,
        worker: &'w Worker,
        params: &ModifyParams,
    ) -> Result<OpCompletion<'w>, Status> {
        self.check_worker(worker)?;
        // SAFETY: self.handle is a live endpoint and params owns initialized storage.
        unsafe {
            crate::completion::status_ptr_with_data(
                ucp_ep_modify_nb(self.handle, &params.handle),
                (),
                worker,
            )
        }
    }

    /// Estimate the time needed to send a message of `message_size` bytes.
    pub fn evaluate_perf(&self, message_size: usize) -> Result<EpPerf, Status> {
        // SAFETY: UCX fills the initialized attribute structure according to its mask.
        let mut attr: ucp_ep_evaluate_perf_attr_t = unsafe { std::mem::zeroed() };
        attr.field_mask = ucp_ep_perf_attr_field::UCP_EP_PERF_ATTR_FIELD_ESTIMATED_TIME as u64;
        // SAFETY: UCX performance parameter structs are valid when zeroed.
        let mut param: ucp_ep_evaluate_perf_param_t = unsafe { std::mem::zeroed() };
        param.field_mask = ucp_ep_perf_param_field::UCP_EP_PERF_PARAM_FIELD_MESSAGE_SIZE as u64;
        param.message_size = message_size;
        // SAFETY: self.handle is live and both parameter structures are initialized.
        status_to_result(unsafe { ucp_ep_evaluate_perf(self.handle, &param, &mut attr) }).map(
            |()| EpPerf {
                estimated_time: attr.estimated_time,
            },
        )
    }

    /// Query endpoint attributes.
    ///
    /// Field masks:
    /// - UCP_EP_ATTR_FIELD_NAME = 1
    /// - UCP_EP_ATTR_FIELD_LOCAL_SOCKADDR = 2
    /// - UCP_EP_ATTR_FIELD_REMOTE_SOCKADDR = 4
    /// - UCP_EP_ATTR_FIELD_TRANSPORTS = 8
    /// - UCP_EP_ATTR_FIELD_USER_DATA = 16
    pub fn query(&self, mask: EpAttrFields) -> Result<EpAttr, Status> {
        // SAFETY: UCX fills the initialized attribute structure according to its mask.
        let mut attr: ucp_ep_attr = unsafe { std::mem::zeroed() };
        attr.field_mask = mask.bits();
        crate::status_to_result(unsafe { ucp_ep_query(self.handle, &mut attr) }).map(|()| {
            let name = if mask.contains(EpAttrFields::NAME) {
                // SAFETY: UCX documents NAME as a NUL-terminated fixed-size array.
                unsafe {
                    std::ffi::CStr::from_ptr(attr.name.as_ptr())
                        .to_string_lossy()
                        .into_owned()
                }
            } else {
                String::new()
            };
            EpAttr {
                name,
                local_sockaddr: mask
                    .contains(EpAttrFields::LOCAL_SOCKADDR)
                    .then_some(SockAddrStorage::from(attr.local_sockaddr)),
                remote_sockaddr: mask
                    .contains(EpAttrFields::REMOTE_SOCKADDR)
                    .then_some(SockAddrStorage::from(attr.remote_sockaddr)),
                transports: mask
                    .contains(EpAttrFields::TRANSPORTS)
                    .then_some(Transports::from(attr.transports)),
                user_data: mask
                    .contains(EpAttrFields::USER_DATA)
                    .then_some(attr.user_data),
            }
        })
    }

    /// Start closing this endpoint.
    ///
    /// The close is a data-less owned operation: `Immediate(())` when UCX
    /// finished inline, otherwise an [`crate::OwningRequest`] driven by the
    /// same shared progress loop as everything else. `worker` must be the
    /// endpoint's worker; on a mismatch the endpoint is handed back in
    /// [`CloseError::ep`] with `UCS_ERR_INVALID_PARAM`.
    ///
    /// # Disconnect recipe
    ///
    /// 1. Either [`cancel`](crate::OwningRequest::cancel) the endpoint's
    ///    outstanding requests, or close with [`CloseMode::Force`], which
    ///    completes them with an error status (typically `UCS_ERR_CANCELED`).
    /// 2. Push the close request into the same pending set as those requests.
    /// 3. Progress the worker and `try_complete` everything until the set is
    ///    empty. Buffers of canceled operations come back in
    ///    [`crate::Failed`].
    ///
    /// ```no_run
    /// # use ucx_sys::{ep::{CloseMode, Ep}, worker::Worker, Completion};
    /// # fn demo(worker: &Worker, ep: Ep) {
    /// match ep.close(worker, CloseMode::Force) {
    ///     Ok(Completion::Immediate(())) => {}
    ///     Ok(Completion::Pending(mut close)) => loop {
    ///         worker.progress();
    ///         match close.try_complete() {
    ///             Ok(_status) => break,
    ///             Err(still) => close = still,
    ///         }
    ///     },
    ///     Err(error) => eprintln!("{error}"),
    /// }
    /// # }
    /// ```
    pub fn close(self, worker: &Worker, mode: CloseMode) -> Result<OpCompletion<'_>, CloseError> {
        if let Err(status) = self.check_worker(worker) {
            return Err(CloseError {
                status,
                ep: Some(self),
            });
        }
        let this = std::mem::ManuallyDrop::new(self);
        // SAFETY: UCX request parameter structs are valid when zeroed.
        let mut param: ucp_request_param_t = unsafe { std::mem::zeroed() };
        param.op_attr_mask = ucp_op_attr_t::UCP_OP_ATTR_FIELD_FLAGS as u32;
        param.flags = mode.flags();
        // SAFETY: this.handle is owned by the endpoint; UCX takes ownership of
        // it whatever the result. The request carries no buffer.
        unsafe {
            crate::completion::status_ptr_with_data(
                ucp_ep_close_nbx(this.handle, &param),
                (),
                worker,
            )
        }
        // UCX consumed the endpoint even on error: nothing to hand back.
        .map_err(|status| CloseError { status, ep: None })
    }
}

/// Error from [`Ep::close`].
#[derive(Debug)]
pub struct CloseError {
    /// Why the close did not start.
    pub status: Status,
    /// The endpoint, when it was not consumed (wrong worker passed). `None`
    /// when UCX already consumed it.
    pub ep: Option<Ep>,
}

impl std::fmt::Display for CloseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "endpoint close failed: {}", self.status)
    }
}

impl std::error::Error for CloseError {}

impl Drop for Ep {
    fn drop(&mut self) {
        if !self.worker_alive.load(std::sync::atomic::Ordering::Acquire) {
            eprintln!("ucx-rs: endpoint dropped after its worker; skipping close");
            return;
        }
        // SAFETY: UCX request parameter structs are valid when zeroed.
        let param: ucp_request_param_t = unsafe { std::mem::zeroed() };
        // Close returns a null status pointer if complete, a request if in progress.
        // Request::Drop frees the request; do not free manually (would double-free).
        // SAFETY: self.handle is owned by this endpoint and param is initialized.
        let status = unsafe { ucp_ep_close_nbx(self.handle, &param) };
        match status_ptr_to_result(status) {
            Ok(_req) if !status.is_null() => {
                eprintln!("ucx-rs: endpoint close is still in progress during Drop; call Ep::close(&worker, flags) first");
                // Do not free a request whose close has not completed.
                std::mem::forget(_req);
            }
            Ok(_) => {}
            Err(error) => eprintln!("ucx-sys: endpoint close during Drop failed: {error:?}; UCX released endpoint resources"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_flush_api_signature() {
        let _flush: for<'w> fn(
            &Ep,
            &'w Worker,
            &crate::RequestParam,
        ) -> Result<OpCompletion<'w>, Status> = Ep::flush;
    }

    #[test]
    fn endpoint_issue_37_api_signatures() {
        let _: for<'w> fn(&Ep, &'w Worker, &ModifyParams) -> Result<OpCompletion<'w>, Status> =
            Ep::modify;
        let _: fn(&Ep, usize) -> Result<EpPerf, Status> = Ep::evaluate_perf;
    }

    #[test]
    fn modify_params_only_sets_supported_fields() {
        let mut builder = ModifyParamsBuilder::new();
        let params = builder.user_data(std::ptr::null_mut()).build();
        assert_eq!(
            params.handle.field_mask,
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_USER_DATA as u64
        );
    }

    #[test]
    fn modify_params_sets_error_handler_argument() {
        let argument = 42usize as *mut std::ffi::c_void;
        let mut builder = ModifyParamsBuilder::new();
        let params = builder.err_handler_arg(argument).build();
        assert_eq!(params.handle.err_handler.arg, argument);
    }

    #[test]
    fn params_builder_sets_error_handling_mode() {
        let mut builder = ParamsBuilder::new();
        let params = builder.err_mode(crate::ErrorHandlerMode::Peer).build();

        assert_ne!(
            params.handle.field_mask
                & ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLING_MODE as u64,
            0
        );
        assert_eq!(
            params.handle.err_mode,
            ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_PEER
        );
    }

    #[test]
    fn params_builder_sets_error_callback_fields() {
        unsafe extern "C" fn callback(_arg: *mut std::ffi::c_void, _ep: ucp_ep_h, _status: Status) {
        }
        let mut builder = ParamsBuilder::new();
        let params = builder
            .err_handler(Some(callback))
            .user_data(std::ptr::null_mut())
            .build();
        assert_ne!(
            params.handle.field_mask & ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLER as u64,
            0
        );
        assert_ne!(
            params.handle.field_mask & ucp_ep_params_field::UCP_EP_PARAM_FIELD_USER_DATA as u64,
            0
        );
    }

    #[test]
    fn err_handler_arg_alone_does_not_advertise_callback() {
        let mut builder = ParamsBuilder::new();
        let params = builder.err_handler_arg(std::ptr::null_mut()).build();
        assert_eq!(
            params.handle.field_mask & ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLER as u64,
            0
        );
    }

    #[test]
    fn close_api_has_worker_signature() {
        let _close: for<'w> fn(Ep, &'w Worker, CloseMode) -> Result<OpCompletion<'w>, CloseError> =
            Ep::close;
    }

    #[test]
    fn teardown_api_has_expected_signatures() {
        let _: fn(Ep) = Ep::destroy;
    }

    #[test]
    fn close_self_endpoint_immediately_or_with_request() {
        let context_params = crate::context::ParamsBuilder::new()
            .features(crate::context::Flags::Tag)
            .mt_workers_shared(1)
            .build();
        let mut context = crate::context::Context::new(
            &crate::context::Config::read("", "").expect("config read"),
            &context_params,
        )
        .expect("context create");
        let worker_params = crate::worker::ParamsBuilder::new().build();
        let worker = context
            .worker_create(&worker_params)
            .expect("worker create");
        let address = worker.pack_address().expect("pack address");
        let remote = crate::worker::RemoteWorkerAddress::new(address.to_vec());
        let ep_params = ParamsBuilder::new().address(&remote).build();
        let ep = worker.create_ep(ep_params).expect("endpoint create");
        drop(address);
        {
            let close = ep
                .close(&worker, CloseMode::Flush)
                .expect("endpoint close should start");
            if let crate::Completion::Pending(mut request) = close {
                let mut done = false;
                for _ in 0..1_000_000 {
                    worker.progress();
                    match request.try_complete() {
                        Ok(result) => {
                            result.expect("endpoint close");
                            done = true;
                            break;
                        }
                        Err(still) => request = still,
                    }
                }
                assert!(done, "endpoint close did not complete");
            }
        }
        drop(worker);
        drop(context);
    }

    /// Passing another worker hands the endpoint back instead of consuming it.
    #[test]
    fn close_with_wrong_worker_returns_endpoint() {
        let context_params = crate::context::ParamsBuilder::new()
            .features(crate::context::Flags::Tag)
            .mt_workers_shared(1)
            .build();
        let mut context = crate::context::Context::new(
            &crate::context::Config::read("", "").expect("config read"),
            &context_params,
        )
        .expect("context create");
        let worker = context
            .worker_create(&crate::worker::ParamsBuilder::new().build())
            .expect("worker");
        let other = context
            .worker_create(&crate::worker::ParamsBuilder::new().build())
            .expect("other worker");
        let address = worker.pack_address().expect("pack address");
        let remote = crate::worker::RemoteWorkerAddress::new(address.to_vec());
        drop(address);
        let ep = worker
            .create_ep(ParamsBuilder::new().address(&remote).build())
            .expect("endpoint");
        assert!(matches!(
            ep.flush(&other, &crate::RequestParamBuilder::new().build()),
            Err(status) if status == Status(ucs_status_t::UCS_ERR_INVALID_PARAM)
        ));
        let error = match ep.close(&other, CloseMode::Flush) {
            Err(error) => error,
            Ok(_) => panic!("close on the wrong worker must fail"),
        };
        assert_eq!(error.status, Status(ucs_status_t::UCS_ERR_INVALID_PARAM));
        let ep = error.ep.expect("endpoint handed back");
        let close = ep
            .close(&worker, CloseMode::Flush)
            .expect("close on its own worker");
        if let crate::Completion::Pending(mut request) = close {
            for _ in 0..1_000_000 {
                worker.progress();
                match request.try_complete() {
                    Ok(result) => {
                        result.expect("close");
                        break;
                    }
                    Err(still) => request = still,
                }
            }
        }
    }

    #[test]
    fn ep_params_sockaddr_points_at_borrowed_address() {
        let addr = "127.0.0.1:13337".parse().unwrap();
        let sock = SockAddr::new(&addr);
        let mut builder = ParamsBuilder::new();
        let params = builder.sockaddr(&sock).build();
        assert_ne!(
            params.handle.field_mask & ucp_ep_params_field::UCP_EP_PARAM_FIELD_SOCK_ADDR as u64,
            0
        );
        let raw = sock.to_ffi();
        assert_eq!(params.handle.sockaddr.addr, raw.addr);
        assert_eq!(params.handle.sockaddr.addrlen, raw.addrlen);
    }
}
