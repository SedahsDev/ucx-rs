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
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// UCX worker ownership wrapper.
///
/// This type is intentionally not `Clone`: a worker handle has one owner and
/// is destroyed on drop.
#[derive(Debug)]
pub struct Worker {
    pub(crate) handle: ucp_worker_h,
    pub(crate) alive: Arc<AtomicBool>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.alive
            .store(false, std::sync::atomic::Ordering::Release);
        let params = RequestParamBuilder::new().build();
        // Drop is best-effort: never panic during drop glue, and do not wait
        // indefinitely for a UCX request which cannot make progress.
        match self.flush(&params) {
            Ok(Some(request)) => {
                const MAX_FLUSH_PROGRESS_ROUNDS: usize = 1_000_000;
                for _ in 0..MAX_FLUSH_PROGRESS_ROUNDS {
                    match request.check_finished() {
                        Ok(true) | Err(_) => break,
                        Ok(false) => {
                            self.progress();
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(error) => eprintln!("ucx-sys: worker flush during Drop failed: {error:?}"),
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
            Ok(()) => Ok(Worker {
                handle: worker,
                alive: Arc::new(AtomicBool::new(true)),
            }),
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

    /// Block until `request` completes, progressing this worker.
    /// Returns `Ok(false)` after a bounded spin; use the efd/arm/wait APIs for
    /// a real blocking wait.
    ///
    /// ```no_run
    /// # let worker: &ucx_sys::worker::Worker = todo!();
    /// # let request: ucx_sys::Request = todo!();
    /// let completed = worker.wait_request(&request).unwrap();
    /// assert!(completed);
    /// ```
    pub fn wait_request(&self, request: &Request) -> Result<bool, ucs_status_t> {
        const MAX_ROUNDS: usize = 1_000_000;
        for _ in 0..MAX_ROUNDS {
            match request.check_finished() {
                Ok(true) => return Ok(true),
                Ok(false) => {
                    self.progress();
                }
                Err(e) => return Err(e),
            }
        }
        Ok(false)
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

    /// Receive the data described by an active-message callback descriptor.
    ///
    /// UCX takes ownership of `data_desc` after this call, including when the
    /// operation fails. The returned request, when present, owns its UCX
    /// request handle and can be polled with [`Request::check_finished`].
    pub fn am_recv_data(
        &self,
        data_desc: NonNull<std::ffi::c_void>,
        buffer: &mut [u8],
        params: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_am_recv_data_nbx(
                self.handle,
                data_desc.as_ptr(),
                buffer.as_mut_ptr() as _,
                buffer.len(),
                &params.handle,
            )
        })
    }

    /// Release active-message data retained after an AM callback.
    pub fn am_data_release(&self, data: NonNull<std::ffi::c_void>) {
        unsafe { ucp_am_data_release(self.handle, data.as_ptr()) }
    }

    /// Worker fence — ensures ordering of operations.
    pub fn fence(&self) -> Result<(), ucs_status_t> {
        crate::status_to_result(unsafe { ucp_worker_fence(self.handle) })
    }

    /// Arm the worker for asynchronous completion.
    ///
    /// In the single-threaded model, progress the worker, arm it, then poll or
    /// epoll [`Self::get_efd`] and call [`Self::wait`]. Repeat after wakeup.
    pub fn arm(&self) -> Result<(), ucs_status_t> {
        crate::status_to_result(unsafe { ucp_worker_arm(self.handle) })
    }

    /// Wait for an asynchronous event on the worker.
    ///
    /// Pair this with [`Self::arm`] and a progress loop; it blocks for a UCX
    /// event but does not perform worker progress itself.
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

    /// Signal the worker to wake up from [`Self::wait`].
    pub fn signal(&self) -> Result<(), ucs_status_t> {
        // SAFETY: self.handle is a live worker handle.
        crate::status_to_result(unsafe { ucp_worker_signal(self.handle) })
    }

    /// Get the event file descriptor for the worker.
    ///
    /// Poll or epoll this fd after [`Self::arm`], then call [`Self::wait`] and
    /// resume the single-threaded progress loop when it becomes readable.
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
    pub fn query(&self, mask: WorkerAttrFields) -> Result<WorkerAttr, ucs_status_t> {
        // SAFETY: UCX fills only fields selected by the documented mask.
        let mut attr: ucp_worker_attr = unsafe { std::mem::zeroed() };
        attr.field_mask = mask.bits();
        // SAFETY: self.handle is a live worker and attr is valid for UCX to fill.
        crate::status_to_result(unsafe { ucp_worker_query(self.handle, &mut attr) }).map(|()| {
            let name = if mask.contains(WorkerAttrFields::NAME) {
                // SAFETY: UCX documents NAME as a NUL-terminated fixed-size array.
                Some(
                    unsafe { std::ffi::CStr::from_ptr(attr.name.as_ptr()) }
                        .to_string_lossy()
                        .into_owned(),
                )
            } else {
                None
            };
            WorkerAttr {
                thread_mode: mask
                    .contains(WorkerAttrFields::THREAD_MODE)
                    .then_some(attr.thread_mode),
                address: mask
                    .contains(WorkerAttrFields::ADDRESS)
                    .then_some(WorkerAddressAttr {
                        address: attr.address,
                        length: attr.address_length,
                    }),
                address_flags: mask
                    .contains(WorkerAttrFields::ADDRESS_FLAGS)
                    .then_some(attr.address_flags),
                max_am_header: mask
                    .contains(WorkerAttrFields::MAX_AM_HEADER)
                    .then_some(attr.max_am_header),
                name,
                max_info_string: mask
                    .contains(WorkerAttrFields::MAX_INFO_STRING)
                    .then_some(attr.max_debug_string),
            }
        })
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct WorkerAttrFields: u64 {
        const THREAD_MODE = 1 << 0;
        const ADDRESS = 1 << 1;
        const ADDRESS_FLAGS = 1 << 2;
        const MAX_AM_HEADER = 1 << 3;
        const NAME = 1 << 4;
        const MAX_INFO_STRING = 1 << 5;
    }
}

#[derive(Debug, Clone)]
pub struct WorkerAddressAttr {
    pub address: *mut ucp_address_t,
    pub length: usize,
}

/// Worker query attribute result. Fields are present only when requested.
#[derive(Debug, Clone)]
pub struct WorkerAttr {
    pub thread_mode: Option<ucs_thread_mode_t>,
    pub address: Option<WorkerAddressAttr>,
    pub address_flags: Option<u32>,
    pub max_am_header: Option<usize>,
    pub name: Option<String>,
    pub max_info_string: Option<usize>,
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
        // SAFETY: UCX parameter structs are valid when zeroed; the field mask controls reads.
        let uninit_params = std::mem::MaybeUninit::new(unsafe { std::mem::zeroed() });
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

    pub fn name(&mut self, name: &str) -> Result<&mut ParamsBuilder, std::ffi::NulError> {
        let name_cs = CString::new(name)?;
        self.field_mask |= ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_NAME as u64;
        self.name = Some(name_cs);
        Ok(self)
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

        if let Some(new_name) = self.name.take() {
            ucp_param.handle.name = new_name.as_ptr();
            ucp_param.name = Some(new_name);
        }

        ucp_param
    }
}

#[derive(Debug)]
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

#[derive(Debug)]
pub struct Params {
    pub(crate) handle: ucp_worker_params_t,
    name: Option<CString>,
}
