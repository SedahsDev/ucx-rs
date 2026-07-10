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
