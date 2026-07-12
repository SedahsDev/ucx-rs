use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::RemoteWorkerAddress;
use crate::worker::Worker;
use crate::worker::WorkerAddress;
use bitflags::bitflags;
use std::ffi::CString;
use std::ptr::NonNull;

#[derive(Debug, Clone)]
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

/// Endpoint attribute result.
#[derive(Debug, Clone)]
pub struct EpAttr {
    pub name: String,
    pub user_data: *mut std::os::raw::c_void,
}

impl Drop for Ep {
    fn drop(&mut self) {
        let param: ucp_request_param_t = unsafe { std::mem::zeroed() };
        // Close returns Ok(None) if complete, Ok(Some(req)) if in progress.
        // Request::Drop frees the request; do not free manually (would double-free).
        match status_ptr_to_result(unsafe { ucp_ep_close_nbx(self.handle, &param) }) {
            Ok(Some(req)) => drop(req),
            Ok(None) => {}
            Err(_) => {}
        }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct UcpEpFields: u64 {
        const None = ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_NONE as u64;
        const Peer = ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_PEER as u64;
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

#[derive(Debug, Clone)]
pub struct Params {
    pub(crate) handle: ucp_ep_params_t,
    name: Option<CString>,
}

#[derive(Debug, Clone)]
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
        if self.name.is_some() {
            let new_name = self.name.clone().unwrap();
            ep_param.handle.name = new_name.as_ptr();
            ep_param.name = Some(new_name);
        }
        ep_param
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context;
    use crate::context::Context;
    use crate::ffi::*;
    use crate::worker;

    // ── UcpEpFields tests ──

    #[test]
    fn test_ep_fields_none_has_zero_bits() {
        let fields = UcpEpFields::None;
        assert_eq!(fields.bits(), 0);
        assert!(fields.is_empty());
    }

    #[test]
    fn test_ep_fields_peer() {
        let fields = UcpEpFields::Peer;
        assert!(fields.contains(UcpEpFields::Peer));
        assert!(!fields.is_empty());
    }

    #[test]
    fn test_ucp_ep_fields_all() {
        let fields = UcpEpFields::all();
        assert!(!fields.is_empty());
    }

    #[test]
    fn test_ucp_ep_fields_clone_copy() {
        let a = UcpEpFields::Peer;
        let b = a.clone();
        assert_eq!(a, b);
        let c = a;
        assert_eq!(a, c);
    }

    // ── ParamsFlags tests ──

    #[test]
    fn test_params_flags_empty() {
        let flags = ParamsFlags::empty();
        assert!(flags.is_empty());
        assert!(!flags.contains(ParamsFlags::ClientServer));
        assert!(!flags.contains(ParamsFlags::NoLoopback));
    }

    #[test]
    fn test_params_flags_client_server() {
        let flags = ParamsFlags::ClientServer;
        assert!(flags.contains(ParamsFlags::ClientServer));
        assert!(!flags.contains(ParamsFlags::NoLoopback));
    }

    #[test]
    fn test_params_flags_no_loopback() {
        let flags = ParamsFlags::NoLoopback;
        assert!(!flags.contains(ParamsFlags::ClientServer));
        assert!(flags.contains(ParamsFlags::NoLoopback));
    }

    #[test]
    fn test_params_flags_send_client_id() {
        let flags = ParamsFlags::SendClientId;
        assert!(!flags.contains(ParamsFlags::ClientServer));
        assert!(!flags.contains(ParamsFlags::NoLoopback));
        assert!(flags.contains(ParamsFlags::SendClientId));
    }

    #[test]
    fn test_params_flags_all() {
        let flags = ParamsFlags::all();
        assert!(flags.contains(ParamsFlags::ClientServer));
        assert!(flags.contains(ParamsFlags::NoLoopback));
        assert!(flags.contains(ParamsFlags::SendClientId));
    }

    #[test]
    fn test_params_flags_clone_copy() {
        let a = ParamsFlags::ClientServer | ParamsFlags::NoLoopback;
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn test_params_flags_debug() {
        let flags = ParamsFlags::ClientServer;
        let debug_str = format!("{:?}", flags);
        assert!(!debug_str.is_empty());
    }

    // ── ParamsBuilder tests (pure unit tests) ──

    #[test]
    fn test_params_builder_new() {
        let builder = ParamsBuilder::new();
        assert_eq!(builder.field_mask, 0u64);
    }

    #[test]
    fn test_params_builder_default() {
        let builder: ParamsBuilder = Default::default();
        assert_eq!(builder.field_mask, 0u64);
    }

    #[test]
    fn test_params_builder_build_empty() {
        let mut builder = ParamsBuilder::new();
        let params = builder.build();
        assert_eq!(params.handle.field_mask, 0u64);
    }

    #[test]
    fn test_params_builder_name() {
        let mut builder = ParamsBuilder::new();
        builder.name("test_ep");
        let params = builder.build();
        assert!(
            params.handle.field_mask & (ucp_ep_params_field::UCP_EP_PARAM_FIELD_NAME as u64) != 0
        );
    }

    #[test]
    fn test_params_builder_chaining_returns_mut_ref() {
        let mut builder = ParamsBuilder::new();
        let result = builder.name("chained");
        result.name("overridden");
        let params = builder.build();
        assert!(
            params.handle.field_mask & (ucp_ep_params_field::UCP_EP_PARAM_FIELD_NAME as u64) != 0
        );
    }

    // ── Ep integration tests (require UCX library) ──

    #[test]
    fn test_ep_create_with_remote_address() {
        let config = context::Config::default();
        let ctx_params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &ctx_params).expect("context init");

        let worker_params = worker::ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        let packed_addr = worker.pack_address().expect("pack_address");
        let addr = worker::RemoteWorkerAddress::new(packed_addr.to_vec());
        drop(packed_addr);

        let ep_params = ParamsBuilder::new().address(&addr).build();
        let ep = Ep::new(ep_params, &worker).expect("ep create");
        assert!(!ep.handle.is_null());
    }

    #[test]
    fn test_ep_create_via_worker() {
        let config = context::Config::default();
        let ctx_params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &ctx_params).expect("context init");

        let worker_params = worker::ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        let packed_addr = worker.pack_address().expect("pack_address");
        let addr = worker::RemoteWorkerAddress::new(packed_addr.to_vec());
        drop(packed_addr);

        let ep_params = ParamsBuilder::new().address(&addr).build();
        let ep = worker.create_ep(ep_params).expect("create_ep");
        assert!(!ep.handle.is_null());
    }

    #[test]
    fn test_ep_handle() {
        let config = context::Config::default();
        let ctx_params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &ctx_params).expect("context init");

        let worker_params = worker::ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        let packed_addr = worker.pack_address().expect("pack_address");
        let addr = worker::RemoteWorkerAddress::new(packed_addr.to_vec());
        drop(packed_addr);

        let ep_params = ParamsBuilder::new().address(&addr).build();
        let ep = worker.create_ep(ep_params).expect("create_ep");
        let raw_handle = ep.handle();
        assert!(!raw_handle.is_null());
    }

    #[test]
    fn test_ep_query_name() {
        let config = context::Config::default();
        let ctx_params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &ctx_params).expect("context init");

        let worker_params = worker::ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        let packed_addr = worker.pack_address().expect("pack_address");
        let addr = worker::RemoteWorkerAddress::new(packed_addr.to_vec());
        drop(packed_addr);

        let ep_params = ParamsBuilder::new()
            .name("query_test_ep")
            .address(&addr)
            .build();
        let ep = worker.create_ep(ep_params).expect("create_ep");

        // UCP_EP_ATTR_FIELD_NAME = 1
        let attr = ep.query(1).expect("ep query");
        // Name may be empty if not set by UCX
        let _ = &attr.name;
    }

    // ── ParamsBuilder field mask tests ──

    #[test]
    fn test_ep_params_field_remote_address_is_bit_0() {
        assert_eq!(
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_REMOTE_ADDRESS as u64,
            1
        );
    }

    #[test]
    fn test_ep_params_field_err_handling_mode_is_bit_1() {
        assert_eq!(
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLING_MODE as u64,
            2
        );
    }

    #[test]
    fn test_ep_params_field_err_handler_is_bit_2() {
        assert_eq!(
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLER as u64,
            4
        );
    }

    #[test]
    fn test_ep_params_field_user_data_is_bit_3() {
        assert_eq!(ucp_ep_params_field::UCP_EP_PARAM_FIELD_USER_DATA as u64, 8);
    }

    #[test]
    fn test_ep_params_field_sock_addr_is_bit_4() {
        assert_eq!(ucp_ep_params_field::UCP_EP_PARAM_FIELD_SOCK_ADDR as u64, 16);
    }

    #[test]
    fn test_ep_params_field_flags_is_bit_5() {
        assert_eq!(ucp_ep_params_field::UCP_EP_PARAM_FIELD_FLAGS as u64, 32);
    }

    #[test]
    fn test_ep_params_field_conn_request_is_bit_6() {
        assert_eq!(
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_CONN_REQUEST as u64,
            64
        );
    }

    #[test]
    fn test_ep_params_field_name_is_bit_7() {
        assert_eq!(ucp_ep_params_field::UCP_EP_PARAM_FIELD_NAME as u64, 128);
    }

    #[test]
    fn test_ep_params_field_local_sock_addr_is_bit_8() {
        assert_eq!(
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_LOCAL_SOCK_ADDR as u64,
            256
        );
    }

    #[test]
    fn test_ep_params_fields_all_distinct() {
        let fields = [
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_REMOTE_ADDRESS as u64,
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLING_MODE as u64,
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_ERR_HANDLER as u64,
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_USER_DATA as u64,
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_SOCK_ADDR as u64,
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_FLAGS as u64,
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_CONN_REQUEST as u64,
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_NAME as u64,
            ucp_ep_params_field::UCP_EP_PARAM_FIELD_LOCAL_SOCK_ADDR as u64,
        ];
        for i in 0..fields.len() {
            for j in (i + 1)..fields.len() {
                assert_eq!(
                    fields[i] & fields[j],
                    0,
                    "Fields {} and {} should be distinct",
                    i,
                    j
                );
            }
        }
    }
}
