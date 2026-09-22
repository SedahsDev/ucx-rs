use crate::context::Context;
use crate::ep;
use crate::ep::Ep;
use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::Status;
use crate::Request;
use crate::RequestParam;
use crate::worker::address::*;
use crate::worker::params::*;
use crate::RequestParamBuilder;
use bitflags::bitflags;
use std::ffi::CString;
use std::ptr::NonNull;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[cfg(debug_assertions)]
struct ProgressGuard<'a> {
    progressing: &'a AtomicBool,
}

#[cfg(debug_assertions)]
impl<'a> ProgressGuard<'a> {
    fn new(progressing: &'a AtomicBool) -> Self {
        if progressing
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_err()
        {
            panic!("concurrent progress() on the same worker");
        }
        Self { progressing }
    }
}

#[cfg(debug_assertions)]
impl Drop for ProgressGuard<'_> {
    fn drop(&mut self) {
        self.progressing
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

/// UCX worker ownership wrapper.
///
/// This type is intentionally not `Clone`: a worker handle has one owner and
/// is destroyed on drop.
#[derive(Debug)]
pub struct Worker {
    pub(crate) handle: ucp_worker_h,
    pub(crate) alive: Arc<AtomicBool>,
    pub(crate) am_handlers: Vec<std::sync::Arc<crate::am::AmHandler>>,
    #[cfg(debug_assertions)]
    pub(crate) progressing: AtomicBool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Config, Context, Flags, ParamsBuilder as ContextParamsBuilder};

    fn setup_worker() -> (Context, Worker) {
        let context_params = ContextParamsBuilder::new()
            .features(Flags::Tag)
            .mt_workers_shared(1)
            .build();
        let mut context =
            Context::new(&Config::read("", "").expect("config read"), &context_params)
                .expect("context create");
        let mut worker_params = ParamsBuilder::new();
        worker_params.thread_mode(ucs_thread_mode_t::UCS_THREAD_MODE_SERIALIZED);
        let worker_params = worker_params.build();
        let worker = context
            .worker_create(&worker_params)
            .expect("worker create");
        (context, worker)
    }

    #[test]
    fn progress_works_with_a_real_worker() {
        let (_context, worker) = setup_worker();
        let _ = worker.progress();
    }

    #[test]
    fn single_mode_worker_cannot_be_wrapped() {
        let context_params = ContextParamsBuilder::new()
            .features(Flags::Tag)
            .mt_workers_shared(1)
            .build();
        let mut context =
            Context::new(&Config::read("", "").expect("config read"), &context_params)
                .expect("context create");
        let mut params = ParamsBuilder::new();
        params.thread_mode(ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE);
        let worker = context
            .worker_create(&params.build())
            .expect("worker create");
        assert!(matches!(
            MtWorker::new(worker),
            Err(Status(ucs_status_t::UCS_ERR_INVALID_PARAM))
        ));
    }

    #[test]
    fn serialized_mode_worker_can_progress_through_wrapper() {
        let (_context, worker) = setup_worker();
        let worker = MtWorker::new(worker).expect("UCX should grant a threaded worker");
        let _ = worker.progress();
        let _clone = worker.clone();
    }
}

/// A cloneable, thread-safe handle to a worker configured for serialized or
/// multi-threaded access.
///
/// Construction queries the mode UCX actually granted. Every operation takes
/// the internal mutex for its duration. Cloning this value clones the handle,
/// not the underlying UCX worker; all clones access the same worker.
///
/// Endpoints returned by [`MtWorker::create_ep`] remain `!Send` and `!Sync`.
/// Keep them on the constructing thread, or serialize their use yourself.
/// Fetch-AMO operations whose reply buffer is borrowed are not cross-thread
/// compatible; an owned-buffer API is future work. Per-operation locking may
/// also negate the parallelism benefit of `UCS_THREAD_MODE_MULTI` and should
/// be measured before relying on it for performance.
pub struct MtWorker {
    inner: Arc<MtWorkerInner>,
}

struct MtWorkerInner {
    worker: std::sync::Mutex<Worker>,
}

impl Clone for MtWorker {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl std::fmt::Debug for MtWorker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("MtWorker").finish_non_exhaustive()
    }
}

// SAFETY: `new` admits only workers whose UCX-granted mode is SERIALIZED or
// MULTI, and every operation below locks the worker mutex before accessing it.
unsafe impl Send for MtWorker {}
// SAFETY: See the `Send` implementation. Shared handles cannot access the
// contained worker without taking the same mutex. However, `create_ep` returns
// an `Ep` that remains `!Send` and `!Sync`, so this mutex does not make endpoints
// derived from the `MtWorker` thread-safe; callers must serialize their use.
unsafe impl Sync for MtWorker {}

impl MtWorker {
    /// Wrap a worker only when UCX reports a thread-safe worker mode.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new(worker: Worker) -> Result<Self, Status> {
        let mode = worker
            .query(WorkerAttrFields::THREAD_MODE)?
            .thread_mode
            .ok_or(ucs_status_t::UCS_ERR_INVALID_PARAM)?;
        if mode != ucs_thread_mode_t::UCS_THREAD_MODE_SERIALIZED
            && mode != ucs_thread_mode_t::UCS_THREAD_MODE_MULTI
        {
            return Err(Status(ucs_status_t::UCS_ERR_INVALID_PARAM));
        }
        Ok(Self {
            inner: Arc::new(MtWorkerInner {
                worker: std::sync::Mutex::new(worker),
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Worker> {
        self.inner
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Run `operation` against the underlying worker while holding the
    /// serialization lock.
    ///
    /// This is the escape hatch for entry points that take a `&Worker` directly,
    /// such as the fetch-AMO family (`Ep::amo_fadd32` and friends) whose
    /// borrowed reply buffer has no owned-buffer equivalent yet. The lock is
    /// held for the whole call.
    ///
    /// `operation` must not re-enter this `MtWorker` (for example by calling
    /// [`MtWorker::progress`] or [`MtWorker::wait_request`]): the internal mutex
    /// is not reentrant and the call will deadlock.
    pub fn with_worker<F, T>(&self, operation: F) -> T
    where
        F: FnOnce(&Worker) -> T,
    {
        operation(&self.lock())
    }

    pub fn progress(&self) -> bool {
        self.lock().progress()
    }

    pub fn flush(&self, params: &RequestParam) -> Result<Option<Request>, Status> {
        self.lock().flush(params)
    }

    /// Apply worker-wide UCX ordering to operations issued before this call.
    pub fn fence(&self) -> Result<(), Status> {
        self.lock().fence()
    }

    pub fn wait(&self) -> Result<(), Status> {
        self.lock().wait()
    }
    pub fn arm(&self) -> Result<(), Status> {
        self.lock().arm()
    }
    pub fn signal(&self) -> Result<(), Status> {
        self.lock().signal()
    }
    pub fn get_efd(&self) -> Result<i32, Status> {
        self.lock().get_efd()
    }
    pub fn cancel_request(&self, request: &mut Request) {
        self.lock().cancel_request(request)
    }

    pub fn wait_request(&self, request: &Request) -> Result<bool, Status> {
        const MAX_ROUNDS: usize = 1_000_000;
        for _ in 0..MAX_ROUNDS {
            match request.check_finished() {
                Ok(true) => return Ok(true),
                Ok(false) => {
                    self.progress();
                }
                Err(error) => return Err(error),
            }
        }
        Ok(false)
    }

    pub fn create_ep(&self, ep_params: ep::Params) -> Result<Ep, Status> {
        self.lock().create_ep(ep_params)
    }

    pub fn pack_address(&self) -> Result<Vec<u8>, Status> {
        self.lock().pack_address().map(|address| address.to_vec())
    }
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
    pub(crate) fn new(context: &mut Context, params: &Params) -> Result<Worker, Status> {
        let mut worker: ucp_worker_h = std::ptr::null_mut();

        let result = status_to_result(unsafe {
            ucp_worker_create(context.handle, &params.handle, &mut worker)
        });
        match result {
            Ok(()) => Ok(Worker {
                handle: worker,
                alive: Arc::new(AtomicBool::new(true)),
                am_handlers: Vec::new(),
                #[cfg(debug_assertions)]
                progressing: AtomicBool::new(false),
            }),
            Err(ucs_status_t) => Err(ucs_status_t),
        }
    }

    pub fn pack_address(&self) -> Result<WorkerAddress<'_>, Status> {
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
    /// Make one non-blocking progress attempt on this worker.
    ///
    /// In a UCX build with multi-thread support, concurrent calls are
    /// safe when this worker uses `UCS_THREAD_MODE_MULTI`, but they contend on
    /// UCX's internal pthread spinlock. Losing callers busy-wait and perform
    /// atomic read-modify-writes on the shared lock word, causing cache-line
    /// thrashing. In a non-MT build or with `UCS_THREAD_MODE_SINGLE`, concurrent
    /// calls are undefined behavior. See [`THREADING.md` section 2.1][threading].
    ///
    /// Prefer one thread owning each worker's progress loop. To wake that loop
    /// from other threads, use [`Self::arm`] and [`Self::get_efd`] rather than
    /// polling from multiple callers. Debug builds panic if this method is
    /// re-entered concurrently on the same worker.
    ///
    /// [threading]: https://github.com/SedahsDev/ucx-rs/blob/master/THREADING.md#21-progress-under-multi-is-a-spinlock-not-a-free-for-all
    pub fn progress(&self) -> bool {
        #[cfg(debug_assertions)]
        let _progress_guard = ProgressGuard::new(&self.progressing);
        let progress = unsafe { ucp_worker_progress(self.handle) };
        progress > 0
    }

    /// Print worker diagnostics to `fd`. Invalid descriptors are ignored.
    pub fn print_info(&self, fd: std::os::fd::RawFd) {
        let _ = crate::config::with_file_stream(fd, |stream| {
            // SAFETY: self owns a live worker and stream is valid for this call.
            unsafe { ucp_worker_print_info(self.handle, stream.cast()) };
        });
    }

    /// Block until `request` completes, progressing this worker.
    ///
    /// This method repeatedly calls [`Self::progress`], so the same
    /// safe-but-contended/undefined-behavior distinction applies: it is safe
    /// but contended under `UCS_THREAD_MODE_MULTI` in an MT-enabled UCX build,
    /// and undefined otherwise when another caller drives the worker. See
    /// [`THREADING.md` section 2.1][threading]. Keep one progress owner per
    /// worker; use [`Self::arm`] and [`Self::get_efd`] for wakeups.
    /// Returns `Ok(false)` after a bounded spin; use the efd/arm/wait APIs for
    /// a real blocking wait.
    ///
    /// ```no_run
    /// # let worker: &ucx_sys::worker::Worker = todo!();
    /// # let request: ucx_sys::Request = todo!();
    /// let completed = worker.wait_request(&request).unwrap();
    /// assert!(completed);
    /// ```
    ///
    /// [threading]: https://github.com/SedahsDev/ucx-rs/blob/master/THREADING.md#21-progress-under-multi-is-a-spinlock-not-a-free-for-all
    pub fn wait_request(&self, request: &Request) -> Result<bool, Status> {
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

    pub fn create_ep(&self, ep_params: ep::Params) -> Result<Ep, Status> {
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

    /// Flush outstanding operations on this worker.
    ///
    /// UCX may make progress while servicing this call. Concurrent calls on a
    /// `UCS_THREAD_MODE_MULTI` worker are safe but contended in an MT-enabled
    /// UCX build; in a non-MT build or `UCS_THREAD_MODE_SINGLE`, concurrent
    /// access is undefined behavior. See [`THREADING.md` section 2.1][threading].
    /// Prefer a single owner for the worker's progress loop and use
    /// [`Self::arm`] plus [`Self::get_efd`] to wake it from other threads.
    ///
    /// [threading]: https://github.com/SedahsDev/ucx-rs/blob/master/THREADING.md#21-progress-under-multi-is-a-spinlock-not-a-free-for-all
    pub fn flush(&self, params: &RequestParam) -> Result<Option<Request>, Status> {
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
    ) -> Result<Option<Request>, Status> {
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
    pub fn fence(&self) -> Result<(), Status> {
        crate::status_to_result(unsafe { ucp_worker_fence(self.handle) })
    }

    /// Arm the worker for asynchronous completion.
    ///
    /// In the single-threaded model, progress the worker, arm it, then poll or
    /// epoll [`Self::get_efd`] and call [`Self::wait`]. Repeat after wakeup.
    pub fn arm(&self) -> Result<(), Status> {
        crate::status_to_result(unsafe { ucp_worker_arm(self.handle) })
    }

    /// Wait for an asynchronous event on the worker.
    ///
    /// Pair this with [`Self::arm`] and a progress loop; it blocks for a UCX
    /// event but does not perform worker progress itself.
    pub fn wait(&self) -> Result<(), Status> {
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
    pub fn signal(&self) -> Result<(), Status> {
        // SAFETY: self.handle is a live worker handle.
        crate::status_to_result(unsafe { ucp_worker_signal(self.handle) })
    }

    /// Get the event file descriptor for the worker.
    ///
    /// Poll or epoll this fd after [`Self::arm`], then call [`Self::wait`] and
    /// resume the single-threaded progress loop when it becomes readable.
    pub fn get_efd(&self) -> Result<i32, Status> {
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
    pub fn query(&self, mask: WorkerAttrFields) -> Result<WorkerAttr, Status> {
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

pub struct CpuSet(pub(crate) ucs_cpu_set_t);

impl std::fmt::Debug for CpuSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("CpuSet").finish_non_exhaustive()
    }
}

