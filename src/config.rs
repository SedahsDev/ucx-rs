//! UCP configuration helpers.
//!
//! Wraps `ucp_config_modify`, `ucp_context_query`,
//! and `ucp_context_print_info`.

use crate::ffi::*;
use crate::status_to_result;
use std::ffi::CStr;

/// Modify a configuration value.
///
/// # Safety
/// Caller must ensure `config` is a valid configuration pointer obtained from `ucp_config_read`.
pub unsafe fn config_modify(
    config: *mut ucp_config_t,
    name: &str,
    value: &str,
) -> Result<(), ucs_status_t> {
    let cname = std::ffi::CString::new(name).expect("config name");
    let cvalue = std::ffi::CString::new(value).expect("config value");
    status_to_result(ucp_config_modify(config, cname.as_ptr(), cvalue.as_ptr()))
}

/// Field masks for `ucp_context_attr`.
pub const UCP_CONTEXT_ATTR_FIELD_REQUEST_SIZE: u64 = 1;
pub const UCP_CONTEXT_ATTR_FIELD_THREAD_MODE: u64 = 2;
pub const UCP_CONTEXT_ATTR_FIELD_MEMORY_TYPES: u64 = 4;
pub const UCP_CONTEXT_ATTR_FIELD_NAME: u64 = 8;

/// Context attributes queried via `context_query()`.
#[derive(Debug, Clone)]
pub struct ContextAttr {
    pub request_size: usize,
    pub thread_mode: ucs_thread_mode_t,
    pub memory_types: u64,
    pub name: String,
}

/// Query context attributes.
///
/// # Safety
/// Caller must ensure `context` is a valid UCP context handle.
pub unsafe fn context_query(
    context: ucp_context_h,
    mask: u64,
) -> Result<ContextAttr, ucs_status_t> {
    let mut attr: ucp_context_attr = std::mem::zeroed();
    attr.field_mask = mask;
    status_to_result(ucp_context_query(context, &mut attr)).map(|()| {
        let name = if mask & UCP_CONTEXT_ATTR_FIELD_NAME != 0 {
            CStr::from_ptr(attr.name.as_ptr())
                .to_string_lossy()
                .into_owned()
        } else {
            String::new()
        };
        ContextAttr {
            request_size: attr.request_size,
            thread_mode: attr.thread_mode,
            memory_types: attr.memory_types,
            name,
        }
    })
}

/// Print context info to a file descriptor.
///
/// # Safety
/// Caller must ensure `context` is a valid UCP context handle and `fd` is a valid file descriptor.
pub unsafe fn context_print_info(context: ucp_context_h, fd: std::os::fd::RawFd) {
    ucp_context_print_info(context, fd as *mut _);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Config, Context, ParamsBuilder};

    #[test]
    fn test_config_modify() {
        let config_ptr = Config::read("", "").expect("config read");
        // UCX 1.18+ silently accepts unknown config keys (returns UCS_OK).
        // Test that the FFI call itself succeeds — the key/value is a no-op.
        let result = unsafe { config_modify(config_ptr, "invalid_key_xyz", "value") };
        assert!(
            result.is_ok(),
            "config_modify FFI call should succeed (UCX 1.18+ ignores unknown keys)"
        );
    }

    #[test]
    fn test_context_query() {
        let config = Config::default();
        let params = ParamsBuilder::new().build();
        let ctx = Context::new(&config, &params).expect("init");
        let attr = unsafe { context_query(ctx.handle, UCP_CONTEXT_ATTR_FIELD_REQUEST_SIZE) }
            .expect("context_query");
        assert!(attr.request_size > 0);
    }
}
