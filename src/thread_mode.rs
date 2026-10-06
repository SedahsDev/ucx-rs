//! Thread mode enum for worker threading configuration.
//!
//! This wraps the `ucs_thread_mode_t` FFI type and provides a safe,
//! idiomatic Rust interface.

use crate::status::Status;

/// Thread mode for UCX worker operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThreadMode {
    /// Single-threaded access only.
    Single,
    /// Serialized access from multiple threads.
    Serialized,
    /// Multi-threaded access allowed.
    Multi,
}

impl From<ThreadMode> for crate::ffi::ucs_thread_mode_t {
    fn from(mode: ThreadMode) -> Self {
        match mode {
            ThreadMode::Single => crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE,
            ThreadMode::Serialized => crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SERIALIZED,
            ThreadMode::Multi => crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_MULTI,
        }
    }
}

impl TryFrom<crate::ffi::ucs_thread_mode_t> for ThreadMode {
    type Error = crate::Status;

    fn try_from(mode: crate::ffi::ucs_thread_mode_t) -> Result<Self, Self::Error> {
        match mode {
            crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE => Ok(ThreadMode::Single),
            crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SERIALIZED => Ok(ThreadMode::Serialized),
            crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_MULTI => Ok(ThreadMode::Multi),
            _ => Err(Status::from_raw(
                crate::ffi::ucs_status_t::UCS_ERR_INVALID_PARAM,
            )),
        }
    }
}
