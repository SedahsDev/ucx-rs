//! Rust bindings for UCX's UCP API.
//!
//! # Threading model
//!
//! The default policy is a single-threaded progress loop. `Context`, `Worker`,
//! and `Ep` are intentionally `!Send` and `!Sync` today (as are the associated
//! handle wrappers such as `MemHandle` and `RemoteKey`). This is the current
//! safe Rust API policy, not a promise that UCX can never use these objects from
//! multiple threads. In particular, this crate does not provide unsafe `Send`
//! or `Sync` implementations, and making the handles transferable in UCX's
//! `UCS_THREAD_MODE_MULTI` mode is future work.
//!
//! `Worker::ParamsBuilder::thread_mode` selects the UCX contract for calls on
//! that worker:
//!
//! * `UCS_THREAD_MODE_SINGLE` permits calls from one thread only.
//! * `UCS_THREAD_MODE_SERIALIZED` permits multiple callers, but the application
//!   must serialize UCX calls.
//! * `UCS_THREAD_MODE_MULTI` permits concurrent UCX calls where UCX documents
//!   them as thread-safe, but it does not make these Rust wrapper values
//!   transferable or remove application-level protocol synchronization.
//!
//! `Worker::progress()` should have one owning progress thread per worker. In
//! an MT-enabled UCX build with `UCS_THREAD_MODE_MULTI`, concurrent progress is
//! safe but contended by UCX's internal spinlock; in non-MT builds or
//! `UCS_THREAD_MODE_SINGLE`, it is undefined behavior. See `THREADING.md` §2.1.
//! Use `Worker::arm()` and `Worker::get_efd()` to wake the owning loop instead
//! of polling concurrently from multiple threads.
//! Operations and their borrowed buffers must also remain valid until UCX says
//! they have completed. Always drop/close endpoints before their `Worker`.
//! `Ep::Drop` checks the runtime `worker_alive` guard and skips a late cleanup as
//! a safety net; that guard is not a license to violate the required drop order.
//!

#![allow(unused_imports)]

mod ffi;
mod threading_assert;
use crate::ffi::*;

// Enumerations that appear in this crate's *public* signatures (`MemMapParamsBuilder::memory_type`,
// `Builder::memory_type`, runtime thread-mode setup). Without re-exporting them, downstream crates
// cannot name a type they are required to pass — the compiler reports `enum ... is private` even
// though the method taking it is `pub`. Re-export rather than widening `ffi` to `pub`.
pub use crate::ffi::{ucs_memory_type_t, ucs_thread_mode_t};

pub mod am;
pub mod config;
pub mod context;
pub mod dt;
pub mod ep;
pub mod listener;
pub mod memh;
pub mod rma;
pub mod stream;
pub mod tag;
pub mod version;
pub mod worker;

pub mod request;
pub mod status;

pub use request::*;
pub use status::*;

use std::ffi::CString;
use std::ptr::NonNull;
