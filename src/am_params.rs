//! AM handler parameters for setting receive callbacks.
//!
//! This wraps the `ucp_am_recv_param_t` FFI type.

/// Builder for AM receive handler parameters.
pub struct AmHandlerParamsBuilder {
    uninit_handle: std::mem::MaybeUninit<crate::ffi::ucp_am_handler_param_t>,
    flags: u64,
}

impl Default for AmHandlerParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AmHandlerParamsBuilder {
    /// Create a new builder with zeroed parameters.
    #[inline]
    pub fn new() -> Self {
        // SAFETY: UCX parameter structs are valid when zeroed.
        let uninit_params = std::mem::MaybeUninit::new(unsafe { std::mem::zeroed() });
        Self {
            uninit_handle: uninit_params,
            flags: 0,
        }
    }

    /// Set the AM ID.
    #[inline]
    pub fn id(&mut self, id: u32) -> &mut Self {
        self.flags |= crate::ffi::ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ID as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.id = id;
        self
    }

    /// Set the flags.
    #[inline]
    pub fn flags(&mut self, flags: crate::am::CbFlags) -> &mut Self {
        self.flags |= crate::ffi::ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_FLAGS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.flags = flags.bits();
        self
    }

    /// Set the callback function.
    #[inline]
    pub fn cb(&mut self, cb: crate::am::AmRecvCb) -> &mut Self {
        self.flags |= crate::ffi::ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_CB as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.cb = Some(cb);
        self
    }

    /// Set the user data argument.
    #[inline]
    pub fn arg(&mut self, arg: *mut std::os::raw::c_void) -> &mut Self {
        self.flags |= crate::ffi::ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ARG as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.arg = arg;
        self
    }

    /// Build the parameters.
    #[inline]
    pub fn build(self) -> crate::AmHandlerParams {
        // SAFETY: we've initialized the handle through the builder.
        let handle = unsafe { std::mem::transmute(self.uninit_handle.assume_init()) };
        crate::AmHandlerParams {
            handle,
        }
    }
}

/// Parameters for AM receive handler registration.
pub struct AmHandlerParams {
    handle: crate::ffi::ucp_am_handler_param_t,
}

impl AmHandlerParams {
    /// Get the underlying raw handle (for internal use).
    #[inline]
    pub(crate) fn as_raw(&self) -> &crate::ffi::ucp_am_handler_param_t {
        &self.handle
    }
}
