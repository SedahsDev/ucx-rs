//! UCP version and library query bindings.
//!
//! Wraps `ucp_get_version`, `ucp_get_version_string`, `ucp_init` (legacy),
//! and `ucp_lib_query`.

use crate::ffi::*;
use crate::status_to_result;
use std::ffi::CStr;

/// Get the UCX library version.
/// Returns (major, minor, release_number).
pub fn get_version() -> (u32, u32, u32) {
    let (mut major, mut minor, mut release) = (0, 0, 0);
    unsafe {
        ucp_get_version(&mut major, &mut minor, &mut release);
    }
    (major, minor, release)
}

/// Get the UCX library version as a string.
pub fn get_version_string() -> String {
    unsafe {
        let ptr = ucp_get_version_string();
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

/// Field mask for ucp_lib_attr.
pub const UCP_LIB_ATTR_FIELD_MAX_THREAD_LEVEL: u64 = 1;

/// Library attributes queried via `lib_query()`.
#[derive(Debug, Clone)]
pub struct LibAttr {
    pub max_thread_level: ucs_thread_mode_t,
}

/// Query library-wide attributes.
pub fn lib_query() -> Result<LibAttr, ucs_status_t> {
    let mut attr: ucp_lib_attr = unsafe { std::mem::zeroed() };
    attr.field_mask = UCP_LIB_ATTR_FIELD_MAX_THREAD_LEVEL;
    status_to_result(unsafe { ucp_lib_query(&mut attr) }).map(|()| LibAttr {
        max_thread_level: attr.max_thread_level,
    })
}

/// Legacy `ucp_init` is deprecated in UCX 1.18 and not available in the
/// generated bindings. Use `Context::init_default()` or `Context::new()` instead.
#[deprecated(
    since = "0.1.0",
    note = "ucp_init removed in UCX 1.18; use Context::init_default() or Context::new()"
)]
///
/// # Safety
/// This function always returns an error. Provided for API compatibility only.
pub unsafe fn init_raw() -> Result<ucp_context_h, ucs_status_t> {
    Err(ucs_status_t::UCS_ERR_UNSUPPORTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_version() {
        let (major, minor, _release) = get_version();
        assert!(major > 0 || minor > 0, "Version should be non-zero");
    }

    #[test]
    fn test_get_version_string() {
        let s = get_version_string();
        assert!(!s.is_empty(), "Version string should not be empty");
    }

    #[test]
    fn test_lib_query() {
        let attr = lib_query().expect("lib_query should succeed");
        // Just verify it returns something valid
        let _ = attr.max_thread_level as i32;
    }
}
