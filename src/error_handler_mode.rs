//! Error handling mode for UCP endpoints.
//!
//! This wraps the `ucp_err_handling_mode_t` FFI type.

use crate::status::Status;

/// Error handling mode for UCP endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ErrorHandlerMode {
    /// No error handling from peer.
    None,
    /// Peer error handling enabled.
    Peer,
}

impl From<ErrorHandlerMode> for crate::ffi::ucp_err_handling_mode_t {
    fn from(mode: ErrorHandlerMode) -> Self {
        match mode {
            ErrorHandlerMode::None => crate::ffi::ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_NONE,
            ErrorHandlerMode::Peer => crate::ffi::ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_PEER,
        }
    }
}

impl TryFrom<crate::ffi::ucp_err_handling_mode_t> for ErrorHandlerMode {
    type Error = crate::Status;

    fn try_from(mode: crate::ffi::ucp_err_handling_mode_t) -> Result<Self, Self::Error> {
        match mode {
            crate::ffi::ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_NONE => Ok(ErrorHandlerMode::None),
            crate::ffi::ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_PEER => Ok(ErrorHandlerMode::Peer),
            _ => Err(Status::from_raw(
                crate::ffi::ucs_status_t::UCS_ERR_INVALID_PARAM,
            )),
        }
    }
}
