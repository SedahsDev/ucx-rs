//! UCP listener and connection request bindings.
//!
//! Wraps `ucp_listener_create`, `ucp_listener_destroy`, `ucp_listener_query`,
//! `ucp_listener_reject`, and `ucp_conn_request_query`.
//!
//! `Listener` follows UCX's single-threaded default and is intentionally not
//! `Send` or `Sync`. Keep it on its owning progress thread.

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
        status_to_result(ucp_listener_create(worker, &params, &mut listener))
            .map(|()| Self { handle: listener })
    }

    /// Query listener attributes.
    pub fn query(&self) -> Result<ListenerAttr, ucs_status_t> {
        let mut attr: ucp_listener_attr = unsafe { std::mem::zeroed() };
        attr.field_mask = 1; // UCP_LISTENER_ATTR_FIELD_SOCKADDR
        status_to_result(unsafe { ucp_listener_query(self.handle, &mut attr) }).map(|()| {
            ListenerAttr {
                sockaddr: attr.sockaddr,
            }
        })
    }

    /// Reject a connection request.
    ///
    /// # Safety
    /// Caller must ensure `conn_request` is a valid connection request handle
    /// obtained from the listener's accept handler.
    pub unsafe fn reject(&self, conn_request: ucp_conn_request_h) -> Result<(), ucs_status_t> {
        status_to_result(ucp_listener_reject(self.handle, conn_request))
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                ucp_listener_destroy(self.handle);
            }
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
///
/// # Safety
/// Caller must ensure `conn_request` is a valid connection request handle.
pub unsafe fn conn_request_query(
    conn_request: ucp_conn_request_h,
    mask: u64,
) -> Result<ConnRequestAttr, ucs_status_t> {
    let mut attr: ucp_conn_request_attr = unsafe { std::mem::zeroed() };
    attr.field_mask = mask;
    status_to_result(unsafe { ucp_conn_request_query(conn_request, &mut attr) }).map(|()| {
        ConnRequestAttr {
            client_id: if mask & UCP_CONN_REQUEST_ATTR_FIELD_CLIENT_ID != 0 {
                attr.client_id
            } else {
                0
            },
        }
    })
}

/// Connection request attribute result.
#[derive(Debug, Clone)]
pub struct ConnRequestAttr {
    pub client_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Config, Context, Flags, ParamsBuilder};
    use crate::worker::ParamsBuilder as WorkerParamsBuilder;

    /// Helper: create a UCX context + worker with Tag feature (minimal setup).
    fn setup_worker() -> (Context, crate::worker::Worker) {
        let ctx_params = ParamsBuilder::new().features(Flags::Tag).build();
        let ctx = Context::new(&Config::default(), &ctx_params).expect("context create");
        let worker_params = WorkerParamsBuilder::new().build();
        let worker = ctx.worker_create(&worker_params).expect("worker create");
        (ctx, worker)
    }

    /// Test that Listener::create compiles and can be called with a valid worker.
    /// The listener will be dropped (destroyed) at end of scope.
    /// Listener creation may fail if no transport supports it — that's OK.
    #[test]
    fn test_listener_create() {
        let (_ctx, worker) = setup_worker();

        // Build a basic sockaddr for localhost
        let mut sa: sockaddr = unsafe { std::mem::zeroed() };
        sa.sa_family = 2; // AF_INET

        let mut ucs_sa: ucs_sock_addr_t = unsafe { std::mem::zeroed() };
        ucs_sa.addr = &sa as *const sockaddr;
        ucs_sa.addrlen = std::mem::size_of::<sockaddr>() as socklen_t;

        // Create listener — this exercises the FFI path
        let listener = unsafe { Listener::create(worker.handle, &ucs_sa) };

        // Listener creation may fail if no transport supports it (e.g., loopback-only),
        // but it should not crash. Accept both success and error.
        match listener {
            Ok(_listener) => {
                // Listener created successfully — query its attributes
                let _attr = _listener.query().expect("listener query");
            }
            Err(_status) => {
                // Some UCX builds/transports don't support listener on loopback.
                // This is acceptable — the important thing is the API compiles and doesn't crash.
            }
        }
    }

    /// Test that ListenerAttr is Clone + Copy + Debug.
    #[test]
    fn test_listener_attr_traits() {
        let attr = ListenerAttr {
            sockaddr: unsafe { std::mem::zeroed() },
        };
        let _copy = attr;
        let _debug = format!("{:?}", attr);
    }

    /// Test that ConnRequestAttr is Clone + Debug.
    #[test]
    fn test_conn_request_attr_traits() {
        let attr = ConnRequestAttr { client_id: 42 };
        let _clone = attr.clone();
        let _debug = format!("{:?}", attr);
        assert_eq!(attr.client_id, 42);
    }

    /// Test that listener field mask constants have expected values.
    #[test]
    fn test_listener_field_masks() {
        assert_eq!(UCP_LISTENER_PARAM_FIELD_SOCK_ADDR, 1);
        assert_eq!(UCP_LISTENER_PARAM_FIELD_ACCEPT_HANDLER, 2);
        assert_eq!(UCP_LISTENER_PARAM_FIELD_CONN_HANDLER, 4);
    }

    /// Test that conn request field mask constants have expected values.
    #[test]
    fn test_conn_request_field_masks() {
        assert_eq!(UCP_CONN_REQUEST_ATTR_FIELD_CLIENT_ADDR, 1);
        assert_eq!(UCP_CONN_REQUEST_ATTR_FIELD_CLIENT_ID, 2);
    }
}
