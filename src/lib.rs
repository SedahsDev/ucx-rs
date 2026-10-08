//! Rust bindings for UCX's UCP API.
//!
//! # Threading model
//!
//! The default policy is a single-threaded progress loop. `Context`, `Worker`,
//! and `Ep` are intentionally `!Send` and `!Sync` today (as are the associated
//! handle wrappers such as `MemHandle` and `RemoteKey`). This is the current
//! safe Rust API policy, not a promise that UCX can never use these objects from
//! multiple threads. In particular, this crate does not provide unsafe `Send`
//! or `Sync` implementations, and making the handles transferable under
//! `ThreadMode::Multi` is future work.
//!
//! `worker::ParamsBuilder::thread_mode` selects the UCX contract for calls on
//! that worker:
//!
//! * `ThreadMode::Single` permits calls from one thread only.
//! * `ThreadMode::Serialized` permits multiple callers, but the application
//!   must serialize UCX calls.
//! * `ThreadMode::Multi` permits concurrent UCX calls where UCX documents
//!   them as thread-safe, but it does not make these Rust wrapper values
//!   transferable or remove application-level protocol synchronization.
//!
//! `Worker::progress()` should have one owning progress thread per worker. In
//! an MT-enabled UCX build with `ThreadMode::Multi`, concurrent progress is
//! safe but contended by UCX's internal spinlock; in non-MT builds or
//! `ThreadMode::Single`, it is undefined behavior. See `THREADING.md` §2.1.
//! Use `Worker::arm()` and `Worker::get_efd()` to wake the owning loop instead
//! of polling concurrently from multiple threads.
//! Operations and their borrowed buffers must also remain valid until UCX says
//! they have completed. Always drop/close endpoints before their `Worker`.
//! `Ep::Drop` checks the runtime `worker_alive` guard and skips a late cleanup as
//! a safety net; that guard is not a license to violate the required drop order.
//!

#![allow(unused_imports)]

mod ffi;
#[cfg(test)]
mod tests;
mod threading_assert;
use crate::ffi::*;

pub mod am;
pub mod config;
pub mod context;
pub mod dt;
pub mod ep;
pub mod error_handler_mode;
pub mod listener;
pub mod memh;
pub mod memory_type;
pub mod rma;
pub mod stream;
pub mod tag;
pub mod thread_mode;
pub mod version;
pub mod worker;

pub mod request;
pub mod status;

pub use request::*;
pub use status::*;

// Re-export wrapped types
pub use error_handler_mode::ErrorHandlerMode;
pub use memory_type::MemoryType;
pub use thread_mode::ThreadMode;

use std::ffi::CString;
use std::ptr::NonNull;
