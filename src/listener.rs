//! UCP listener and connection request bindings.
//!
//! Wraps `ucp_listener_create`, `ucp_listener_destroy`, `ucp_listener_query`,
//! `ucp_listener_reject`, and `ucp_conn_request_query`.

use crate::ffi::*;
use crate::status_to_result;
use std::mem::MaybeUninit;

/// Field masks for ucp_listener_params.
pub const UCP_LISTENER_PARAM_FIELD_SOCK_ADDR: u64 = 1;
pub const UCP_LISTENER_PARAM_FIELD_ACCEPT_HANDLER: u64 = 2;
pub const UCP_LISTENER_PARAM_FIELD_CONN_HANDLER: u64 = 4;

/// RAII wrapper for a UCP listener.
pub struct Listener {
    handle: ucp_listener_h,
}

unsafe impl Send for Listener {}

impl Listener {
    /// Create a listener on the given worker.
    ///
    /// # Safety
    /// The `sockaddr` must remain valid for the lifetime of the listener.
    pub unsafe fn create(
        worker: ucp_worker_h,
        sockaddr: *const ucs_sock_addr_t,
    ) -> Result<Self, ucs_status_t> {
        let mut listener: ucp_listener_h = std::ptr::null_mut();
        let mut params: ucp_listener_params = unsafe { std::mem::zeroed() };
        params.field_mask = UCP_LISTENER_PARAM_FIELD_SOCK_ADDR;
        params.sockaddr = *sockaddr;
        status_to_result(ucp_listener_create(worker, &params, &mut listener)).map(|()| Self { handle: listener })
    }

    /// Query listener attributes.
    pub fn query(&self) -> Result<ListenerAttr, ucs_status_t> {
        let mut attr: ucp_listener_attr = unsafe { std::mem::zeroed() };
        attr.field_mask = 1; // UCP_LISTENER_ATTR_FIELD_SOCKADDR
        status_to_result(unsafe { ucp_listener_query(self.handle, &mut attr) }).map(|()| ListenerAttr {
            sockaddr: attr.sockaddr,
        })
    }

    /// Reject a connection request.
    pub fn reject(&self, conn_request: ucp_conn_request_h) -> Result<(), ucs_status_t> {
        status_to_result(unsafe { ucp_listener_reject(self.handle, conn_request) })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ucp_listener_destroy(self.handle); }
        }
    }
}

/// Listener attribute result.
#[derive(Debug, Clone, Copy)]
pub struct ListenerAttr {
    pub sockaddr: sockaddr_storage,
}

/// Field masks for ucp_conn_request_attr.
pub const UCP_CONN_REQUEST_ATTR_FIELD_CLIENT_ADDR: u64 = 1;
pub const UCP_CONN_REQUEST_ATTR_FIELD_CLIENT_ID: u64 = 2;

/// Query connection request attributes.
pub fn conn_request_query(
    conn_request: ucp_conn_request_h,
    mask: u64,
) -> Result<ConnRequestAttr, ucs_status_t> {
    let mut attr: ucp_conn_request_attr = unsafe { std::mem::zeroed() };
    attr.field_mask = mask;
    status_to_result(unsafe { ucp_conn_request_query(conn_request, &mut attr) }).map(|()| ConnRequestAttr {
        client_id: if mask & UCP_CONN_REQUEST_ATTR_FIELD_CLIENT_ID != 0 { attr.client_id } else { 0 },
    })
}

/// Connection request attribute result.
#[derive(Debug, Clone)]
pub struct ConnRequestAttr {
    pub client_id: u64,
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn test_listener_create() {
        // Requires a worker and socket address
        unimplemented!("Requires worker setup");
    }
}
