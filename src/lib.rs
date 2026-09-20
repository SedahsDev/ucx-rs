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
//! `ThreadMode::Multi` mode is future work.
//!
//! `Worker::ParamsBuilder::thread_mode` selects the UCX contract for calls on
//! that worker:
//!
//! * `ThreadMode::Single` permits calls from one thread only.
//! * `ThreadMode::Serialized` permits multiple callers,
