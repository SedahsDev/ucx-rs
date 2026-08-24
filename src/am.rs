use crate::ep::Ep;
use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Request;
use crate::RequestParam;
use bitflags::bitflags;
use std::sync::{Arc, Mutex};

pub type AmRecvCb = unsafe extern "C" fn(
    arg: *mut ::std::os::raw::c_void,
    header: *const ::std::os::raw::c_void,
    header_length: usize,
    data: *mut ::std::os::raw::c_void,
    length: usize,
    param: *const ucp_am_recv_param_t,
) -> ucs_status_t;

type AmCallback = Box<dyn FnMut(&[u8], &[u8]) -> ucs_status_t + Send + 'static>;

/// The Rust state retained by [`Worker::am_register_handler`].
pub struct AmHandler {
    inner: Mutex<AmCallback>,
}

impl std::fmt::Debug for AmHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AmHandler").finish_non_exhaustive()
    }
}

unsafe extern "C" fn am_trampoline(
    arg: *mut std::os::raw::c_void,
    header: *const std::os::raw::c_void,
    header_length: usize,
    data: *mut std::os::raw::c_void,
    length: usize,
    _param: *const ucp_am_recv_param_t,
) -> ucs_status_t {
    // SAFETY: `arg` is an Arc<AmHandler> pointer installed by
    // am_register_handler and retained by Worker until after UCX destroys the
    // worker. UCX owns the callback buffers for this invocation; null pointers
    // are permitted for zero-length messages.
    let handler = unsafe { &*(arg as *const AmHandler) };
    let header = if header.is_null() && header_length == 0 {
        &[]
    } else if header.is_null() {
        return ucs_status_t::UCS_ERR_INVALID_PARAM;
    } else {
        unsafe { std::slice::from_raw_parts(header as *const u8, header_length) }
    };
    let data = if data.is_null() && length == 0 {
        &[]
    } else if data.is_null() {
        return ucs_status_t::UCS_ERR_INVALID_PARAM;
    } else {
        unsafe { std::slice::from_raw_parts(data as *const u8, length) }
    };
    let mut callback = match handler.inner.lock() {
        Ok(callback) => callback,
        // A panic poisons the mutex. Do not invoke a possibly inconsistent
        // handler again; report the callback failure to UCX instead.
        Err(_) => return ucs_status_t::UCS_ERR_IO_ERROR,
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(header, data))) {
        Ok(status) => status,
        // Handler panics are contained here and reported as a UCX error; they
        // never unwind through this extern "C" trampoline into UCX.
        Err(_) => ucs_status_t::UCS_ERR_IO_ERROR,
    }
}

impl Worker {
    /// Register a safe AM receive callback. Only one callback is retained by
    /// this wrapper for a worker; registering again replaces the Rust closure.
    /// UCX invokes it in the progress context, so it must not block or call
    /// back into this worker; send heavy work to an application channel.
    pub fn am_register_handler<F>(
        &mut self,
        id: u32,
        flags: CbFlags,
        handler: F,
    ) -> Result<(), ucs_status_t>
    where
        F: FnMut(&[u8], &[u8]) -> ucs_status_t + Send + 'static,
    {
        let handler = Arc::new(AmHandler {
            inner: Mutex::new(Box::new(handler)),
        });
        let params = HandlerParamsBuilder::new()
            .id(id)
            .flags(flags)
            .cb(am_trampoline)
            .arg(Arc::as_ptr(&handler) as *mut std::ffi::c_void)
            .build();
        status_to_result(unsafe { ucp_worker_set_am_recv_handler(self.handle, &params.handle) })?;
        // UCX has no unregister operation. Keep replaced handlers alive until
        // worker destruction because UCX may still dispatch an in-flight
        // callback using the previous opaque argument.
        self.am_handlers.push(handler);
        Ok(())
    }

    #[inline]
    /// Register a raw AM callback. The callback runs in the progress context:
    /// the thread calling `Worker::progress()`, or UCX-internal progress under
    /// MULTI. Do not block or call back into the same worker; hop heavy work to
    /// an application thread or channel. See `THREADING.md` section 4.
    pub fn am_register(&self, am_param: &HandlerParams) -> Result<(), ucs_status_t> {
        status_to_result(unsafe { ucp_worker_set_am_recv_handler(self.handle, &am_param.handle) })
    }
}

impl Ep {
    #[inline]
    pub fn am_send(
        &self,
        id: u32,
        header: &[u8],
        data: &[u8],
        params: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_am_send_nbx(
                self.handle,
                id,
                header.as_ptr() as _,
                header.len(),
                data.as_ptr() as _,
                data.len(),
                &params.handle,
            )
        })
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct CbFlags: u32 {
        const WholeMsg = ucp_am_cb_flags::UCP_AM_FLAG_WHOLE_MSG as u32;
    const PersistentData = ucp_am_cb_flags::UCP_AM_FLAG_PERSISTENT_DATA as u32;
    }
}

#[derive(Debug, Clone)]
pub struct HandlerParamsBuilder {
    uninit_handle: std::mem::MaybeUninit<ucp_am_handler_param_t>,
    flags: u64,
}

impl Default for HandlerParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl HandlerParamsBuilder {
    #[inline]
    pub fn new() -> HandlerParamsBuilder {
        // SAFETY: UCX parameter structs are valid when zeroed; the field mask controls reads.
        let uninit_params = std::mem::MaybeUninit::new(unsafe { std::mem::zeroed() });
        HandlerParamsBuilder {
            uninit_handle: uninit_params,
            flags: 0,
        }
    }

    #[inline]
    pub fn id(&mut self, id: u32) -> &mut HandlerParamsBuilder {
        self.flags |= ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ID as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.id = id;
        self
    }

    #[inline]
    pub fn flags(&mut self, flags: CbFlags) -> &mut HandlerParamsBuilder {
        self.flags |= ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_FLAGS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.flags = flags.bits();
        self
    }

    #[inline]
    pub fn cb(&mut self, cb: AmRecvCb) -> &mut HandlerParamsBuilder {
        self.flags |= ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_CB as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.cb = Some(cb);
        self
    }

    #[inline]
    pub fn arg(&mut self, arg: *mut std::os::raw::c_void) -> &mut HandlerParamsBuilder {
        self.flags |= ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ARG as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.arg = arg;
        self
    }

    #[inline]
    pub fn build(&mut self) -> HandlerParams {
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.field_mask = self.flags;

        HandlerParams {
            handle: unsafe { self.uninit_handle.assume_init() },
        }
    }
}

pub struct HandlerParams {
    pub(crate) handle: ucp_am_handler_param_t,
}

/// Receive active message data.
///
/// # Safety
/// Caller must ensure `data_desc` is a valid data descriptor from the AM handler.
#[deprecated(since = "0.1.0", note = "Use Worker::am_recv_data instead")]
pub unsafe fn am_recv_data_nbx(
    worker: ucp_worker_h,
    data_desc: *mut std::os::raw::c_void,
    buffer: *mut std::os::raw::c_void,
    count: usize,
) -> crate::Request {
    let ptr = ucp_am_recv_data_nbx(worker, data_desc, buffer, count, std::ptr::null());
    crate::Request::from_raw(ptr)
}

/// Release active message data.
///
/// # Safety
/// Caller must ensure `data` was obtained from an AM receive handler.
#[deprecated(since = "0.1.0", note = "Use Worker::am_data_release() instead")]
pub unsafe fn am_data_release(worker: ucp_worker_h, data: *mut std::os::raw::c_void) {
    ucp_am_data_release(worker, data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Config, Context, Flags, ParamsBuilder as ContextParamsBuilder};
    use crate::ep::ParamsBuilder as EpParamsBuilder;
    use crate::worker::{ParamsBuilder as WorkerParamsBuilder, RemoteWorkerAddress};
    use std::sync::atomic::{AtomicU32, Ordering};

    type AmRecvDataFn = fn(
        &Worker,
        std::ptr::NonNull<std::ffi::c_void>,
        &mut [u8],
        &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t>;

    #[test]
    fn test_worker_am_receive_api_signatures() {
        let _recv: AmRecvDataFn = Worker::am_recv_data;
        let _release: fn(&Worker, std::ptr::NonNull<std::ffi::c_void>) = Worker::am_data_release;
    }

    #[test]
    fn safe_handler_receives_self_am() {
        let context_params = ContextParamsBuilder::new()
            .features(Flags::Am)
            .mt_workers_shared(1)
            .build();
        let mut context = Context::new(&Config::read("", "").unwrap(), &context_params).unwrap();
        let worker_params = WorkerParamsBuilder::new().build();
        let mut worker = context.worker_create(&worker_params).unwrap();
        let packed = worker.pack_address().unwrap();
        let address = RemoteWorkerAddress::new(packed.to_vec());
        drop(packed);
        let endpoint = worker
            .create_ep(EpParamsBuilder::new().address(&address).build())
            .unwrap();
        let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let seen = Arc::clone(&count);
        worker
            .am_register_handler(23, CbFlags::WholeMsg, move |header, data| {
                assert_eq!(header, b"h");
                assert_eq!(data, b"d");
                seen.fetch_add(1, Ordering::Relaxed);
                ucs_status_t::UCS_OK
            })
            .unwrap();
        let request_param = crate::RequestParamBuilder::new().no_imm_cmpl().build();
        if let Some(request) = endpoint.am_send(23, b"h", b"d", &request_param).unwrap() {
            assert!(worker.wait_request(&request).unwrap());
        }
        for _ in 0..1000 {
            worker.progress();
            if count.load(Ordering::Relaxed) == 1 {
                break;
            }
        }
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }
}
