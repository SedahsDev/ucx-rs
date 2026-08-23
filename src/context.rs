use crate::ffi::*;
use crate::status_to_result;
use crate::worker;
use crate::worker::Worker;
use bitflags::bitflags;
use std::ffi::CString;

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Nul(std::ffi::NulError),
    Ucs(ucs_status_t),
}

impl From<std::ffi::NulError> for ConfigError {
    fn from(error: std::ffi::NulError) -> Self {
        Self::Nul(error)
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nul(error) => error.fmt(f),
            Self::Ucs(status) => write!(f, "UCX error: {status:?}"),
        }
    }
}

impl std::error::Error for ConfigError {}

type RequestInitCb = unsafe extern "C" fn(request: *mut ::std::os::raw::c_void);
type RequestCleanUpCb = unsafe extern "C" fn(request: *mut ::std::os::raw::c_void);

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Flags: u64 {
        const Tag = ucp_feature::UCP_FEATURE_TAG as u64;
        const Rma = ucp_feature::UCP_FEATURE_RMA as u64;
        const Amo32 = ucp_feature::UCP_FEATURE_AMO32 as u64;
        const Amo64 = ucp_feature::UCP_FEATURE_AMO64 as u64;
        const Wakeup = ucp_feature::UCP_FEATURE_WAKEUP as u64;
        const Stream = ucp_feature::UCP_FEATURE_STREAM as u64;
        const Am = ucp_feature::UCP_FEATURE_AM as u64;
        const ExportedMemH = ucp_feature::UCP_FEATURE_EXPORTED_MEMH as u64;
    }
}

/// UCX configuration ownership wrapper.
///
/// This type is intentionally not `Clone`: cloning would duplicate ownership
/// of the configuration pointer and cause a double release.
#[derive(Debug)]
pub struct Config {
    pub(crate) handle: *mut ucp_config_t,
}

impl Config {
    pub fn read(name: &str, file: &str) -> Result<Config, ConfigError> {
        let mut config: *mut ucp_config_t = std::ptr::null_mut();
        let c_name = CString::new(name)?;
        let c_file = CString::new(file)?;
        status_to_result(unsafe { ucp_config_read(c_name.as_ptr(), c_file.as_ptr(), &mut config) })
            .map(|()| Config { handle: config })
            .map_err(ConfigError::Ucs)
    }

    /// Print this configuration to `fd`. Invalid descriptors or titles with NULs are ignored.
    pub fn print(
        &self,
        title: &str,
        print_flags: ucs_config_print_flags_t,
        fd: std::os::fd::RawFd,
    ) {
        let Ok(title) = CString::new(title) else {
            return;
        };
        let _ = crate::config::with_file_stream(fd, |stream| {
            // SAFETY: self owns a live configuration; title and stream are valid for this call.
            unsafe { ucp_config_print(self.handle, stream.cast(), title.as_ptr(), print_flags) };
        });
    }
}

impl Drop for Config {
    fn drop(&mut self) {
        unsafe { ucp_config_release(self.handle) };
    }
}

#[derive(Debug)]
pub struct ParamsBuilder {
    uninit_handle: std::mem::MaybeUninit<ucp_params_t>,
    field_mask: u64,
    name: Option<CString>,
    mt_workers_shared: bool,
}

#[derive(Debug)]
pub struct Params {
    handle: ucp_params_t,
    name: Option<CString>,
    pub(crate) mt_workers_shared: bool,
}

// This builder wraps up the unsafe parts of building the ucp_param_t struct. On construction
// it makes a zero filled ucp_params_t which Rust considers uninitialized. Each call on the builder
// will fill in the fields of the struct and add the mask for that field. On the final build()
// it will fill in the final value of the features field_mask and proclame the rest of the struct
// as initialized. This is Rust safe because all of the other fields are guaranteed to not be used
// by the library since the proper feature flag is not set.

impl Default for ParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ParamsBuilder {
    pub fn new() -> ParamsBuilder {
        // SAFETY: UCX parameter structs are valid when zeroed; the field mask controls reads.
        let uninit_params = std::mem::MaybeUninit::new(unsafe { std::mem::zeroed() });
        ParamsBuilder {
            uninit_handle: uninit_params,
            field_mask: 0,
            name: None,
            mt_workers_shared: false,
        }
    }

    pub fn features(&mut self, features: Flags) -> &mut ParamsBuilder {
        self.field_mask |= ucp_params_field::UCP_PARAM_FIELD_FEATURES as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.features = features.bits();
        self
    }

    pub fn request_size(&mut self, size: usize) -> &mut ParamsBuilder {
        self.field_mask |= ucp_params_field::UCP_PARAM_FIELD_REQUEST_SIZE as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.request_size = size;
        self
    }

    pub fn request_init(&mut self, cb: RequestInitCb) -> &mut ParamsBuilder {
        self.field_mask |= ucp_params_field::UCP_PARAM_FIELD_REQUEST_INIT as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };

        params.request_init = Some(cb);
        self
    }

    pub fn request_cleanup(&mut self, cb: RequestCleanUpCb) -> &mut ParamsBuilder {
        self.field_mask |= ucp_params_field::UCP_PARAM_FIELD_REQUEST_CLEANUP as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.request_cleanup = Some(cb);
        self
    }

    pub fn tag_sender_mask(&mut self, mask: u64) -> &mut ParamsBuilder {
        self.field_mask |= ucp_params_field::UCP_PARAM_FIELD_TAG_SENDER_MASK as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.tag_sender_mask = mask;
        self
    }

    pub fn mt_workers_shared(&mut self, shared: i32) -> &mut ParamsBuilder {
        self.field_mask |= ucp_params_field::UCP_PARAM_FIELD_MT_WORKERS_SHARED as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.mt_workers_shared = shared;
        self.mt_workers_shared = shared != 0;
        self
    }

    pub fn estimated_num_eps(&mut self, num_eps: usize) -> &mut ParamsBuilder {
        self.field_mask |= ucp_params_field::UCP_PARAM_FIELD_ESTIMATED_NUM_EPS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.estimated_num_eps = num_eps;
        self
    }

    pub fn estimated_num_ppn(&mut self, num_ppn: usize) -> &mut ParamsBuilder {
        self.field_mask |= ucp_params_field::UCP_PARAM_FIELD_ESTIMATED_NUM_PPN as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.estimated_num_ppn = num_ppn;
        self
    }

    pub fn name(&mut self, name: &str) -> Result<&mut ParamsBuilder, std::ffi::NulError> {
        let name_cs = CString::new(name)?;
        self.field_mask |= ucp_params_field::UCP_PARAM_FIELD_NAME as u64;
        self.name = Some(name_cs);
        Ok(self)
    }

    pub fn build(&mut self) -> Params {
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.field_mask = self.field_mask;

        let mut ucp_param = Params {
            name: None,
            mt_workers_shared: self.mt_workers_shared,
            handle: unsafe { self.uninit_handle.assume_init() },
        };

        if let Some(new_name) = self.name.take() {
            ucp_param.handle.name = new_name.as_ptr();
            ucp_param.name = Some(new_name);
        }

        ucp_param
    }
}

impl Context {
    /// Initializes a context; at least one non-empty feature is required.
    pub fn new(config: &Config, params: &Params) -> Result<Context, ucs_status_t> {
        if params.handle.field_mask & ucp_params_field::UCP_PARAM_FIELD_FEATURES as u64 == 0
            || params.handle.features == 0
        {
            return Err(ucs_status_t::UCS_ERR_INVALID_PARAM);
        }
        if !params.mt_workers_shared
            || params.handle.field_mask & ucp_params_field::UCP_PARAM_FIELD_MT_WORKERS_SHARED as u64
                == 0
            || params.handle.mt_workers_shared == 0
        {
            return Err(ucs_status_t::UCS_ERR_INVALID_PARAM);
        }
        let mut context: ucp_context_h = std::ptr::null_mut();

        let result = status_to_result(unsafe {
            ucp_init_version(
                UCP_API_MAJOR,
                UCP_API_MINOR,
                &params.handle,
                config.handle,
                &mut context,
            )
        });
        match result {
            Ok(()) => Ok(Context { handle: context }),
            Err(ucs_status_t) => Err(ucs_status_t),
        }
    }

    pub fn worker_create<'a>(&'a self, params: &'a worker::Params) -> Result<Worker, ucs_status_t> {
        Worker::new(self, params)
    }

    /// Print context diagnostics to `fd`. Invalid descriptors are ignored.
    pub fn print_info(&self, fd: std::os::fd::RawFd) {
        let _ = crate::config::with_file_stream(fd, |stream| {
            // SAFETY: self owns a live context and stream is valid for this call.
            unsafe { ucp_context_print_info(self.handle, stream.cast()) };
        });
    }
}

/// UCX context ownership wrapper.
///
/// This type is intentionally not `Clone`: cloning would create two owners of
/// one context and cause a double cleanup/use-after-free.
#[derive(Debug)]
pub struct Context {
    pub(crate) handle: ucp_context_h,
}

// SAFETY: Send is sound because Context::new refuses to construct a Context
// unless UCP_PARAM_FIELD_MT_WORKERS_SHARED was set nonzero, which enables UCX's
// internal context mt-lock (ucp_context.c). Send is additionally sound from
// exclusive non-Clone RAII ownership. Worker and Ep remain !Send, so per-worker
// state stays thread-bound.
unsafe impl Send for Context {}

// SAFETY: Sync is sound because Context::new refuses to construct a Context
// unless UCP_PARAM_FIELD_MT_WORKERS_SHARED was set nonzero, which enables UCX's
// internal context mt-lock (ucp_context.c). Send is additionally sound from
// exclusive non-Clone RAII ownership. Worker and Ep remain !Send, so per-worker
// state stays thread-bound even when shared Context access crosses threads.
unsafe impl Sync for Context {}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: self.handle is the live context handle owned by this wrapper.
        unsafe { ucp_cleanup(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_rejects_missing_features_before_ffi() {
        let config = Config::read("", "").expect("config read");
        let params = ParamsBuilder::new().mt_workers_shared(1).build();
        assert!(matches!(
            Context::new(&config, &params),
            Err(ucs_status_t::UCS_ERR_INVALID_PARAM)
        ));
    }

    #[test]
    fn context_rejects_unshared_mt_context() {
        let config = Config::read("", "").expect("config read");
        let params = ParamsBuilder::new().features(Flags::Tag).build();
        assert!(matches!(
            Context::new(&config, &params),
            Err(ucs_status_t::UCS_ERR_INVALID_PARAM)
        ));
    }
}
