use crate::ep::Ep;
use crate::ep::EpHandle;
use crate::ffi::*;

/// Re-exported for the return type of [`AmRecvCb`] (tracked in #73).
pub use crate::ffi::ucs_status_t;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Request;
use crate::RequestParam;
use crate::Status;
use bitflags::bitflags;
use std::sync::{Arc, Mutex};

/// Raw active-message receive callback. UCX passes the receive parameters as
/// `*const AmRecvParam`, valid only for the duration of the call. `AmRecvParam` is
/// `#[repr(transparent)]` over UCX's C struct, so this matches UCX's C callback type.
pub type AmRecvCb = unsafe extern "C" fn(
    arg: *mut ::std::os::raw::c_void,
    header: *const ::std::os::raw::c_void,
    header_length: usize,
    data: *mut ::std::os::raw::c_void,
    length: usize,
    param: *const AmRecvParam,
) -> ucs_status_t;

const AM_RECV_ATTR_FIELD_REPLY_EP: u64 = ucp_am_recv_attr_t::UCP_AM_RECV_ATTR_FIELD_REPLY_EP as u64;
const AM_RECV_ATTR_FLAG_DATA: u64 = ucp_am_recv_attr_t::UCP_AM_RECV_ATTR_FLAG_DATA as u64;
const AM_RECV_ATTR_FLAG_RNDV: u64 = ucp_am_recv_attr_t::UCP_AM_RECV_ATTR_FLAG_RNDV as u64;

bitflags! {
    /// Receive attributes UCX reports to an [`AmRecvCb`] through [`AmRecvParam::recv_attr`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct AmRecvAttr: u64 {
        /// `UCP_AM_RECV_ATTR_FIELD_REPLY_EP`: UCX provided a reply endpoint.
        const REPLY_EP = AM_RECV_ATTR_FIELD_REPLY_EP;
        /// `UCP_AM_RECV_ATTR_FLAG_DATA`: `data` is a descriptor the handler may keep.
        const DATA = AM_RECV_ATTR_FLAG_DATA;
        /// `UCP_AM_RECV_ATTR_FLAG_RNDV`: `data` is a rendezvous descriptor.
        const RNDV = AM_RECV_ATTR_FLAG_RNDV;
    }
}

/// Receive parameters UCX passes to an [`AmRecvCb`].
///
/// UCX owns this value. A callback receives it as `*const AmRecvParam`, valid only for the
/// duration of the call. `#[repr(transparent)]` gives it exactly the layout of UCX's C struct.
#[repr(transparent)]
pub struct AmRecvParam(ucp_am_recv_param_t);

// `AmRecvCb` receives `*const AmRecvParam` in place of UCX's `*const ucp_am_recv_param_t`.
const _: () = {
    assert!(std::mem::size_of::<AmRecvParam>() == std::mem::size_of::<ucp_am_recv_param_t>());
    assert!(std::mem::align_of::<AmRecvParam>() == std::mem::align_of::<ucp_am_recv_param_t>());
};

impl AmRecvParam {
    /// The receive attributes UCX reported for this message. Unknown bits are kept.
    pub fn recv_attr(&self) -> AmRecvAttr {
        AmRecvAttr::from_bits_retain(self.0.recv_attr)
    }

    /// The endpoint to reply on, if UCX provided one: [`AmRecvAttr::REPLY_EP`] is set and the
    /// handle is non-null. The handle is borrowed from UCX; never close it.
    pub fn reply_ep(&self) -> Option<EpHandle> {
        if self.recv_attr().contains(AmRecvAttr::REPLY_EP) && !self.0.reply_ep.is_null() {
            Some(EpHandle(self.0.reply_ep))
        } else {
            None
        }
    }
}

impl std::fmt::Debug for AmRecvParam {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AmRecvParam")
            .field("recv_attr", &self.recv_attr())
            .field("reply_ep", &self.reply_ep())
            .finish()
    }
}

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
    _param: *const AmRecvParam,
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
    ) -> Result<(), Status>
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
        // UCX has no unregister/unset operation for AM handlers. Keep replaced handlers
        // alive until worker destruction because UCX may still dispatch an in-flight
        // callback using the previous opaque argument (`arg` pointer).
        //
        // Cleanup strategy (Issue #100):
        // - When a new handler is registered, the old `AmHandler` Arc is pushed to
        //   `Worker::am_handlers` vector and retained.
        // - The old handler's memory remains valid because the UCX callback pointer
        //   still references it; dropping it prematurely would cause use-after-free.
        // - When the worker is destroyed via `ucp_worker_destroy`, UCX will eventually
        //   destroy all pending callbacks. By then, all Rust-side `AmHandler` instances
        //   are still alive and will be dropped safely when `Worker`'s Drop impl
        //   destroys the `am_handlers` vector.
        //
        // This ensures no dangling callbacks and no FFI leaks.
        self.am_handlers.push(handler);
        Ok(())
    }

    #[inline]
    /// Register a raw AM callback. The callback runs in the progress context:
    /// the thread calling `Worker::progress()`, or UCX-internal progress under
    /// MULTI. Do not block or call back into the same worker; hop heavy work to
    /// an application thread or channel. See `THREADING.md` section 4.
    pub fn am_register(&self, am_param: &HandlerParams) -> Result<(), Status> {
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
    ) -> Result<Option<Request>, Status> {
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

const AM_CB_FLAG_WHOLE_MSG: u32 = ucp_am_cb_flags::UCP_AM_FLAG_WHOLE_MSG as u32;
const AM_CB_FLAG_PERSISTENT_DATA: u32 = ucp_am_cb_flags::UCP_AM_FLAG_PERSISTENT_DATA as u32;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct CbFlags: u32 {
        const WholeMsg = AM_CB_FLAG_WHOLE_MSG;
        const PersistentData = AM_CB_FLAG_PERSISTENT_DATA;
    }
}

impl From<ucp_am_cb_flags> for CbFlags {
    fn from(flag: ucp_am_cb_flags) -> Self {
        CbFlags::from_bits_truncate(flag as u32)
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
        // SAFETY: `AmRecvCb` and UCX's `ucp_am_recv_callback_t` differ only in the pointee type of
        // the `param` pointer (`AmRecvParam` is `#[repr(transparent)]` over `ucp_am_recv_param_t`),
        // and pointers to sized types are ABI-compatible.
        params.cb =
            unsafe { std::mem::transmute::<Option<AmRecvCb>, ucp_am_recv_callback_t>(Some(cb)) };
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
    worker: &crate::worker::Worker,
    data_desc: *mut std::os::raw::c_void,
    buffer: *mut std::os::raw::c_void,
    count: usize,
) -> crate::Request {
    let ptr = ucp_am_recv_data_nbx(worker.handle, data_desc, buffer, count, std::ptr::null());
    crate::Request::from_raw(ptr)
}

/// Release active message data.
///
/// # Safety
/// Caller must ensure `data` was obtained from an AM receive handler.
#[deprecated(since = "0.1.0", note = "Use Worker::am_data_release() instead")]
pub unsafe fn am_data_release(worker: &crate::worker::Worker, data: *mut std::os::raw::c_void) {
    ucp_am_data_release(worker.handle, data);
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
    ) -> Result<Option<Request>, Status>;

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

    #[test]
    fn am_recv_param_reply_ep_requires_field_bit() {
        let fake = 0x40usize as ucp_ep_h;
        // SAFETY: the raw receive-param struct is plain C data; all-zero is valid.
        let mut raw: ucp_am_recv_param_t = unsafe { std::mem::zeroed() };
        raw.reply_ep = fake;
        let without_bit = AmRecvParam(raw);
        assert!(without_bit.recv_attr().is_empty());
        assert!(without_bit.reply_ep().is_none());

        raw.recv_attr = AmRecvAttr::REPLY_EP.bits() | AmRecvAttr::DATA.bits();
        let with_bit = AmRecvParam(raw);
        assert!(with_bit.recv_attr().contains(AmRecvAttr::DATA));
        assert_eq!(with_bit.reply_ep().map(|ep| ep.0), Some(fake));

        raw.reply_ep = std::ptr::null_mut();
        let null_ep = AmRecvParam(raw);
        assert!(null_ep.reply_ep().is_none());
    }

    unsafe extern "C" fn record_recv_param(
        arg: *mut std::os::raw::c_void,
        header: *const std::os::raw::c_void,
        header_length: usize,
        _data: *mut std::os::raw::c_void,
        _length: usize,
        param: *const AmRecvParam,
    ) -> ucs_status_t {
        let seen = &mut *(arg as *mut [u8; 2]);
        if header_length == 1 && !header.is_null() {
            seen[0] = *(header as *const u8);
        }
        seen[1] = match param.as_ref() {
            Some(param) if param.reply_ep().is_none() => 1,
            Some(_) => 2,
            None => 3,
        };
        ucs_status_t::UCS_OK
    }

    #[test]
    fn raw_callback_receives_native_recv_param() {
        let comms = crate::tests::setup_default();
        let mut seen = [0u8; 2];
        let handler = HandlerParamsBuilder::new()
            .id(24)
            .cb(record_recv_param)
            .arg(seen.as_mut_ptr() as *mut std::os::raw::c_void)
            .build();
        comms.worker.am_register(&handler).unwrap();
        let request_param = crate::RequestParamBuilder::new().no_imm_cmpl().build();
        if let Some(request) = comms.ep.am_send(24, b"q", b"", &request_param).unwrap() {
            assert!(comms.worker.wait_request(&request).unwrap());
        }
        for _ in 0..1000 {
            if seen[0] != 0 {
                break;
            }
            comms.worker.progress();
        }
        // No reply flag was sent, so UCX must not report a reply endpoint.
        assert_eq!(seen, [b'q', 1]);
    }
}
