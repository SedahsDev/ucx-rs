//! UCP listeners and incoming connection requests.

use crate::ffi::*;
use crate::status_to_result;
use crate::worker::Worker;
use bitflags::bitflags;
use libc::{sockaddr_in, sockaddr_in6};
use std::mem::MaybeUninit;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ptr;

bitflags! {
    /// Fields accepted by [`ParamsBuilder`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ListenerParamFields: u64 {
        const SOCK_ADDR = ucp_listener_params_field::UCP_LISTENER_PARAM_FIELD_SOCK_ADDR as u64;
        const ACCEPT_HANDLER = ucp_listener_params_field::UCP_LISTENER_PARAM_FIELD_ACCEPT_HANDLER as u64;
        const CONN_HANDLER = ucp_listener_params_field::UCP_LISTENER_PARAM_FIELD_CONN_HANDLER as u64;
    }
}

pub const UCP_LISTENER_PARAM_FIELD_SOCK_ADDR: u64 = ListenerParamFields::SOCK_ADDR.bits();
pub const UCP_LISTENER_PARAM_FIELD_ACCEPT_HANDLER: u64 = ListenerParamFields::ACCEPT_HANDLER.bits();
pub const UCP_LISTENER_PARAM_FIELD_CONN_HANDLER: u64 = ListenerParamFields::CONN_HANDLER.bits();

/// Builder for the raw UCX listener parameter structure.
#[derive(Debug)]
pub struct ParamsBuilder {
    params: ucp_listener_params,
}

/// Compatibility name for code that treats listener parameters as a value.
pub type ListenerParams = ParamsBuilder;
pub type ListenerParamsBuilder = ParamsBuilder;

impl ParamsBuilder {
    pub fn new() -> Self {
        // SAFETY: ucp_listener_params is a C POD; zero is the documented
        // default for optional callbacks and unused fields.
        let params = unsafe { MaybeUninit::<ucp_listener_params>::zeroed().assume_init() };
        Self { params }
    }

    pub fn sockaddr(mut self, addr: ucs_sock_addr_t) -> Self {
        self.params.field_mask |= UCP_LISTENER_PARAM_FIELD_SOCK_ADDR;
        self.params.sockaddr = addr;
        self
    }

    pub fn accept_handler(
        mut self,
        callback: ucp_listener_accept_callback_t,
        arg: *mut std::ffi::c_void,
    ) -> Self {
        self.params.field_mask |= UCP_LISTENER_PARAM_FIELD_ACCEPT_HANDLER;
        self.params.accept_handler = ucp_listener_accept_handler { cb: callback, arg };
        self
    }

    pub fn conn_handler(
        mut self,
        callback: ucp_listener_conn_callback_t,
        arg: *mut std::ffi::c_void,
    ) -> Self {
        self.params.field_mask |= UCP_LISTENER_PARAM_FIELD_CONN_HANDLER;
        self.params.conn_handler = ucp_listener_conn_handler { cb: callback, arg };
        self
    }

    pub fn build(self) -> ucp_listener_params {
        self.params
    }

    /// Set a callback and opaque argument supplied directly to UCX.
    ///
    /// # Safety
    /// The callback and argument must remain valid for every invocation by UCX.
    pub unsafe fn raw_accept_handler(
        self,
        callback: ucp_listener_accept_callback_t,
        arg: *mut std::ffi::c_void,
    ) -> Self {
        self.accept_handler(callback, arg)
    }

    /// Set a connection callback and opaque argument supplied directly to UCX.
    ///
    /// # Safety
    /// The callback and argument must remain valid for every invocation by UCX.
    pub unsafe fn raw_conn_handler(
        self,
        callback: ucp_listener_conn_callback_t,
        arg: *mut std::ffi::c_void,
    ) -> Self {
        self.conn_handler(callback, arg)
    }
}

impl Default for ParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII wrapper for a UCP listener.
pub struct Listener {
    handle: ucp_listener_h,
}

impl Listener {
    /// Create a listener bound to `addr`.
    pub fn create(worker: &Worker, addr: &SocketAddr) -> Result<Self, ucs_status_t> {
        let (_storage, sockaddr) = socket_address(addr);
        let params = ParamsBuilder::new().sockaddr(sockaddr).build();
        let mut handle = ptr::null_mut();
        // `storage` keeps the address backing memory alive through the call;
        // UCX copies the address as part of listener creation.
        let result =
            status_to_result(unsafe { ucp_listener_create(worker.handle, &params, &mut handle) });
        result.map(|()| Self { handle })
    }

    /// Create a listener from explicitly-built parameters.
    pub fn create_with_params(
        worker: &Worker,
        params: &ParamsBuilder,
    ) -> Result<Self, ucs_status_t> {
        let params = params.clone_params();
        let mut handle = ptr::null_mut();
        status_to_result(unsafe { ucp_listener_create(worker.handle, &params, &mut handle) })
            .map(|()| Self { handle })
    }

    pub fn as_raw(&self) -> ucp_listener_h {
        self.handle
    }

    pub fn query(&self) -> Result<ListenerAttr, ucs_status_t> {
        // SAFETY: zeroed C POD, then UCX fills the requested field.
        let mut attr = unsafe { MaybeUninit::<ucp_listener_attr>::zeroed().assume_init() };
        attr.field_mask = ucp_listener_attr_field::UCP_LISTENER_ATTR_FIELD_SOCKADDR as u64;
        // SAFETY: self.handle is live and attr is a valid writable UCX struct.
        status_to_result(unsafe { ucp_listener_query(self.handle, &mut attr) }).map(|()| {
            ListenerAttr {
                sockaddr: attr.sockaddr,
                socket_addr: from_storage(&attr.sockaddr),
            }
        })
    }

    pub fn reject(&self, request: &ConnRequest) -> Result<(), ucs_status_t> {
        // SAFETY: request is a live handle delivered by UCX to the callback.
        status_to_result(unsafe { ucp_listener_reject(self.handle, request.handle) })
    }
}

impl ParamsBuilder {
    fn clone_params(&self) -> ucp_listener_params {
        self.params
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: handle was returned by UCX and is destroyed exactly once.
            unsafe { ucp_listener_destroy(self.handle) };
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ListenerAttr {
    pub sockaddr: sockaddr_storage,
    pub socket_addr: Option<SocketAddr>,
}

bitflags! {
    /// Fields that may be requested from a connection request.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ConnRequestFields: u64 {
        const CLIENT_ADDR = ucp_conn_request_attr_field::UCP_CONN_REQUEST_ATTR_FIELD_CLIENT_ADDR as u64;
        const CLIENT_ID = ucp_conn_request_attr_field::UCP_CONN_REQUEST_ATTR_FIELD_CLIENT_ID as u64;
    }
}

pub const UCP_CONN_REQUEST_ATTR_FIELD_CLIENT_ADDR: u64 = ConnRequestFields::CLIENT_ADDR.bits();
pub const UCP_CONN_REQUEST_ATTR_FIELD_CLIENT_ID: u64 = ConnRequestFields::CLIENT_ID.bits();

/// A borrowed UCX connection request handle.
#[derive(Debug, Clone, Copy)]
pub struct ConnRequest {
    handle: ucp_conn_request_h,
}

impl ConnRequest {
    /// # Safety
    /// `handle` must be a valid request supplied by a UCX listener callback.
    pub unsafe fn from_raw(handle: ucp_conn_request_h) -> Self {
        Self { handle }
    }
    pub fn as_raw(&self) -> ucp_conn_request_h {
        self.handle
    }
    pub fn reject(&self, listener: &Listener) -> Result<(), ucs_status_t> {
        listener.reject(self)
    }
    pub fn query(&self, fields: ConnRequestFields) -> Result<ConnRequestAttr, ucs_status_t> {
        // SAFETY: zeroed C POD, then UCX fills exactly the requested fields.
        let mut attr = unsafe { MaybeUninit::<ucp_conn_request_attr>::zeroed().assume_init() };
        attr.field_mask = fields.bits();
        // SAFETY: handle is guaranteed by ConnRequest's constructor contract.
        status_to_result(unsafe { ucp_conn_request_query(self.handle, &mut attr) }).map(|()| {
            ConnRequestAttr {
                client_address: fields
                    .contains(ConnRequestFields::CLIENT_ADDR)
                    .then(|| from_storage(&attr.client_address))
                    .flatten(),
                client_id: fields
                    .contains(ConnRequestFields::CLIENT_ID)
                    .then_some(attr.client_id),
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct ConnRequestAttr {
    pub client_address: Option<SocketAddr>,
    pub client_id: Option<u64>,
}

/// Compatibility helper for callers that already have a raw UCX handle.
///
/// # Safety
/// `conn_request` must be a valid UCX connection request handle.
pub unsafe fn conn_request_query(
    conn_request: ucp_conn_request_h,
    fields: ConnRequestFields,
) -> Result<ConnRequestAttr, ucs_status_t> {
    ConnRequest::from_raw(conn_request).query(fields)
}

fn socket_address(addr: &SocketAddr) -> (SocketStorage, ucs_sock_addr_t) {
    match addr {
        SocketAddr::V4(v4) => {
            let sin = sockaddr_in {
                sin_family: libc::AF_INET as _,
                sin_port: v4.port().to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes(v4.ip().octets()),
                },
                sin_zero: [0; 8],
            };
            let storage = SocketStorage::V4(Box::new(sin));
            let sa = ucs_sock_addr_t {
                addr: storage.as_ptr(),
                addrlen: std::mem::size_of::<sockaddr_in>() as _,
            };
            (storage, sa)
        }
        SocketAddr::V6(v6) => {
            let sin6 = sockaddr_in6 {
                sin6_family: libc::AF_INET6 as _,
                sin6_port: v6.port().to_be(),
                sin6_flowinfo: v6.flowinfo(),
                sin6_addr: libc::in6_addr {
                    s6_addr: v6.ip().octets(),
                },
                sin6_scope_id: v6.scope_id(),
            };
            let storage = SocketStorage::V6(Box::new(sin6));
            let sa = ucs_sock_addr_t {
                addr: storage.as_ptr(),
                addrlen: std::mem::size_of::<sockaddr_in6>() as _,
            };
            (storage, sa)
        }
    }
}

enum SocketStorage {
    V4(Box<sockaddr_in>),
    V6(Box<sockaddr_in6>),
}
impl SocketStorage {
    fn as_ptr(&self) -> *const sockaddr {
        match self {
            Self::V4(v) => v.as_ref() as *const _ as _,
            Self::V6(v) => v.as_ref() as *const _ as _,
        }
    }
}

fn from_storage(storage: &sockaddr_storage) -> Option<SocketAddr> {
    // SAFETY: UCX stores a sockaddr with the family in the first field; the
    // casts are guarded by family and match the corresponding C layouts.
    unsafe {
        match storage.ss_family as i32 {
            libc::AF_INET => {
                let v = &*(storage as *const _ as *const sockaddr_in);
                Some(SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::from(u32::from_be(v.sin_addr.s_addr))),
                    u16::from_be(v.sin_port),
                ))
            }
            libc::AF_INET6 => {
                let v = &*(storage as *const _ as *const sockaddr_in6);
                Some(SocketAddr::new(
                    IpAddr::V6(Ipv6Addr::from(v.sin6_addr.s6_addr)),
                    u16::from_be(v.sin6_port),
                ))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn masks_and_ipv4_conversion() {
        assert_eq!(UCP_LISTENER_PARAM_FIELD_SOCK_ADDR, 1);
        let addr = SocketAddr::from(([127, 0, 0, 1], 42));
        let (_storage, raw) = socket_address(&addr);
        assert_eq!(raw.addrlen as usize, std::mem::size_of::<sockaddr_in>());
        let mut ss = unsafe { MaybeUninit::<sockaddr_storage>::zeroed().assume_init() };
        unsafe {
            ptr::copy_nonoverlapping(
                raw.addr as *const u8,
                &mut ss as *mut _ as *mut u8,
                raw.addrlen as usize,
            );
        }
        assert_eq!(from_storage(&ss), Some(addr));
    }
}
