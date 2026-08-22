use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::RemoteWorkerAddress;
use crate::worker::Worker;
use crate::worker::WorkerAddress;
use bitflags::bitflags;
use std::ffi::CString;
use std::ptr::NonNull;

/// UCX endpoint ownership wrapper.
///
/// This type is intentionally not `Clone`: cloning would create two owners of
/// one endpoint and cause a double close/use-after-free.
#[derive(Debug)]
pub struct Ep {
    pub(crate) handle: ucp_ep_h,
}

impl Ep {
    /// Expose the raw UCP endpoint handle for FFI callers.
    pub fn handle(&self) -> ucp_ep_h {
        self.handle
    }

    pub fn new(ep_params: Params, worker: &Worker) -> Result<Ep, ucs_status_t> {
        let mut ep: ucp_ep_h = std::ptr::null_mut();
        let result =
            status_to_result(unsafe { ucp_ep_create(worker.handle, &ep_params.handle, &mut ep) });
        match result {
            Ok(()) => Ok(Ep { handle: ep }),
            Err(ucs_status_t) => Err(ucs_status_t),
        }
    }

    /// Flush the endpoint.
    pub fn flush_nbx(&self) -> crate::Request {
        unsafe {
            let ptr = ucp_ep_flush_nbx(self.handle, std::ptr::null());
            crate::Request::from_raw(ptr)
        }
    }

    /// Flush all outstanding AMO and RMA operations on this endpoint.
    ///
    /// Completion guarantees that operations issued before the flush have
    /// completed at both the origin and target. Unlike [`Self::flush_nbx`],
    /// this wrapper preserves immediate completion and UCX error statuses.
    pub fn flush(
        &self,
        params: &crate::RequestParam,
    ) -> Result<Option<crate::Request>, ucs_status_t> {
        status_ptr_to_result(unsafe { ucp_ep_flush_nbx(self.handle, &params.handle) })
    }

    /// Query endpoint attributes.
    ///
    /// Field masks:
    /// - UCP_EP_ATTR_FIELD_NAME = 1
    /// - UCP_EP_ATTR_FIELD_LOCAL_SOCKADDR = 2
    /// - UCP_EP_ATTR_FIELD_REMOTE_SOCKADDR = 4
    /// - UCP_EP_ATTR_FIELD_TRANSPORTS = 8
    /// - UCP_EP_ATTR_FIELD_USER_DATA = 16
    pub fn query(&self, mask: u64) -> Result<EpAttr, ucs_status_t> {
        let mut attr: ucp_ep_attr = unsafe { std::mem::zeroed() };
        attr.field_mask = mask;
        crate::status_to_result(unsafe { ucp_ep_query(self.handle, &mut attr) }).map(|()| {
            let name = if mask & 1 != 0 {
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
                user_data: attr.user_data,
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
    fn close_api_has_consuming_request_signature() {
        let _close: fn(Ep, u32) -> Result<Option<crate::Request>, ucs_status_t> = Ep::close;
    }

    #[test]
    fn close_self_endpoint_immediately_or_with_request() {
        let context_params = crate::context::ParamsBuilder::new()
            .features(crate::context::Flags::Tag)
            .build();
        let context =
            crate::context::Context::new(&crate::context::Config::default(), &context_params)
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
        let close_request = ep.close(0).expect("endpoint close should start");
        if let Some(request) = close_request {
            for _ in 0..1_000_000 {
                if request.check_finished().expect("close request status") {
                    break;
                }
                worker.progress();
            }
            assert!(request.check_finished().expect("close request status"));
            request.free();
        }
        drop(worker);
        drop(context);
    }
}

/// Endpoint attribute result.
#[derive(Debug, Clone)]
pub struct EpAttr {
    pub name: String,
    pub user_data: *mut std::os::raw::c_void,
}

impl Ep {
    /// Start closing this endpoint and return the completion request, if any.
    /// The caller owns progress of a returned request and should check it while
    /// progressing the associated worker.
    pub fn close(self, flags: u32) -> Result<Option<crate::Request>, ucs_status_t> {
        let this = std::mem::ManuallyDrop::new(self);
        let mut param: ucp_request_param_t = unsafe { std::mem::zeroed() };
        param.flags = flags;
        status_ptr_to_result(unsafe { ucp_ep_close_nbx(this.handle, &param) })
    }
}

impl Drop for Ep {
    fn drop(&mut self) {
        let param: ucp_request_param_t = unsafe { std::mem::zeroed() };
        // Close returns Ok(None) if complete, Ok(Some(req)) if in progress.
        // Request::Drop frees the request; do not free manually (would double-free).
        match status_ptr_to_result(unsafe { ucp_ep_close_nbx(self.handle, &param) }) {
            Ok(Some(req)) => drop(req),
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
        let uninit_params = std::mem::MaybeUninit::<ucp_ep_params_t>::uninit();
        ParamsBuilder {
            uninit_handle: uninit_params,
            field_mask: 0,
            name: None,
        }
    }

    pub fn local_address(&mut self, worker_address: &WorkerAddress) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_REMOTE_ADDRESS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.address = worker_address.handle;
        self
    }

    pub fn address(&mut self, worker_address: &RemoteWorkerAddress) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_REMOTE_ADDRESS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        let (address, _) = worker_address.get_handle();
        params.address = address;
        self
    }

    pub fn err_mode(&mut self, mode: ucp_err_handling_mode_t) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLING_MODE as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.err_mode = mode;
        self
    }

    pub fn name(&mut self, name: &str) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_NAME as u64;
        let name_cs = CString::new(name).unwrap();
        self.name = Some(name_cs);
        self
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
