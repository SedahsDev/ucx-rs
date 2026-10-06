//! Worker parameters for creating UCP workers.
//!
//! This wraps the `ucp_worker_params` FFI type and provides a safe,
//! idiomatic Rust interface.

use std::mem::MaybeUninit;
use std::ptr;

use crate::ffi::*;
use crate::thread_mode::ThreadMode;

/// Builder for worker creation parameters.
pub struct WorkerParamsBuilder {
    params: ucp_worker_params,
    flags: u64,
}

impl Default for WorkerParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerParamsBuilder {
    /// Create a new builder with zeroed params.
    #[inline]
    pub fn new() -> Self {
        // SAFETY: ucp_worker_params is a C POD; zero is the documented
        // default for optional callbacks and unused fields.
        let params = unsafe { MaybeUninit::<ucp_worker_params>::zeroed().assume_init() };
        Self {
            params,
            flags: 0,
        }
    }

    /// Set the thread mode for the worker.
    #[inline]
    pub fn thread_mode(&mut self, mode: ThreadMode) -> &mut Self {
        self.flags |= ucp_worker_params_field::UCP_WORKER_PARAM_FIELD_THREAD_MODE as u64;
        self.params.thread_mode = mode.into();
        self
    }

    /// Build the parameters for worker creation.
    #[inline]
    pub fn build(self) -> WorkerParams {
        WorkerParams {
            handle: self.params,
        }
    }
}

/// Parameters for worker creation.
pub struct WorkerParams {
    handle: ucp_worker_params,
}

impl WorkerParams {
    /// Get the underlying raw handle (for internal use).
    #[inline]
    pub(crate) fn as_raw(&self) -> &ucp_worker_params {
        &self.handle
    }
}
