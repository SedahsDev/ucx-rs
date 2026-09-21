use crate::ffi::*;

/// Parameters supported by [`crate::ep::Ep::modify`].
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
