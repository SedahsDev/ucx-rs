use crate::ffi::*;
use crate::status_to_result;
use crate::worker;
use crate::worker::Worker;
use bitflags::bitflags;
use std::ffi::CString;

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

#[derive(Debug, Clone)]
pub struct Config {
    handle: *mut ucp_config_t,
}

impl Config {
    pub fn read(name: &str, file: &str) -> Result<*mut ucp_config_t, ucs_status_t> {
        let mut config: *mut ucp_config_t = std::ptr::null_mut();
        let c_name = CString::new(name).unwrap();
        let c_file = CString::new(file).unwrap();
        status_to_result(unsafe { ucp_config_read(c_name.as_ptr(), c_file.as_ptr(), &mut config) })
            .unwrap();
        Ok(config)
    }
}

impl Default for Config {
    fn default() -> Self {
        let config = Config::read("", "").unwrap();
        Config { handle: config }
    }
}

impl Drop for Config {
    fn drop(&mut self) {
        unsafe { ucp_config_release(self.handle) };
    }
}

#[derive(Debug, Clone)]
pub struct ParamsBuilder {
    uninit_handle: std::mem::MaybeUninit<ucp_params_t>,
    field_mask: u64,
    name: Option<CString>,
}

#[derive(Debug, Clone)]
pub struct Params {
    handle: ucp_params_t,
    name: Option<CString>,
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
        let uninit_params = std::mem::MaybeUninit::<ucp_params_t>::uninit();
        ParamsBuilder {
            uninit_handle: uninit_params,
            field_mask: 0,
            name: None,
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

    pub fn name(&mut self, name: &str) -> &mut ParamsBuilder {
        self.field_mask |= ucp_params_field::UCP_PARAM_FIELD_NAME as u64;
        let name_cs = CString::new(name).unwrap();
        self.name = Some(name_cs);
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

impl Context {
    pub fn new(config: &Config, params: &Params) -> Result<Context, ucs_status_t> {
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
}

#[derive(Debug, Clone)]
pub struct Context {
    pub(crate) handle: ucp_context_h,
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe { ucp_cleanup(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::*;

    // ── Flags tests (pure unit tests, no FFI) ──

    #[test]
    fn test_flags_empty() {
        let flags = Flags::empty();
        assert!(flags.is_empty());
        assert!(!flags.contains(Flags::Tag));
        assert!(!flags.contains(Flags::Rma));
    }

    #[test]
    fn test_flags_tag() {
        let flags = Flags::Tag;
        assert!(flags.contains(Flags::Tag));
        assert!(!flags.contains(Flags::Rma));
        assert!(!flags.is_empty());
    }

    #[test]
    fn test_flags_rma() {
        let flags = Flags::Rma;
        assert!(!flags.contains(Flags::Tag));
        assert!(flags.contains(Flags::Rma));
    }

    #[test]
    fn test_flags_multiple() {
        let flags = Flags::Tag | Flags::Rma | Flags::Amo32;
        assert!(flags.contains(Flags::Tag));
        assert!(flags.contains(Flags::Rma));
        assert!(flags.contains(Flags::Amo32));
        assert!(!flags.contains(Flags::Amo64));
    }

    #[test]
    fn test_flags_all() {
        let flags = Flags::all();
        assert!(flags.contains(Flags::Tag));
        assert!(flags.contains(Flags::Rma));
        assert!(flags.contains(Flags::Amo32));
        assert!(flags.contains(Flags::Amo64));
        assert!(flags.contains(Flags::Wakeup));
        assert!(flags.contains(Flags::Stream));
        assert!(flags.contains(Flags::Am));
        assert!(flags.contains(Flags::ExportedMemH));
    }

    #[test]
    fn test_flags_clone_copy() {
        let a = Flags::Tag | Flags::Rma;
        let b = a.clone();
        assert_eq!(a, b);
        let c = a;
        assert_eq!(a, c);
    }

    #[test]
    fn test_flags_insert_remove() {
        let mut flags = Flags::empty();
        flags.insert(Flags::Tag);
        assert!(flags.contains(Flags::Tag));
        flags.remove(Flags::Tag);
        assert!(!flags.contains(Flags::Tag));
    }

    #[test]
    fn test_flags_debug() {
        let flags = Flags::Tag;
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
    fn test_params_builder_features() {
        let mut builder = ParamsBuilder::new();
        builder.features(Flags::Tag | Flags::Rma);
        let params = builder.build();
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_FEATURES as u64) != 0
        );
    }

    #[test]
    fn test_params_builder_request_size() {
        let mut builder = ParamsBuilder::new();
        builder.request_size(256);
        let params = builder.build();
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_REQUEST_SIZE as u64) != 0
        );
        assert_eq!(params.handle.request_size, 256);
    }

    #[test]
    fn test_params_builder_tag_sender_mask() {
        let mut builder = ParamsBuilder::new();
        builder.tag_sender_mask(0xFFFF_FFFF_FFFF_FFFF);
        let params = builder.build();
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_TAG_SENDER_MASK as u64)
                != 0
        );
        assert_eq!(params.handle.tag_sender_mask, 0xFFFF_FFFF_FFFF_FFFF);
    }

    #[test]
    fn test_params_builder_mt_workers_shared() {
        let mut builder = ParamsBuilder::new();
        builder.mt_workers_shared(1);
        let params = builder.build();
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_MT_WORKERS_SHARED as u64)
                != 0
        );
        assert_eq!(params.handle.mt_workers_shared, 1);
    }

    #[test]
    fn test_params_builder_estimated_num_eps() {
        let mut builder = ParamsBuilder::new();
        builder.estimated_num_eps(16);
        let params = builder.build();
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_ESTIMATED_NUM_EPS as u64)
                != 0
        );
        assert_eq!(params.handle.estimated_num_eps, 16);
    }

    #[test]
    fn test_params_builder_estimated_num_ppn() {
        let mut builder = ParamsBuilder::new();
        builder.estimated_num_ppn(4);
        let params = builder.build();
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_ESTIMATED_NUM_PPN as u64)
                != 0
        );
        assert_eq!(params.handle.estimated_num_ppn, 4);
    }

    #[test]
    fn test_params_builder_name() {
        let mut builder = ParamsBuilder::new();
        builder.name("test_context");
        let params = builder.build();
        assert!(params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_NAME as u64) != 0);
    }

    extern "C" fn dummy_init_cb(_request: *mut ::std::os::raw::c_void) {}
    extern "C" fn dummy_cleanup_cb(_request: *mut ::std::os::raw::c_void) {}

    #[test]
    fn test_params_builder_request_init() {
        let mut builder = ParamsBuilder::new();
        builder.request_init(dummy_init_cb);
        let params = builder.build();
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_REQUEST_INIT as u64) != 0
        );
    }

    #[test]
    fn test_params_builder_request_cleanup() {
        let mut builder = ParamsBuilder::new();
        builder.request_cleanup(dummy_cleanup_cb);
        let params = builder.build();
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_REQUEST_CLEANUP as u64)
                != 0
        );
    }

    #[test]
    fn test_params_builder_full_chain() {
        let mut builder = ParamsBuilder::new();
        builder
            .features(Flags::Tag | Flags::Rma)
            .request_size(128)
            .request_init(dummy_init_cb)
            .request_cleanup(dummy_cleanup_cb)
            .tag_sender_mask(u64::MAX)
            .mt_workers_shared(1)
            .estimated_num_eps(8)
            .estimated_num_ppn(2)
            .name("full_chain");
        let params = builder.build();
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_FEATURES as u64) != 0
        );
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_REQUEST_SIZE as u64) != 0
        );
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_REQUEST_INIT as u64) != 0
        );
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_REQUEST_CLEANUP as u64)
                != 0
        );
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_TAG_SENDER_MASK as u64)
                != 0
        );
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_MT_WORKERS_SHARED as u64)
                != 0
        );
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_ESTIMATED_NUM_EPS as u64)
                != 0
        );
        assert!(
            params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_ESTIMATED_NUM_PPN as u64)
                != 0
        );
        assert!(params.handle.field_mask & (ucp_params_field::UCP_PARAM_FIELD_NAME as u64) != 0);
    }

    #[test]
    fn test_params_builder_chaining_returns_mut_ref() {
        let mut builder = ParamsBuilder::new();
        let result = builder.features(Flags::Tag);
        result.request_size(64);
        let params = builder.build();
        assert_eq!(params.handle.request_size, 64);
    }

    // ── Context integration tests (require UCX library) ──

    #[test]
    fn test_context_create_default_config() {
        let config = Config::default();
        let params = ParamsBuilder::new().build();
        let ctx = Context::new(&config, &params).expect("context init");
        // Context created successfully — handle should be non-null
        assert!(!ctx.handle.is_null());
    }

    #[test]
    fn test_context_create_with_features() {
        let config = Config::default();
        let params = ParamsBuilder::new()
            .features(Flags::Tag | Flags::Rma | Flags::Amo32 | Flags::Amo64)
            .build();
        let ctx = Context::new(&config, &params).expect("context init");
        assert!(!ctx.handle.is_null());
    }

    #[test]
    fn test_context_create_with_name() {
        let config = Config::default();
        let params = ParamsBuilder::new()
            .name("test_named_context")
            .features(Flags::Tag)
            .build();
        let ctx = Context::new(&config, &params).expect("context init");
        assert!(!ctx.handle.is_null());
    }

    #[test]
    fn test_context_worker_create() {
        let config = Config::default();
        let params = ParamsBuilder::new().features(Flags::Tag).build();
        let ctx = Context::new(&config, &params).expect("context init");

        let worker_params = worker::ParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");
        // Worker created — handle should be non-null
        assert!(!worker.handle.is_null());
    }

    #[test]
    fn test_context_config_read() {
        let config_ptr = Config::read("", "").expect("config read");
        assert!(!config_ptr.is_null(), "config pointer should not be null");
    }

    // ── ParamsBuilder field mask tests ──

    #[test]
    fn test_params_field_features_is_bit_0() {
        assert_eq!(ucp_params_field::UCP_PARAM_FIELD_FEATURES as u64, 1);
    }

    #[test]
    fn test_params_field_request_size_is_bit_1() {
        assert_eq!(ucp_params_field::UCP_PARAM_FIELD_REQUEST_SIZE as u64, 2);
    }

    #[test]
    fn test_params_field_request_init_is_bit_2() {
        assert_eq!(ucp_params_field::UCP_PARAM_FIELD_REQUEST_INIT as u64, 4);
    }

    #[test]
    fn test_params_field_request_cleanup_is_bit_3() {
        assert_eq!(ucp_params_field::UCP_PARAM_FIELD_REQUEST_CLEANUP as u64, 8);
    }

    #[test]
    fn test_params_field_tag_sender_mask_is_bit_4() {
        assert_eq!(ucp_params_field::UCP_PARAM_FIELD_TAG_SENDER_MASK as u64, 16);
    }

    #[test]
    fn test_params_field_mt_workers_shared_is_bit_5() {
        assert_eq!(
            ucp_params_field::UCP_PARAM_FIELD_MT_WORKERS_SHARED as u64,
            32
        );
    }

    #[test]
    fn test_params_field_estimated_num_eps_is_bit_6() {
        assert_eq!(
            ucp_params_field::UCP_PARAM_FIELD_ESTIMATED_NUM_EPS as u64,
            64
        );
    }

    #[test]
    fn test_params_field_estimated_num_ppn_is_bit_7() {
        assert_eq!(
            ucp_params_field::UCP_PARAM_FIELD_ESTIMATED_NUM_PPN as u64,
            128
        );
    }

    #[test]
    fn test_params_field_name_is_bit_8() {
        assert_eq!(ucp_params_field::UCP_PARAM_FIELD_NAME as u64, 256);
    }

    #[test]
    fn test_params_fields_all_distinct() {
        let fields = [
            ucp_params_field::UCP_PARAM_FIELD_FEATURES as u64,
            ucp_params_field::UCP_PARAM_FIELD_REQUEST_SIZE as u64,
            ucp_params_field::UCP_PARAM_FIELD_REQUEST_INIT as u64,
            ucp_params_field::UCP_PARAM_FIELD_REQUEST_CLEANUP as u64,
            ucp_params_field::UCP_PARAM_FIELD_TAG_SENDER_MASK as u64,
            ucp_params_field::UCP_PARAM_FIELD_MT_WORKERS_SHARED as u64,
            ucp_params_field::UCP_PARAM_FIELD_ESTIMATED_NUM_EPS as u64,
            ucp_params_field::UCP_PARAM_FIELD_ESTIMATED_NUM_PPN as u64,
            ucp_params_field::UCP_PARAM_FIELD_NAME as u64,
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
