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
