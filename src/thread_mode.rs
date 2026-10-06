//! Thread mode enum for worker threading configuration.
//!
//! This wraps the `ucs_thread_mode_t` FFI type and provides a safe,
//! idiomatic Rust interface.

/// Thread mode for UCX worker operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThreadMode {
    /// Single-threaded access only.
    Single,
    /// Serialized access from multiple threads.
    Serialized,
    /// Multi-threaded access allowed.
    Multi,
    /// A value UCX returned that this build's enum does not name.
    Unknown,
}

impl ThreadMode {
    /// Convert to the raw FFI thread mode. `Unknown` maps to the last known
    /// variant so the conversion stays total.
    pub(crate) const fn to_ffi(self) -> crate::ffi::ucs_thread_mode_t {
        match self {
            ThreadMode::Single => crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE,
            ThreadMode::Serialized => crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SERIALIZED,
            ThreadMode::Multi | ThreadMode::Unknown => {
                crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_MULTI
            }
        }
    }

    /// Convert from the raw FFI thread mode. Unknown values map to
    /// [`ThreadMode::Unknown`].
    pub(crate) fn from_ffi(raw: crate::ffi::ucs_thread_mode_t) -> Self {
        match raw {
            crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE => ThreadMode::Single,
            crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SERIALIZED => ThreadMode::Serialized,
            crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_MULTI => ThreadMode::Multi,
            _ => ThreadMode::Unknown,
        }
    }
}
