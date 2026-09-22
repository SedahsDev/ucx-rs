use crate::ffi::*;
use bitflags::bitflags;
use std::ffi::CString;

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

    pub fn address(&mut self, worker_address: &crate::worker::RemoteWorkerAddress) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_REMOTE_ADDRESS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        let (address, _) = worker_address.get_handle();
        params.address = address;
        self
    }

    pub fn sockaddr(&mut self, addr: &crate::ep::SockAddr) -> &mut ParamsBuilder {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_SOCK_ADDR as u64;
        // SAFETY: builder storage is initialized and the sockaddr is copied.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).sockaddr = addr.to_ffi();
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
    pub fn err_handler(&mut self, cb: crate::ep::ErrHandlerCb) -> &mut ParamsBuilder {
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
