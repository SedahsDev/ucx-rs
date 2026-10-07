//! UCP Remote Memory Access (RMA) bindings.
//!
//! Wraps `ucp_put_nbx`, `ucp_get_nbx`, `ucp_atomic_op_nbx`,
//! `ucp_ep_rkey_unpack`, `ucp_rkey_ptr`, and `ucp_rkey_destroy`.

pub mod atomic;
pub mod ep_ops;
pub mod remote_key;

pub use atomic::*;
pub use ep_ops::*;
pub use remote_key::*;

/// Re-export the remote key handle type for external callers.
#[allow(non_camel_case_types)]
pub(crate) type ucp_rkey_h = crate::ffi::ucp_rkey_h;
