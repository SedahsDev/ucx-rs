//! Worker and related types for UCX communication.
//!
//! This module provides the core worker abstraction and associated types
//! for managing UCX endpoints, progress, and resource management.

pub mod address;
pub mod params;
// Keeps the public `worker::worker` path; its items are re-exported below.
#[allow(clippy::module_inception)]
pub mod worker;

pub use address::*;
pub use params::*;
pub use worker::*;
