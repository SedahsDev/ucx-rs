use crate::context::Context;
use crate::ep;
use crate::ep::Ep;
use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::Request;
use crate::RequestParam;
use crate::RequestParamBuilder;
use bitflags::bitflags;
use std::ffi::CString;
use std::ptr::NonNull;

#[derive(Debug, Clone)]
pub struct Worker {
    pub(crate) handle: ucp_worker_h,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let params = RequestParamBuilder::new().build();
        let request = self.flush(&params).unwrap();
        if let Some(request) = request {
            while !request.check_finished().unwrap() {
                self.progress();
            }
        }
        unsafe { ucp_worker_destroy(self.handle) };
    }
}

impl Worker {
    pub(crate) fn new(context: &Context, params: &Params) -> Result<Worker, ucs_status_t> {
        let mut worker: ucp_worker_h = std::ptr::null_mut();

        let result = status_to_result(unsafe {
            ucp_worker_create(context.handle, &params.handle, &mut worker)
        });
        match result {
            Ok(()) => Ok(Worker { handle: worker }),
            Err(ucs_status_t) => Err(ucs_status_t),
        }
    }

    pub fn pack_address(&self) -> Result<WorkerAddress<'_>, ucs_status_t> {
        let mut address: *mut ucp_address_t = std::ptr::null_mut();
        let mut size: usize = 0;

        let result = status_to_result(unsafe {
            ucp_worker_get_address(self.handle, &mut address, &mut size)
        });
        match result {
            Ok(()) => Ok(WorkerAddress {
                handle: address,
                parent: self,
                size,
            }),
            Err(ucs_status_t) => Err(ucs_status_t),
        }
    }

    #[inline]
    pub fn progress(&self) -> bool {
        let progress = unsafe { ucp_worker_progress(self.handle) };
        progress > 0
    }

    pub fn create_ep(&self, ep_params: ep::Params) -> Result<Ep, ucs_status_t> {
        Ep::new(ep_params, self)
    }

    /// Cancel a pending request on this worker.
    ///
    /// Prefer [`Request::cancel`] which also frees the request handle.
    pub fn cancel_request(&self, request: &mut Request) {
        if let Some(h) = request.handle {
            // SAFETY: both handles are live UCX objects.
            unsafe { ucp_request_cancel(self.handle, h.as_ptr()) };
        }
    }

    pub fn flush(&self, params: &RequestParam) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe { ucp_worker_flush_nbx(self.handle, &params.handle) })
    }

    /// Flush the worker (legacy variant).
    pub fn flush_nb(&self, flags: u32) -> crate::Request {
        unsafe {
            let ptr = ucp_worker_flush_nb(self.handle, flags, None);
            crate::Request::from_raw(ptr)
        }
    }

    /// Worker fence — ensures ordering of operations.
    pub fn fence(&self) -> Result<(), ucs_status_t> {
        crate::status_to_result(unsafe { ucp_worker_fence(self.handle) })
    }

    /// Arm the worker for asynchronous completion.
    pub fn arm(&self) -> Result<(), ucs_status_t> {
        crate::status_to_result(unsafe { ucp_worker_arm(self.handle) })
    }

    /// Wait for an asynchronous event on the worker.
    pub fn wait(&self) -> Result<(), ucs_status_t> {
        crate::status_to_result(unsafe { ucp_worker_wait(self.handle) })
    }

    /// Wait for an asynchronous event with memory hint.
    ///
    /// # Safety
    /// The `address` pointer is used as a memory hint by the runtime.
    pub unsafe fn wait_mem(&self, address: *mut std::os::raw::c_void) {
        ucp_worker_wait_mem(self.handle, address);
    }

    /// Signal the worker to wake up from wait.
    pub fn signal(&self) {
        unsafe {
            let _ = ucp_worker_signal(self.handle);
        }
    }

    /// Get the event file descriptor for the worker.
    pub fn get_efd(&self) -> Result<i32, ucs_status_t> {
        let mut fd: std::os::raw::c_int = -1;
        crate::status_to_result(unsafe { ucp_worker_get_efd(self.handle, &mut fd) }).map(|()| fd)
    }

    /// Query worker attributes.
    ///
    /// Field masks:
    /// - UCP_WORKER_ATTR_FIELD_THREAD_MODE = 1
    /// - UCP_WORKER_ATTR_FIELD_ADDRESS = 2
    /// - UCP_WORKER_ATTR_FIELD_ADDRESS_FLAGS = 4
    /// - UCP_WORKER_ATTR_FIELD_MAX_AM_HEADER = 8
    /// - UCP_WORKER_ATTR_FIELD_NAME = 16
    /// - UCP_WORKER_ATTR_FIELD_MAX_INFO_STRING = 32
    pub fn query(&self, mask: u64) -> Result<WorkerAttr, ucs_status_t> {
        let mut attr: ucp_worker_attr = unsafe { std::mem::zeroed() };
        attr.field_mask = mask;
        crate::status_to_result(unsafe { ucp_worker_query(self.handle, &mut attr) }).map(|()| {
            let name = if mask & (1u64 << 4) != 0 {
                unsafe {
                    std::ffi::CStr::from_ptr(attr.name.as_ptr())
                        .to_string_lossy()
                        .into_owned()
                }
            } else {
                String::new()
            };
            WorkerAttr {
                thread_mode: attr.thread_mode,
                max_am_header: attr.max_am_header,
                name,
            }
        })
    }
}

/// Worker query attribute result.
#[derive(Debug, Clone)]
pub struct WorkerAttr {
    pub thread_mode: ucs_thread_mode_t,
    pub max_am_header: usize,
    pub name: String,
}

/// Query worker address attributes.
///
/// Field mask: UCP_WORKER_ADDRESS_ATTR_FIELD_UID = 1
pub fn address_query(address: *const ucp_address_t) -> Result<u64, ucs_status_t> {
    let mut attr: ucp_worker_address_attr = unsafe { std::mem::zeroed() };
    attr.field_mask = 1; // UCP_WORKER_ADDRESS_ATTR_FIELD_UID
    crate::status_to_result(unsafe { ucp_worker_address_query(address as *mut _, &mut attr) })
        .map(|()| attr.worker_uid)
}

pub struct RemoteWorkerAddress {
    address: Vec<u8>,
}

impl RemoteWorkerAddress {
    pub fn new(address: Vec<u8>) -> RemoteWorkerAddress {
        RemoteWorkerAddress { address }
    }

    pub fn get_handle(&self) -> (*const ucp_address_t, usize) {
        (
            self.address.as_ptr() as *const ucp_address_t,
            self.address.len(),
        )
    }
}

pub struct WorkerAddress<'a> {
    pub(crate) handle: *const ucp_address_t,
    size: usize,
    parent: &'a Worker,
}

impl WorkerAddress<'_> {
    pub fn to_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.handle as *const u8, self.size) }
    }
    pub fn to_vec(&self) -> Vec<u8> {
        self.to_slice().to_vec()
    }
}

impl Drop for WorkerAddress<'_> {
    fn drop(&mut self) {
        unsafe {
            ucp_worker_release_address(self.parent.handle, self.handle as *mut ucp_address_t)
        };
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct UcpWorkerFlags: u64 {
        const IgnoreRequestLeak = ucp_worker_flags_t::UCP_WORKER_FLAG_IGNORE_REQUEST_LEAK as u64;
    }
}

/// Set active message receive handler on a worker (nbx variant).
///
/// This is a thin unsafe wrapper around `ucp_worker_set_am_recv_handler`.
///
/// # Safety
/// Caller must ensure `worker` is valid and the handler param is properly constructed.
pub unsafe fn worker_set_am_recv_handler_nbx(
    worker: ucp_worker_h,
    param: &ucp_am_handler_param_t,
) -> Result<(), ucs_status_t> {
    status_to_result(ucp_worker_set_am_recv_handler(worker, param))
}

impl ParamsBuilder {
    pub fn new() -> ParamsBuilder {
        let uninit_params = std::mem::MaybeUninit::<ucp_worker_params_t>::uninit();
        ParamsBuilder {
            uninit_handle: uninit_params,
            field_mask: 0,
            name: None,
        }
    }

    pub fn thread_mode(&mut self, thread_mode: ucs_thread_mode_t) -> &mut ParamsBuilder {
        self.field_mask |= ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_THREAD_MODE as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.thread_mode = thread_mode;
        self
    }

    pub fn cpu_set(&mut self, cpu_set: ucs_cpu_set_t) -> &mut ParamsBuilder {
        self.field_mask |= ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_CPU_MASK as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.cpu_mask = cpu_set;
        self
    }

    pub fn events(&mut self, events: u32) -> &mut ParamsBuilder {
        self.field_mask |= ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_EVENTS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.events = events;
        self
    }

    pub fn user_data(&mut self, data: *mut std::ffi::c_void) -> &mut ParamsBuilder {
        self.field_mask |= ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_USER_DATA as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.user_data = data;
        self
    }

    pub fn event_fd(&mut self, event_fd: i32) -> &mut ParamsBuilder {
        self.field_mask |= ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_EVENT_FD as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.event_fd = event_fd;
        self
    }

    pub fn flags(&mut self, flags: UcpWorkerFlags) -> &mut ParamsBuilder {
        self.field_mask |= ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_FLAGS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.flags = flags.bits();
        self
    }

    pub fn name(&mut self, name: &str) -> &mut ParamsBuilder {
        self.field_mask |= ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_NAME as u64;
        let name_cs = CString::new(name).unwrap();
        self.name = Some(name_cs);
        self
    }

    pub fn am_alignment(&mut self, am_alignment: usize) -> &mut ParamsBuilder {
        self.field_mask |= ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_AM_ALIGNMENT as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.am_alignment = am_alignment;
        self
    }

    pub fn client_id(&mut self, client_id: u64) -> &mut ParamsBuilder {
        self.field_mask |= ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_CLIENT_ID as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.client_id = client_id;
        self
    }

    pub fn build(&mut self) -> Params {
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.field_mask = self.field_mask;

        let mut ucp_param = Params {
            name: None,
            handle: unsafe { self.uninit_handle.assume_init() },
        };

        if self.name.is_some() {
            let new_name = self.name.clone().unwrap();
            ucp_param.handle.name = new_name.as_ptr();
            ucp_param.name = Some(new_name);
        }

        ucp_param
    }
}

#[derive(Debug, Clone)]
pub struct ParamsBuilder {
    uninit_handle: std::mem::MaybeUninit<ucp_worker_params_t>,
    field_mask: u64,
    name: Option<CString>,
}

impl Default for ParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct Params {
    pub(crate) handle: ucp_worker_params_t,
    name: Option<CString>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context;
    use crate::context::Context;
    use crate::ffi::*;

    // ── ParamsBuilder tests (pure unit tests, no FFI) ──

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
    fn test_params_builder_thread_mode() {
        let mut builder = ParamsBuilder::new();
        builder.thread_mode(ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_THREAD_MODE as u64)
                != 0
        );
        assert_eq!(
            params.handle.thread_mode,
            ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE
        );
    }

    #[test]
    fn test_params_builder_thread_mode_multi() {
        let mut builder = ParamsBuilder::new();
        builder.thread_mode(ucs_thread_mode_t::UCS_THREAD_MODE_MULTI);
        let params = builder.build();
        assert_eq!(
            params.handle.thread_mode,
            ucs_thread_mode_t::UCS_THREAD_MODE_MULTI
        );
    }

    #[test]
    fn test_params_builder_events() {
        let mut builder = ParamsBuilder::new();
        builder.events(0x100);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_EVENTS as u64)
                != 0
        );
        assert_eq!(params.handle.events, 0x100);
    }

    #[test]
    fn test_params_builder_user_data() {
        let mut builder = ParamsBuilder::new();
        let data: i32 = 42;
        builder.user_data(&data as *const i32 as *mut std::ffi::c_void);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_USER_DATA as u64)
                != 0
        );
        assert_eq!(
            params.handle.user_data,
            &data as *const i32 as *mut std::ffi::c_void
        );
    }

    #[test]
    fn test_params_builder_event_fd() {
        let mut builder = ParamsBuilder::new();
        builder.event_fd(99);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_EVENT_FD as u64)
                != 0
        );
        assert_eq!(params.handle.event_fd, 99);
    }

    #[test]
    fn test_params_builder_flags() {
        let mut builder = ParamsBuilder::new();
        builder.flags(UcpWorkerFlags::IgnoreRequestLeak);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_FLAGS as u64)
                != 0
        );
    }

    #[test]
    fn test_params_builder_name() {
        let mut builder = ParamsBuilder::new();
        builder.name("test_worker");
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_NAME as u64)
                != 0
        );
    }

    #[test]
    fn test_params_builder_am_alignment() {
        let mut builder = ParamsBuilder::new();
        builder.am_alignment(64);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_AM_ALIGNMENT as u64)
                != 0
        );
        assert_eq!(params.handle.am_alignment, 64);
    }

    #[test]
    fn test_params_builder_client_id() {
        let mut builder = ParamsBuilder::new();
        builder.client_id(12345);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_CLIENT_ID as u64)
                != 0
        );
        assert_eq!(params.handle.client_id, 12345);
    }

    #[test]
    fn test_params_builder_full_chain() {
        let mut builder = ParamsBuilder::new();
        let data: i32 = 99;
        builder
            .thread_mode(ucs_thread_mode_t::UCS_THREAD_MODE_MULTI)
            .events(0x100)
            .user_data(&data as *const i32 as *mut std::ffi::c_void)
            .event_fd(42)
            .flags(UcpWorkerFlags::IgnoreRequestLeak)
            .name("chain_worker")
            .am_alignment(128)
            .client_id(777);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_THREAD_MODE as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_EVENTS as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_USER_DATA as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_EVENT_FD as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_FLAGS as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_NAME as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_AM_ALIGNMENT as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_CLIENT_ID as u64)
                != 0
        );
    }

    #[test]
    fn test_params_builder_chaining_returns_mut_ref() {
        let mut builder = ParamsBuilder::new();
        let result = builder.thread_mode(ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE);
        result.events(0x200);
        let params = builder.build();
        assert_eq!(
            params.handle.thread_mode,
            ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE
        );
        assert_eq!(params.handle.events, 0x200);
    }

    // ── UcpWorkerFlags tests ──

    #[test]
    fn test_ucp_worker_flags_empty() {
        let flags = UcpWorkerFlags::empty();
        assert!(flags.is_empty());
        assert!(!flags.contains(UcpWorkerFlags::IgnoreRequestLeak));
    }

    #[test]
    fn test_ucp_worker_flags_ignore_request_leak() {
        let flags = UcpWorkerFlags::IgnoreRequestLeak;
        assert!(flags.contains(UcpWorkerFlags::IgnoreRequestLeak));
        assert!(!flags.is_empty());
    }

    #[test]
    fn test_ucp_worker_flags_all() {
        let flags = UcpWorkerFlags::all();
        assert!(!flags.is_empty());
    }

    #[test]
    fn test_ucp_worker_flags_clone_copy() {
        let a = UcpWorkerFlags::IgnoreRequestLeak;
        let b = a.clone();
        assert_eq!(a, b);
        let c = a; // Copy
        assert_eq!(a, c);
    }

    #[test]
    fn test_ucp_worker_flags_debug() {
        let flags = UcpWorkerFlags::IgnoreRequestLeak;
        let debug_str = format!("{:?}", flags);
        assert!(!debug_str.is_empty());
    }

    // ── RemoteWorkerAddress tests (pure unit tests) ──

    #[test]
    fn test_remote_worker_address_new() {
        let addr = RemoteWorkerAddress::new(vec![1, 2, 3, 4]);
        let (ptr, len) = addr.get_handle();
        assert_eq!(len, 4);
        assert!(!ptr.is_null());
    }

    #[test]
    fn test_remote_worker_address_empty() {
        let addr = RemoteWorkerAddress::new(vec![]);
        let (_, len) = addr.get_handle();
        assert_eq!(len, 0);
    }

    #[test]
    fn test_remote_worker_address_large() {
        let data = vec![0u8; 1024];
        let addr = RemoteWorkerAddress::new(data);
        let (_, len) = addr.get_handle();
        assert_eq!(len, 1024);
    }

    // ── Worker integration tests (require UCX library) ──

    #[test]
    fn test_worker_create_and_pack_address() {
        let config = context::Config::default();
        let params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &params).expect("context init");

        let worker_params = ParamsBuilder::new()
            .thread_mode(ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE)
            .build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        let addr = worker.pack_address().expect("pack_address");
        let slice = addr.to_slice();
        assert!(!slice.is_empty(), "Worker address should not be empty");
    }

    #[test]
    fn test_worker_progress_returns_bool() {
        let config = context::Config::default();
        let params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &params).expect("context init");

        let worker_params = ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        // progress() returns bool — just verify it doesn't panic
        let _ = worker.progress();
    }

    #[test]
    fn test_worker_flush() {
        let config = context::Config::default();
        let params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &params).expect("context init");

        let worker_params = ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        let param = crate::RequestParamBuilder::new().build();
        let result = worker.flush(&param);
        assert!(result.is_ok(), "flush on idle worker should succeed");
    }

    #[test]
    fn test_worker_fence() {
        let config = context::Config::default();
        let params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &params).expect("context init");

        let worker_params = ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        let result = worker.fence();
        assert!(result.is_ok(), "fence should succeed");
    }

    #[test]
    fn test_worker_query_thread_mode() {
        let config = context::Config::default();
        let params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &params).expect("context init");

        let worker_params = ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        // UCP_WORKER_ATTR_FIELD_THREAD_MODE = 1
        let attr = worker.query(1).expect("worker query");
        // thread_mode should be a valid enum value (0=single, 1=multi)
        let _ = attr.thread_mode as i32;
    }

    #[test]
    fn test_worker_query_max_am_header() {
        let config = context::Config::default();
        let params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &params).expect("context init");

        let worker_params = ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        // UCP_WORKER_ATTR_FIELD_MAX_AM_HEADER = 8
        let attr = worker.query(8).expect("worker query");
        // max_am_header should be non-negative
        let _ = attr.max_am_header;
    }

    #[test]
    fn test_worker_address_to_vec() {
        let config = context::Config::default();
        let params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &params).expect("context init");

        let worker_params = ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        let addr = worker.pack_address().expect("pack_address");
        let vec = addr.to_vec();
        assert!(!vec.is_empty());
    }

    // ── Worker address query tests ──

    #[test]
    fn test_worker_address_query() {
        let config = context::Config::default();
        let params = context::ParamsBuilder::new()
            .features(context::Flags::Tag)
            .build();
        let ctx = Context::new(&config, &params).expect("context init");

        let worker_params = ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");

        let addr = worker.pack_address().expect("pack_address");
        let uid_result = address_query(addr.handle);
        assert!(uid_result.is_ok(), "address_query should succeed");
        let uid = uid_result.unwrap();
        // UID should be a valid value (may be 0 in some UCX versions)
        let _ = uid;
    }

    // ── WorkerParams field mask tests ──

    #[test]
    fn test_worker_params_field_thread_mode_is_bit_0() {
        assert_eq!(
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_THREAD_MODE as u64,
            1
        );
    }

    #[test]
    fn test_worker_params_field_cpu_mask_is_bit_1() {
        assert_eq!(
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_CPU_MASK as u64,
            2
        );
    }

    #[test]
    fn test_worker_params_field_events_is_bit_2() {
        assert_eq!(
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_EVENTS as u64,
            4
        );
    }

    #[test]
    fn test_worker_params_field_user_data_is_bit_3() {
        assert_eq!(
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_USER_DATA as u64,
            8
        );
    }

    #[test]
    fn test_worker_params_field_event_fd_is_bit_4() {
        assert_eq!(
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_EVENT_FD as u64,
            16
        );
    }

    #[test]
    fn test_worker_params_field_flags_is_bit_5() {
        assert_eq!(
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_FLAGS as u64,
            32
        );
    }

    #[test]
    fn test_worker_params_field_name_is_bit_6() {
        assert_eq!(
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_NAME as u64,
            64
        );
    }

    #[test]
    fn test_worker_params_field_am_alignment_is_bit_7() {
        assert_eq!(
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_AM_ALIGNMENT as u64,
            128
        );
    }

    #[test]
    fn test_worker_params_field_client_id_is_bit_8() {
        assert_eq!(
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_CLIENT_ID as u64,
            256
        );
    }

    #[test]
    fn test_worker_params_fields_all_distinct() {
        let fields = [
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_THREAD_MODE as u64,
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_CPU_MASK as u64,
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_EVENTS as u64,
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_USER_DATA as u64,
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_EVENT_FD as u64,
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_FLAGS as u64,
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_NAME as u64,
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_AM_ALIGNMENT as u64,
            ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_CLIENT_ID as u64,
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
