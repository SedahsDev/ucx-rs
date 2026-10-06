//! Error handling mode for UCP endpoints.
//!
//! This wraps the `ucp_err_handling_mode_t` FFI type.

/// Error handling mode for UCP endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ErrorHandlerMode {
    /// No error handling from peer.
    None,
    /// Peer error handling enabled.
    Peer,
}

impl ErrorHandlerMode {
    /// Convert to the raw FFI error handling mode.
    pub(crate) const fn to_ffi(self) -> crate::ffi::ucp_err_handling_mode_t {
        match self {
            ErrorHandlerMode::None => {
                crate::ffi::ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_NONE
            }
            ErrorHandlerMode::Peer => {
                crate::ffi::ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_PEER
            }
        }
    }

    /// Convert from the raw FFI error handling mode. Unknown values map to
    /// [`ErrorHandlerMode::None`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_ffi(raw: crate::ffi::ucp_err_handling_mode_t) -> Self {
        if raw == crate::ffi::ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_PEER {
            ErrorHandlerMode::Peer
        } else {
            ErrorHandlerMode::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for mode in [ErrorHandlerMode::None, ErrorHandlerMode::Peer] {
            assert_eq!(mode.from_ffi(mode.to_ffi()), mode);
        }
    }

    /// SAFETY: the FFI enum is a C int; 99 is outside any defined variant.
    #[test]
    fn unknown_maps_to_none() {
        let raw = unsafe { std::mem::transmute::<u32, crate::ffi::ucp_err_handling_mode_t>(99) };
        assert_eq!(ErrorHandlerMode::from_ffi(raw), ErrorHandlerMode::None);
    }
}
