//! Listener parameters for creating UCP listeners.
//!
//! This wraps the `ucp_listener_params` FFI type and provides a safe,
//! idiomatic Rust interface.

use std::mem::MaybeUninit;
use std::ptr;

use crate::listener::{ConnHandlerCb, UCP_LISTENER_PARAM_FIELD_ACCEPT_HANDLER, UCP_LISTENER_PARAM_FIELD_CONN_HANDLER, UCP_LISTENER_PARAM_FIELD_SOCK_ADDR};
use crate::status::Status;
use crate::ffi::*;
use bitflags::bitflags;

bitflags! {
    /// Listener parameter flags.
    #[repr(transparent)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ListenerParamFlags: u64 {
        const SOCK_ADDR = UCP_LISTENER_PARAM_FIELD_SOCK_ADDR as u64;
        const ACCEPT_HANDLER = UCP_LISTENER_PARAM_FIELD_ACCEPT_HANDLER as u64;
        const CONN_HANDLER = UCP_LISTENER_PARAM_FIELD_CONN_HANDLER as u64;
    }
}

/// Raw listener params builder for internal use.
pub(crate) struct RawListenerParamsBuilder {
    params: ucp_listener_params,
    flags: ListenerParamFlags,
}

impl Default for RawListenerParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl RawListenerParamsBuilder {
    /// Create a new builder with zeroed params.
    #[inline]
    pub fn new() -> Self {
        // SAFETY: UCX parameter structs are valid when zeroed.
        let params = unsafe { std::mem::zeroed() };
        Self {
            params,
            flags: ListenerParamFlags::empty(),
        }
    }

    /// Set the socket address.
    #[inline]
    pub(crate) fn sockaddr(&mut self, addr: &crate::ep::SockAddr) {
        self.flags |= ListenerParamFlags::SOCK_ADDR;
        self.params.sockaddr = addr.to_ffi();
    }

    /// Set the connection handler callback.
    #[inline]
    pub(crate) unsafe fn conn_handler(&mut self, callback: ConnHandlerCb, arg: *mut std::ffi::c_void) {
        self.flags |= ListenerParamFlags::CONN_HANDLER;
        self.params.conn_handler = ucp_listener_conn_handler { cb: callback, arg };
    }

    /// Set the accept handler callback.
    #[inline]
    pub(crate) unsafe fn accept_handler(&mut self, callback: crate::listener::AcceptHandlerCb, arg: *mut std::ffi::c_void) {
        self.flags |= ListenerParamFlags::ACCEPT_HANDLER;
        self.params.accept_handler = ucp_listener_accept_handler { cb: callback, arg };
    }

    /// Build the raw FFI params.
    #[inline]
    pub(crate) fn build(self) -> ucp_listener_params {
        self.params
    }
}

/// Builder for listener creation parameters.
pub struct ListenerParamsBuilder {
    raw_builder: RawListenerParamsBuilder,
}

impl ListenerParamsBuilder {
    /// Create a new builder.
    #[inline]
    pub fn new() -> Self {
        Self {
            raw_builder: RawListenerParamsBuilder::new(),
        }
    }

    /// Set the socket address to bind to.
    #[inline]
    pub fn sockaddr(&mut self, addr: &crate::ep::SockAddr) -> &mut Self {
        self.raw_builder.sockaddr(addr);
        self
    }

    /// Set the connection handler callback.
    ///
    /// # Safety
    /// The callback must remain valid for the listener's lifetime.
    #[inline]
    pub unsafe fn conn_handler(&mut self, callback: ConnHandlerCb, arg: *mut std::ffi::c_void) -> &mut Self {
        self.raw_builder.conn_handler(callback, arg);
        self
    }

    /// Set the accept handler callback.
    ///
    /// # Safety
    /// The callback must remain valid for the listener's lifetime.
    #[inline]
    pub unsafe fn accept_handler(&mut self, callback: crate::listener::AcceptHandlerCb, arg: *mut std::ffi::c_void) -> &mut Self {
        self.raw_builder.accept_handler(callback, arg);
        self
    }

    /// Build the parameters for listener creation.
    #[inline]
    pub fn build(self) -> ListenerParams {
        ListenerParams {
            handle: self.raw_builder.build(),
        }
    }
}

impl Default for ListenerParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Parameters for listener creation.
pub struct ListenerParams {
    handle: ucp_listener_params,
}

impl ListenerParams {
    /// Get the underlying raw handle (for internal use).
    #[inline]
    pub(crate) fn as_raw(&self) -> &ucp_listener_params {
        &self.handle
    }

    /// Get a mutable reference to the underlying raw handle (for internal use).
    #[inline]
    pub(crate) fn as_raw_mut(&mut self) -> &mut ucp_listener_params {
        &mut self.handle
    }
}
