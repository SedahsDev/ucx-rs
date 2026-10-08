use crate::ffi::*;
use bitflags::bitflags;
use std::ffi::CString;
use std::marker::PhantomData;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ParamsFlags: u64 {
        const ClientServer = ucp_ep_params_flags_field::UCP_EP_PARAMS_FLAGS_CLIENT_SERVER as u64;
        const NoLoopback = ucp_ep_params_flags_field::UCP_EP_PARAMS_FLAGS_NO_LOOPBACK as u64;
        const SendClientId = ucp_ep_params_flags_field::UCP_EP_PARAMS_FLAGS_SEND_CLIENT_ID as u64;
    }
}

/// Built endpoint parameters, ready for [`crate::worker::Worker::create_ep`].
///
/// `'a` is the borrow of any [`crate::ep::SockAddr`] or
/// [`crate::worker::RemoteWorkerAddress`] given to the builder. UCX reads those addresses
/// inside `ucp_ep_create`, so they must stay alive until the endpoint has been created.
#[derive(Debug)]
pub struct Params<'a> {
    pub(crate) handle: ucp_ep_params_t,
    name: Option<CString>,
    _borrows: PhantomData<&'a ()>,
}

/// Builder for [`Params`]. `'a` is the borrow of the addresses passed to
/// [`ParamsBuilder::address`] and [`ParamsBuilder::sockaddr`].
#[derive(Debug)]
pub struct ParamsBuilder<'a> {
    uninit_handle: std::mem::MaybeUninit<ucp_ep_params_t>,
    field_mask: u64,
    name: Option<CString>,
    _borrows: PhantomData<&'a ()>,
}

impl Default for ParamsBuilder<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> ParamsBuilder<'a> {
    pub fn new() -> Self {
        // SAFETY: UCX parameter structs are valid when zeroed; the field mask controls reads.
        let uninit_params = std::mem::MaybeUninit::new(unsafe { std::mem::zeroed() });
        Self {
            uninit_handle: uninit_params,
            field_mask: 0,
            name: None,
            _borrows: PhantomData,
        }
    }

    pub fn address(&mut self, worker_address: &'a crate::worker::RemoteWorkerAddress) -> &mut Self {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_REMOTE_ADDRESS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        let (address, _) = worker_address.get_handle();
        params.address = address;
        self
    }

    /// Connect to the socket address `addr` (client side of a client/server connection).
    ///
    /// The builder and the [`Params`] built from it borrow `addr`: UCX reads the address
    /// inside [`crate::worker::Worker::create_ep`], so `addr` must stay alive until the
    /// endpoint has been created. The compiler enforces this:
    ///
    /// ```no_run
    /// use ucx_sys::ep::{Ep, ParamsBuilder, SockAddr};
    ///
    /// fn connect(worker: &ucx_sys::worker::Worker) -> Result<Ep, ucx_sys::Status> {
    ///     let addr = "127.0.0.1:13337".parse().unwrap();
    ///     let sa = SockAddr::new(&addr);
    ///     let ep = worker.create_ep(ParamsBuilder::new().sockaddr(&sa).build());
    ///     ep
    /// }
    /// ```
    ///
    /// Dropping the address before the endpoint is created is rejected (issue #106):
    ///
    /// ```compile_fail,E0597
    /// use ucx_sys::ep::{Ep, ParamsBuilder, SockAddr};
    ///
    /// fn dangling(worker: &ucx_sys::worker::Worker) -> Result<Ep, ucx_sys::Status> {
    ///     let addr = "127.0.0.1:13337".parse().unwrap();
    ///     let params = {
    ///         let sa = SockAddr::new(&addr);
    ///         ParamsBuilder::new().sockaddr(&sa).build()
    ///     }; // `sa` (and its boxed sockaddr) is freed here
    ///     worker.create_ep(params) // UCX would read the freed sockaddr
    /// }
    /// ```
    pub fn sockaddr(&mut self, addr: &'a crate::ep::SockAddr) -> &mut Self {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_SOCK_ADDR as u64;
        // SAFETY: builder storage is initialized. UCX reads (and copies) the address inside
        // ucp_ep_create; the 'a borrow keeps `addr` alive until the built Params are consumed.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).sockaddr = addr.to_ffi();
        }
        self
    }

    pub fn conn_request(&mut self, req: ucp_conn_request_h) -> &mut Self {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_CONN_REQUEST as u64;
        // SAFETY: req is an opaque UCX handle accepted by this parameter.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).conn_request = req;
        }
        self
    }

    pub fn params_flags(&mut self, flags: ParamsFlags) -> &mut Self {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_FLAGS as u64;
        // SAFETY: builder storage is initialized and flags is a typed UCX mask.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).flags = flags.bits() as u32;
        }
        self
    }

    pub fn err_mode(&mut self, mode: crate::ErrorHandlerMode) -> &mut Self {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLING_MODE as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.err_mode = mode.to_ffi();
        self
    }

    /// Configure the endpoint error callback. Its state may be supplied with `user_data`.
    pub fn err_handler(&mut self, cb: crate::ep::ErrHandlerCb) -> &mut Self {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLER as u64;
        // SAFETY: uninit_handle is initialized by ParamsBuilder::new and this
        // field is written before build exposes the struct.
        // `Status` is `#[repr(transparent)]` over `ucs_status_t`, so `ErrHandlerCb` and UCX's
        // `ucp_err_handler_cb_t` have ABI-compatible signatures.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).err_handler.cb =
                std::mem::transmute::<crate::ep::ErrHandlerCb, ucp_err_handler_cb_t>(cb);
        }
        self
    }

    /// Set the callback argument passed to the endpoint error callback.
    pub fn err_handler_arg(&mut self, ptr: *mut std::ffi::c_void) -> &mut Self {
        // SAFETY: uninit_handle is initialized by ParamsBuilder::new and this
        // field is written before build exposes the struct.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).err_handler.arg = ptr;
        }
        self
    }

    /// Set opaque endpoint user data.
    pub fn user_data(&mut self, ptr: *mut std::ffi::c_void) -> &mut Self {
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_USER_DATA as u64;
        // SAFETY: uninit_handle is initialized by ParamsBuilder::new and this
        // field is written before build exposes the struct.
        unsafe {
            (*self.uninit_handle.as_mut_ptr()).user_data = ptr;
        }
        self
    }

    pub fn name(&mut self, name: &str) -> Result<&mut Self, std::ffi::NulError> {
        let name_cs = CString::new(name)?;
        self.field_mask |= ucp_ep_params_field::UCP_EP_PARAM_FIELD_NAME as u64;
        self.name = Some(name_cs);
        Ok(self)
    }

    pub fn build(&mut self) -> Params<'a> {
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.field_mask = self.field_mask;
        let mut ep_param = Params {
            handle: unsafe { self.uninit_handle.assume_init() },
            name: None,
            _borrows: PhantomData,
        };
        if let Some(new_name) = self.name.take() {
            ep_param.handle.name = new_name.as_ptr();
            ep_param.name = Some(new_name);
        }
        ep_param
    }
}
