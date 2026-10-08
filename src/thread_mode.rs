//! Thread mode enum for worker threading configuration.
//!
//! This wraps the `ucs_thread_mode_t` FFI type and provides a safe,
//! idiomatic Rust interface.

/// Thread mode for UCX worker operations.
///
/// This enum is `#[non_exhaustive]`: variants may be added when the crate supports newer UCX
/// versions, so a `match` on it outside this crate needs a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ThreadMode {
    /// Single-threaded access only.
    Single,
    /// Serialized access from multiple threads.
    Serialized,
    /// Multi-threaded access allowed.
    Multi,
    /// Not a thread mode UCX grants. Reported only if UCX returns its end-of-enum sentinel
    /// value instead of a real mode. Passed to `worker::ParamsBuilder::thread_mode`, it is
    /// requested as `ThreadMode::Multi`.
    Unknown,
}

impl ThreadMode {
    /// Convert to the raw FFI thread mode. `Unknown` is not a real mode; it is requested as
    /// the multi-threaded mode, the one with the strongest thread-safety guarantees.
    pub(crate) const fn to_ffi(self) -> crate::ffi::ucs_thread_mode_t {
        match self {
            ThreadMode::Single => crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE,
            ThreadMode::Serialized => crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SERIALIZED,
            ThreadMode::Multi | ThreadMode::Unknown => {
                crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_MULTI
            }
        }
    }

    /// Convert from the raw FFI thread mode. The bindgen enum is a real Rust enum, so the
    /// only other value it can hold is UCX's end-of-enum sentinel, which maps to `Unknown`.
    pub(crate) fn from_ffi(raw: crate::ffi::ucs_thread_mode_t) -> Self {
        match raw {
            crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SINGLE => ThreadMode::Single,
            crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_SERIALIZED => ThreadMode::Serialized,
            crate::ffi::ucs_thread_mode_t::UCS_THREAD_MODE_MULTI => ThreadMode::Multi,
            _ => ThreadMode::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::ucs_thread_mode_t;

    #[test]
    fn thread_mode_unknown_is_requested_as_multi() {
        assert_eq!(
            ThreadMode::Unknown.to_ffi(),
            ucs_thread_mode_t::UCS_THREAD_MODE_MULTI
        );
        assert_eq!(
            ThreadMode::from_ffi(ucs_thread_mode_t::UCS_THREAD_MODE_LAST),
            ThreadMode::Unknown
        );
    }

    #[test]
    fn thread_mode_known_variants_round_trip() {
        for mode in [
            ThreadMode::Single,
            ThreadMode::Serialized,
            ThreadMode::Multi,
        ] {
            assert_eq!(ThreadMode::from_ffi(mode.to_ffi()), mode);
        }
    }
}
