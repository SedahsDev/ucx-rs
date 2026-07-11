use crate::ep::Ep;
use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Request;
use crate::RequestParam;
use bitflags::bitflags;

type AmRecvCb = unsafe extern "C" fn(
    arg: *mut ::std::os::raw::c_void,
    header: *const ::std::os::raw::c_void,
    header_length: usize,
    data: *mut ::std::os::raw::c_void,
    length: usize,
    param: *const ucp_am_recv_param_t,
) -> ucs_status_t;

impl Worker {
    #[inline]
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
        let uninit_params = std::mem::MaybeUninit::<ucp_am_handler_param_t>::uninit();
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
#[deprecated(
    since = "0.1.0",
    note = "Use a safe wrapper around AM receive data instead"
)]
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
    use std::os::raw::c_void;

    #[test]
    fn test_cbflags_empty() {
        let flags = CbFlags::empty();
        assert!(flags.is_empty());
        assert!(!flags.contains(CbFlags::WholeMsg));
        assert!(!flags.contains(CbFlags::PersistentData));
    }

    #[test]
    fn test_cbflags_whole_msg() {
        let flags = CbFlags::WholeMsg;
        assert!(flags.contains(CbFlags::WholeMsg));
        assert!(!flags.contains(CbFlags::PersistentData));
        assert!(!flags.is_empty());
        assert_eq!(flags.bits(), 1u32);
    }

    #[test]
    fn test_cbflags_persistent_data() {
        let flags = CbFlags::PersistentData;
        assert!(!flags.contains(CbFlags::WholeMsg));
        assert!(flags.contains(CbFlags::PersistentData));
        assert!(!flags.is_empty());
        assert_eq!(flags.bits(), 2u32);
    }

    #[test]
    fn test_cbflags_all() {
        let flags = CbFlags::all();
        assert!(flags.contains(CbFlags::WholeMsg));
        assert!(flags.contains(CbFlags::PersistentData));
        assert!(!flags.is_empty());
        assert_eq!(flags.bits(), 3u32);
    }

    #[test]
    fn test_cbflags_insert_remove() {
        let mut flags = CbFlags::empty();
        assert!(flags.is_empty());
        flags.insert(CbFlags::WholeMsg);
        assert!(flags.contains(CbFlags::WholeMsg));
        assert_eq!(flags.bits(), 1u32);
        flags.remove(CbFlags::WholeMsg);
        assert!(flags.is_empty());
        assert_eq!(flags.bits(), 0u32);
    }

    #[test]
    fn test_cbflags_set() {
        let mut flags = CbFlags::empty();
        flags.set(CbFlags::WholeMsg, true);
        assert!(flags.contains(CbFlags::WholeMsg));
        flags.set(CbFlags::WholeMsg, false);
        assert!(!flags.contains(CbFlags::WholeMsg));
    }

    #[test]
    fn test_cbflags_intersection() {
        let a = CbFlags::WholeMsg | CbFlags::PersistentData;
        let b = CbFlags::WholeMsg;
        let intersection = a.intersection(b);
        assert_eq!(intersection, CbFlags::WholeMsg);
    }

    #[test]
    fn test_cbflags_union() {
        let a = CbFlags::WholeMsg;
        let b = CbFlags::PersistentData;
        let union = a.union(b);
        assert_eq!(union, CbFlags::all());
    }

    #[test]
    fn test_cbflags_equality() {
        let a = CbFlags::WholeMsg;
        let b = CbFlags::WholeMsg;
        assert_eq!(a, b);
    }

    #[test]
    fn test_cbflags_clone_copy() {
        let a = CbFlags::all();
        let b = a.clone();
        assert_eq!(a, b);
        let c = a;
        assert_eq!(a, c);
    }

    #[test]
    fn test_cbflags_debug() {
        let flags = CbFlags::WholeMsg;
        let debug_str = format!("{:?}", flags);
        assert!(!debug_str.is_empty());
    }

    extern "C" fn dummy_am_cb(
        _arg: *mut c_void,
        _header: *const c_void,
        _header_length: usize,
        _data: *mut c_void,
        _length: usize,
        _param: *const ucp_am_recv_param_t,
    ) -> ucs_status_t {
        ucs_status_t::UCS_OK
    }

    extern "C" fn am_cb_return_error(
        _arg: *mut c_void,
        _header: *const c_void,
        _header_length: usize,
        _data: *mut c_void,
        _length: usize,
        _param: *const ucp_am_recv_param_t,
    ) -> ucs_status_t {
        ucs_status_t::UCS_ERR_INVALID_PARAM
    }

    #[test]
    fn test_amrecvcb_signature_ok() {
        let result = unsafe {
            dummy_am_cb(std::ptr::null_mut(), std::ptr::null(), 0, std::ptr::null_mut(), 0, std::ptr::null())
        };
        assert_eq!(result, ucs_status_t::UCS_OK);
    }

    #[test]
    fn test_amrecvcb_signature_error() {
        let result = unsafe {
            am_cb_return_error(std::ptr::null_mut(), std::ptr::null(), 0, std::ptr::null_mut(), 0, std::ptr::null())
        };
        assert_eq!(result, ucs_status_t::UCS_ERR_INVALID_PARAM);
    }

    #[test]
    fn test_amrecvcb_callback_type_alias() {
        let cb: AmRecvCb = dummy_am_cb;
        let result = unsafe {
            cb(std::ptr::null_mut(), std::ptr::null(), 0, std::ptr::null_mut(), 0, std::ptr::null())
        };
        assert_eq!(result, ucs_status_t::UCS_OK);
    }

    #[test]
    fn test_handler_params_builder_new() {
        let builder = HandlerParamsBuilder::new();
        assert_eq!(builder.flags, 0u64);
    }

    #[test]
    fn test_handler_params_builder_default() {
        let builder = HandlerParamsBuilder::default();
        assert_eq!(builder.flags, 0u64);
    }

    #[test]
    fn test_handler_params_builder_build_empty() {
        let mut builder = HandlerParamsBuilder::new();
        let params = builder.build();
        assert_eq!(params.handle.field_mask, 0u64);
    }

    #[test]
    fn test_handler_params_builder_id() {
        let mut builder = HandlerParamsBuilder::new();
        builder.id(42);
        let params = builder.build();
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ID as u64) != 0);
        assert_eq!(params.handle.id, 42);
    }

    #[test]
    fn test_handler_params_builder_id_multiple() {
        let mut builder = HandlerParamsBuilder::new();
        builder.id(10);
        builder.id(20);
        let params = builder.build();
        assert_eq!(params.handle.id, 20);
    }

    #[test]
    fn test_handler_params_builder_flags() {
        let mut builder = HandlerParamsBuilder::new();
        builder.flags(CbFlags::WholeMsg);
        let params = builder.build();
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_FLAGS as u64) != 0);
        assert_eq!(params.handle.flags, CbFlags::WholeMsg.bits());
    }

    #[test]
    fn test_handler_params_builder_flags_all() {
        let mut builder = HandlerParamsBuilder::new();
        builder.flags(CbFlags::all());
        let params = builder.build();
        assert_eq!(params.handle.flags, CbFlags::all().bits());
    }

    #[test]
    fn test_handler_params_builder_cb() {
        let mut builder = HandlerParamsBuilder::new();
        builder.cb(dummy_am_cb);
        let params = builder.build();
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_CB as u64) != 0);
    }

    #[test]
    fn test_handler_params_builder_cb_error_callback() {
        let mut builder = HandlerParamsBuilder::new();
        builder.cb(am_cb_return_error);
        let params = builder.build();
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_CB as u64) != 0);
    }

    #[test]
    fn test_handler_params_builder_arg() {
        let mut builder = HandlerParamsBuilder::new();
        let test_data: i32 = 42;
        builder.arg(&test_data as *const i32 as *mut c_void);
        let params = builder.build();
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ARG as u64) != 0);
        assert_eq!(params.handle.arg, &test_data as *const i32 as *mut c_void);
    }

    #[test]
    fn test_handler_params_builder_arg_null() {
        let mut builder = HandlerParamsBuilder::new();
        builder.arg(std::ptr::null_mut());
        let params = builder.build();
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ARG as u64) != 0);
        assert!(params.handle.arg.is_null());
    }

    #[test]
    fn test_handler_params_builder_full_chain() {
        let mut builder = HandlerParamsBuilder::new();
        let test_data: i32 = 99;
        builder
            .id(7)
            .flags(CbFlags::WholeMsg | CbFlags::PersistentData)
            .cb(dummy_am_cb)
            .arg(&test_data as *const i32 as *mut c_void);
        let params = builder.build();
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ID as u64) != 0);
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_FLAGS as u64) != 0);
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_CB as u64) != 0);
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ARG as u64) != 0);
        assert_eq!(params.handle.id, 7);
        assert_eq!(params.handle.flags, (CbFlags::WholeMsg | CbFlags::PersistentData).bits());
        assert_eq!(params.handle.arg, &test_data as *const i32 as *mut c_void);
    }

    #[test]
    fn test_handler_params_builder_partial_id_only() {
        let mut builder = HandlerParamsBuilder::new();
        builder.id(100);
        let params = builder.build();
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ID as u64) != 0);
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_FLAGS as u64) == 0);
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_CB as u64) == 0);
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ARG as u64) == 0);
    }

    #[test]
    fn test_handler_params_builder_partial_flags_only() {
        let mut builder = HandlerParamsBuilder::new();
        builder.flags(CbFlags::PersistentData);
        let params = builder.build();
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_FLAGS as u64) != 0);
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ID as u64) == 0);
    }

    #[test]
    fn test_handler_params_builder_chaining_returns_mut_ref() {
        let mut builder = HandlerParamsBuilder::new();
        let result = builder.id(1);
        result.flags(CbFlags::WholeMsg);
        result.cb(dummy_am_cb);
        let params = builder.build();
        assert_eq!(params.handle.id, 1);
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_CB as u64) != 0);
    }

    #[test]
    fn test_handler_params_builder_clone() {
        let mut builder = HandlerParamsBuilder::new();
        builder.id(15);
        let _cloned = builder.clone();
    }

    #[test]
    fn test_handler_params_builder_debug() {
        let builder = HandlerParamsBuilder::new();
        let debug_str = format!("{:?}", builder);
        assert!(!debug_str.is_empty());
    }

    #[test]
    fn test_field_mask_id_is_bit_0() {
        assert_eq!(ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ID as u64, 1);
    }

    #[test]
    fn test_field_mask_flags_is_bit_1() {
        assert_eq!(ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_FLAGS as u64, 2);
    }

    #[test]
    fn test_field_mask_cb_is_bit_2() {
        assert_eq!(ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_CB as u64, 4);
    }

    #[test]
    fn test_field_mask_arg_is_bit_3() {
        assert_eq!(ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ARG as u64, 8);
    }

    #[test]
    fn test_field_mask_all_bits_distinct() {
        let id = ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ID as u64;
        let flags = ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_FLAGS as u64;
        let cb = ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_CB as u64;
        let arg = ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ARG as u64;
        assert_eq!(id & flags, 0);
        assert_eq!(id & cb, 0);
        assert_eq!(id & arg, 0);
        assert_eq!(flags & cb, 0);
        assert_eq!(flags & arg, 0);
        assert_eq!(cb & arg, 0);
    }

    #[test]
    fn test_handler_params_builder_id_zero() {
        let mut builder = HandlerParamsBuilder::new();
        builder.id(0);
        let params = builder.build();
        assert_eq!(params.handle.id, 0);
        assert!(params.handle.field_mask & (ucp_am_handler_param_field::UCP_AM_HANDLER_PARAM_FIELD_ID as u64) != 0);
    }

    #[test]
    fn test_handler_params_builder_id_max() {
        let mut builder = HandlerParamsBuilder::new();
        builder.id(u32::MAX);
        let params = builder.build();
        assert_eq!(params.handle.id, u32::MAX);
    }

    #[test]
    fn test_cbflags_from_bits_exact() {
        assert_eq!(CbFlags::from_bits(1).unwrap(), CbFlags::WholeMsg);
        assert_eq!(CbFlags::from_bits(2).unwrap(), CbFlags::PersistentData);
        assert_eq!(CbFlags::from_bits(3).unwrap(), CbFlags::all());
    }

    #[test]
    fn test_cbflags_from_bits_invalid() {
        assert!(CbFlags::from_bits(4).is_none());
    }

    #[test]
    fn test_cbflags_from_bits_truncate() {
        let flags = CbFlags::from_bits_truncate(0xFF);
        assert_eq!(flags.bits(), 3u32);
    }
}
