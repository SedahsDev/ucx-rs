//! UCP Remote Memory Access (RMA) bindings.
//!
//! Wraps `ucp_put_nbx`, `ucp_get_nbx`, `ucp_atomic_op_nbx`,
//! `ucp_ep_rkey_unpack`, `ucp_rkey_ptr`, and `ucp_rkey_destroy`.

pub mod remote_key;
pub mod ep_ops;
pub mod atomic;

pub use remote_key::*;
pub use ep_ops::*;
pub use atomic::*;

/// Re-export the remote key handle type for external callers.
#[allow(non_camel_case_types)]
pub type ucp_rkey_h = crate::ffi::ucp_rkey_h;
