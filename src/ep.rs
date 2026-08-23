//! # Teardown
//!
//! [`Ep::close`] is the explicit graceful teardown path and waits for the
//! close request to complete. [`Drop`] performs a best-effort graceful close
//! when the worker is still alive. [`Ep::destroy`] and [`Ep::disconnect_nb`]
//! are force/error-path escape hatches; use them only for broken endpoints and
//! never while operations are pending on the endpoint.

use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::RemoteWorkerAddress;
use crate::worker::Worker;
use crate::worker::WorkerAddress;
use bitflags::bitflags;
use std::ffi::CString;
use std::ptr::NonNull;
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
}

impl Ep {
    /// Expose the raw UCP endpoint handle for FFI callers.
    pub fn handle(&self) -> ucp_ep_h {
        self.handle
    }

    /// Print endpoint diagnostics to `fd`. Invalid descriptors are ignored.
    pub fn print_info(&self, fd: std::os::fd::RawFd) {
        let _ = crate::config::with_file_stream(fd, |stream| {
            // SAFETY: self owns a live endpoint and stream is valid for this call.
            unsafe { ucp_ep_print_info(self.handle, stream.cast()) };
        });
    }

    pub fn new(ep_params: Params, worker: &Worker) -> Result<Ep, ucs_status_t> {
        let mut ep: ucp_ep_h = std::ptr::null_mut();
        let result =
            status_to_result(unsafe { ucp_ep_create(worker.handle, &ep_params.handle, &mut ep) });
        match result {
            Ok(()) => Ok(Ep {
                handle: ep,
                worker_alive: Arc::clone(&worker.alive),
            }),
            Err(ucs_status_t) => Err(ucs_status_t),
        }
    }

    /// Flush all outstanding AMO and RMA operations on this endpoint.
    ///
    /// Completion guarantees that operations issued before the flush have
    /// completed at both the origin and target. This wrapper preserves
    /// immediate completion and UCX error statuses.
    pub fn flush(
        &self,
        params: &crate::RequestParam,
    ) -> Result<Option<crate::Request>, ucs_status_t> {
        status_ptr_to_result(unsafe { ucp_ep_flush_nbx(self.handle, &params.handle) })
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

    /// Start the deprecated legacy disconnect operation.
    ///
    /// This is an error-path/force-teardown escape hatch and must not be used
    /// while operations are pending. [`Ep::close`] is recommended for normal
    /// graceful teardown; `ucp_ep_close_nb` replaces this deprecated UCX API.
    /// The returned request, when present, is owned by the caller.
    pub fn disconnect_nb(self) -> Result<Option<crate::Request>, ucs_status_t> {
        let this = std::mem::ManuallyDrop::new(self);
        if !this.worker_alive.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(None);
        }
        // SAFETY: this endpoint is owned and its worker is alive. On an
        // immediate error, UCX did not take ownership, so destroy the handle.
        let result = status_ptr_to_result(unsafe { ucp_disconnect_nb(this.handle) });
        if result.is_err() {
            // SAFETY: the disconnect call returned an error without taking
            // ownership of this endpoint; `this` is its sole owner.
            unsafe { ucp_ep_destroy(this.handle) };
        }
        result
    }

    /// Modify endpoint error handling and user data.
    ///
    /// `ucp_ep_modify_nb` is an upstream deprecated compatibility API declared
    /// in `ucp_compat.h`.
    pub fn modify(&self, params: &ModifyParams) -> Result<Option<crate::Request>, ucs_status_t> {
        // SAFETY: self.handle is a live endpoint and params owns initialized storage.
        status_ptr_to_result(unsafe { ucp_ep_modify_nb(self.handle, &params.handle) })
    }

    /// Estimate the time needed to send a message of `message_size` bytes.
    pub fn evaluate_perf(&self, message_size: usize) -> Result<EpPerf, ucs_status_t> {
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
    pub fn query(&self, mask: EpAttrFields) -> Result<EpAttr, ucs_status_t> {
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
                    .then_some(attr.local_sockaddr),
                remote_sockaddr: mask
                    .contains(EpAttrFields::REMOTE_SOCKADDR)
                    .then_some(attr.remote_sockaddr),
                transports: mask
                    .contains(EpAttrFields::TRANSPORTS)
                    .then_some(attr.transports),
                user_data: mask
                    .contains(EpAttrFields::USER_DATA)
                    .then_some(attr.user_data),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_flush_api_signature() {
        let _flush: fn(&Ep, &crate::RequestParam) -> Result<Option<crate::Request>, ucs_status_t> =
            Ep::flush;
    }

    #[test]
    fn endpoint_issue_37_api_signatures() {
        let _: fn(&Ep, &ModifyParams) -> Result<Option<crate::Request>, ucs_status_t> = Ep::modify;
        let _: fn(&Ep, usize) -> Result<EpPerf, ucs_status_t> = Ep::evaluate_perf;
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
        let params = builder
            .err_mode(ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_PEER)
            .build();

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
        unsafe extern "C" fn callback(
            _arg: *mut std::ffi::c_void,
            _ep: ucp_ep_h,
            _status: ucs_status_t,
        ) {
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
        let _close: fn(Ep, &Worker, u32) -> Result<(), ucs_status_t> = Ep::close;
    }

    #[test]
    fn teardown_api_has_expected_signatures() {
        let _: fn(Ep) = Ep::destroy;
        let _: fn(Ep) -> Result<Option<crate::Request>, ucs_status_t> = Ep::disconnect_nb;
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
        ep.close(&worker, 0)
            .expect("endpoint close should complete");
        drop(worker);
        drop(context);
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct EpAttrFields: u64 {
        const NAME = 1 << 0;
        const LOCAL_SOCKADDR = 1 << 1;
        const REMOTE_SOCKADDR = 1 << 2;
        const TRANSPORTS = 1 << 3;
        const USER_DATA = 1 << 4;
    }
}

pub struct EpAttr {
    pub name: String,
    pub local_sockaddr: Option<sockaddr_storage>,
    pub remote_sockaddr: Option<sockaddr_storage>,
    pub transports: Option<ucp_transports_t>,
    pub user_data: Option<*mut std::os::raw::c_void>,
}

/// Parameters supported by [`Ep::modify`].
#[derive(Debug)]
pub struct ModifyParams {
    pub(crate) handle: ucp_ep_params_t,
}

#[derive(Debug)]
pub struct ModifyParamsBuilder {
    handle: ucp_ep_params_t,
}

impl ModifyParamsBuilder {
    pub fn new() -> Self {
        // SAFETY: UCX parameter structs are valid when zeroed; the field mask
        // controls which fields UCX reads.
        Self {
            handle: unsafe { std::mem::zeroed() },
        }
    }

    /// Set the endpoint error callback.
    pub fn err_handler(&mut self, cb: ucp_err_handler_cb_t) -> &mut Self {
        self.handle.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLER as u64;
        self.handle.err_handler.cb = cb;
        self
    }

    /// Set the argument passed to the endpoint error callback.
    pub fn err_handler_arg(&mut self, ptr: *mut std::ffi::c_void) -> &mut Self {
        self.handle.err_handler.arg = ptr;
        self
    }

    /// Set opaque endpoint user data.
    pub fn user_data(&mut self, ptr: *mut std::ffi::c_void) -> &mut Self {
        self.handle.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_USER_DATA as u64;
        self.handle.user_data = ptr;
        self
    }

    pub fn build(&self) -> ModifyParams {
        ModifyParams {
            handle: self.handle,
        }
    }
}

impl Default for ModifyParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Estimated endpoint performance for a requested message size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EpPerf {
    pub estimated_time: f64,
}

impl Ep {
    /// Start closing this endpoint and return the completion request, if any.
    /// The caller owns progress of a returned request and should check it while
    /// progressing the associated worker. If close times out, it returns
    /// `Err(UCS_ERR_TIMED_OUT)` after leaking the in-flight close request; the
    /// endpoint must be recreated before it is used again.
    pub fn close(self, worker: &Worker, flags: u32) -> Result<(), ucs_status_t> {
        let this = std::mem::ManuallyDrop::new(self);
        // SAFETY: UCX request parameter structs are valid when zeroed.
        let mut param: ucp_request_param_t = unsafe { std::mem::zeroed() };
        param.flags = flags;
        // SAFETY: this.handle is owned by the endpoint and param is initialized.
        let request = status_ptr_to_result(unsafe { ucp_ep_close_nbx(this.handle, &param) })?;
        if let Some(request) = request {
            for _ in 0..1_000_000 {
                match request.check_finished() {
                    Ok(true) => {
                        request.free();
                        return Ok(());
                    }
                    Ok(false) => {
                        worker.progress();
                    }
                    Err(error) => {
                        // Do not free a request that may still be in flight.
                        std::mem::forget(request);
                        return Err(error);
                    }
                }
            }
            // Do not free a request that may still be in flight.
            std::mem::forget(request);
            return Err(ucs_status_t::UCS_ERR_TIMED_OUT);
        }
        Ok(())
    }
}

impl Drop for Ep {
    fn drop(&mut self) {
        if !self.worker_alive.load(std::sync::atomic::Ordering::Acquire) {
            eprintln!("ucx-rs: endpoint dropped after its worker; skipping close");
            return;
        }
        // SAFETY: UCX request parameter structs are valid when zeroed.
        let param: ucp_request_param_t = unsafe { std::mem::zeroed() };
        // Close returns Ok(None) if complete, Ok(Some(req)) if in progress.
        // Request::Drop frees the request; do not free manually (would double-free).
        // SAFETY: self.handle is owned by this endpoint and param is initialized.
        match status_ptr_to_result(unsafe { ucp_ep_close_nbx(self.handle, &param) }) {
            Ok(Some(_req)) => {
                eprintln!("ucx-rs: endpoint close is still in progress during Drop; call Ep::close(&worker, flags) first");
                // Do not free a request whose close has not completed.
                std::mem::forget(_req);
            }
            Ok(None) => {}
            Err(error) => eprintln!("ucx-sys: endpoint close during Drop failed: {error:?}; UCX released endpoint resources"),
        }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ParamsFlags: u64 {
        const ClientServer = ucp_ep_params_flags_field::UCP_EP_PARAMS_FLAGS_CLIENT_SERVER as u64;
        const NoLoopback = ucp_ep_params_flags_field::UCP_EP_PARAMS_FLAGS_NO_LOOPBACK as u64;
        const SendClientId = ucp_ep_params_flags_field::UCP_EP_PARAMS_FLAGS_SEND_CLIENT_ID as u64;
    }
}

#[derive(Debug)]
pub struct Params {
    pub(crate) handle: ucp_ep_params_t,
    name: Option<CString>,
}

#[derive(Debug)]
pub struct ParamsBuilder {
    uninit_handle: std::mem::MaybeUninit<ucp_ep_params_t>,
    field_mask: u64,
    name: Option<CString>,
}

impl Default for ParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ParamsBuilder {
    pub fn new() -> ParamsBuilder {
        // SAFETY: UCX parameter structs are valid when zeroed; the field mask controls reads.
        let uninit_params = std::mem::MaybeUninit::new(unsafe { std::mem::zeroed() });
        ParamsBuilder {
            uninit_handle: uninit_params,
            field_mask: 0,
            name: None,
        }
    }

    pub fn address(&mut self, worker_address: &RemoteWorkerAddress) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_REMOTE_ADDRESS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        let (address, _) = worker_address.get_handle();
        params.address = address;
        self
    }

    pub fn sockaddr(&mut self, addr: &ucs_sock_addr_t) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_SOCK_ADDR as u64;
        // SAFETY: builder storage is initialized and the sockaddr is copied.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).sockaddr = *addr;
        }
        self
    }

    pub fn conn_request(&mut self, req: ucp_conn_request_h) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_CONN_REQUEST as u64;
        // SAFETY: req is an opaque UCX handle accepted by this parameter.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).conn_request = req;
        }
        self
    }

    pub fn params_flags(&mut self, flags: ParamsFlags) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_FLAGS as u64;
        // SAFETY: builder storage is initialized and flags is a typed UCX mask.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).flags = flags.bits() as u32;
        }
        self
    }

    pub fn err_mode(&mut self, mode: ucp_err_handling_mode_t) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLING_MODE as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.err_mode = mode;
        self
    }

    /// Configure the endpoint error callback. Its state may be supplied with `user_data`.
    pub fn err_handler(&mut self, cb: ucp_err_handler_cb_t) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLER as u64;
        // SAFETY: uninit_handle is initialized by ParamsBuilder::new and this
        // field is written before build exposes the struct.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).err_handler.cb = cb;
        }
        self
    }

    /// Set the callback argument passed to the endpoint error callback.
    pub fn err_handler_arg(&mut self, ptr: *mut std::ffi::c_void) -> &mut ParamsBuilder {
        // SAFETY: uninit_handle is initialized by ParamsBuilder::new and this
        // field is written before build exposes the struct.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).err_handler.arg = ptr;
        }
        self
    }

    /// Set opaque endpoint user data.
    pub fn user_data(&mut self, ptr: *mut std::ffi::c_void) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_USER_DATA as u64;
        // SAFETY: uninit_handle is initialized by ParamsBuilder::new and this
        // field is written before build exposes the struct.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).user_data = ptr;
        }
        self
    }

    pub fn name(&mut self, name: &str) -> Result<&mut ParamsBuilder, std::ffi::NulError> {
        let name_cs = CString::new(name)?;
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_NAME as u64;
        self.name = Some(name_cs);
        Ok(self)
    }

    pub fn build(&mut self) -> Params {
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.field_mask = self.field_mask;
        let mut ep_param = Params {
            handle: unsafe { self.uninit_handle.assume_init() },
            name: None,
        };
        if let Some(new_name) = self.name.take() {
            ep_param.handle.name = new_name.as_ptr();
            ep_param.name = Some(new_name);
        }
        ep_param
    }
}
