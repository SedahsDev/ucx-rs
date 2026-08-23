//! UCP listeners and incoming connection requests.

use crate::ffi::*;
use crate::status_to_result;
use crate::worker::Worker;
use bitflags::bitflags;
use libc::{sockaddr_in, sockaddr_in6};
use std::mem::MaybeUninit;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ptr;
use std::sync::{Arc, Mutex};

struct ConnHandler {
    inner: Mutex<Box<dyn FnMut(ConnRequest) + Send + 'static>>,
    state: Arc<ListenerState>,
}

#[derive(Debug)]
struct ListenerState {
    // Raw UCX handles are opaque pointers and are not Send/Sync in Rust.
    // Carrying the address as usize lets this state cross UCX progress threads.
    handle: Mutex<usize>,
}

impl Drop for ListenerState {
    fn drop(&mut self) {
        let handle = match self.handle.lock() {
            Ok(handle) => *handle as ucp_listener_h,
            Err(poisoned) => *poisoned.into_inner() as ucp_listener_h,
        };
        if !handle.is_null() {
            // SAFETY: the state owns the handle and is dropped only after the
            // Listener and all callback-delivered ConnRequests are gone.
            unsafe { ucp_listener_destroy(handle) };
        }
    }
}

unsafe extern "C" fn conn_trampoline(conn_request: ucp_conn_request_h, arg: *mut std::ffi::c_void) {
    // SAFETY: `arg` points to the ConnHandler held by Listener for the entire
    // lifetime of the UCX listener. UCX supplies a live request handle for
    // this callback; ConnRequest owns the callback delivery and must be used
    // before the callback returns according to UCX's request lifetime rules.
    let handler = unsafe { &*(arg as *const ConnHandler) };
    let request = ConnRequest {
        handle: conn_request,
        state: Arc::clone(&handler.state),
    };
    let mut callback = match handler.inner.lock() {
        Ok(callback) => callback,
        // A poisoned handler cannot be trusted. The request is dropped without
        // invoking user code, and the callback returns to UCX normally.
        Err(_) => return,
    };
    // Handler panics are contained here and never unwind through this
    // extern "C" trampoline into UCX.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(request)));
}

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

    /// Set the raw callback and opaque argument supplied directly to UCX.
    ///
    /// # Safety
    /// The callback and argument must remain valid for the listener's entire
    /// lifetime and for every invocation by UCX.
    pub unsafe fn accept_handler(
        mut self,
        callback: ucp_listener_accept_callback_t,
        arg: *mut std::ffi::c_void,
    ) -> Self {
        self.params.field_mask |= UCP_LISTENER_PARAM_FIELD_ACCEPT_HANDLER;
        self.params.accept_handler = ucp_listener_accept_handler { cb: callback, arg };
        self
    }

    /// Set the raw connection callback and opaque argument supplied directly
    /// to UCX.
    ///
    /// # Safety
    /// The callback and argument must remain valid for the listener's entire
    /// lifetime and for every invocation by UCX.
    pub unsafe fn conn_handler(
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
}

impl Default for ParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII wrapper for a UCP listener.
pub struct Listener {
    state: Arc<ListenerState>,
    #[allow(dead_code)]
    conn_handler: Option<Arc<ConnHandler>>,
}

impl Listener {
    /// Create a listener bound to `addr`.
    ///
    /// Any UCX callback configured through the parameters runs in the progress
    /// context: the thread calling [`Worker::progress`], or UCX-internal
    /// progress under MULTI. Never block or call back into the same worker from
    /// a handler; hop heavy work to an application thread or channel. See
    /// `THREADING.md` section 4.
    pub fn create(worker: &Worker, addr: &SocketAddr) -> Result<Self, ucs_status_t> {
        let (_storage, sockaddr) = socket_address(addr);
        let params = ParamsBuilder::new().sockaddr(sockaddr).build();
        let mut handle = ptr::null_mut();
        // `storage` keeps the address backing memory alive through the call;
        // UCX copies the address as part of listener creation.
        let result =
            status_to_result(unsafe { ucp_listener_create(worker.handle, &params, &mut handle) });
        result.map(|()| Self {
            state: Arc::new(ListenerState {
                handle: Mutex::new(handle as usize),
            }),
            conn_handler: None,
        })
    }

    /// Create a listener from explicitly-built parameters. Callback execution
    /// follows the progress-context rules documented on [`Self::create`].
    pub fn create_with_params(
        worker: &Worker,
        params: &ParamsBuilder,
    ) -> Result<Self, ucs_status_t> {
        let params = params.clone_params();
        let mut handle = ptr::null_mut();
        status_to_result(unsafe { ucp_listener_create(worker.handle, &params, &mut handle) }).map(
            |()| Self {
                state: Arc::new(ListenerState {
                    handle: Mutex::new(handle as usize),
                }),
                conn_handler: None,
            },
        )
    }

    /// Create a listener with a safe connection callback. The callback runs in
    /// the progress context (the thread calling [`Worker::progress`], or UCX's
    /// internal progress thread under MULTI). Do not block or call back into
    /// the same worker; send heavy work to an application thread or channel.
    ///
    /// The delivered [`ConnRequest`] retains the listener state, so it may be
    /// safely moved out of the callback and rejected later. This also keeps the
    /// UCX listener alive until every delivered request has been dropped.
    pub fn create_with_conn_handler<F>(
        worker: &Worker,
        addr: &SocketAddr,
        handler: F,
    ) -> Result<Self, ucs_status_t>
    where
        F: FnMut(ConnRequest) + Send + 'static,
    {
        let conn_handler = Arc::new(ConnHandler {
            inner: Mutex::new(Box::new(handler)),
            state: Arc::new(ListenerState {
                handle: Mutex::new(0),
            }),
        });
        let (_storage, sockaddr) = socket_address(addr);
        // SAFETY: the callback and its Arc-backed argument are retained by
        // the returned listener until after UCX listener destruction.
        let params = unsafe {
            ParamsBuilder::new()
                .sockaddr(sockaddr)
                .conn_handler(Some(conn_trampoline), Arc::as_ptr(&conn_handler) as *mut _)
                .build()
        };
        let mut handle = ptr::null_mut();
        match status_to_result(unsafe { ucp_listener_create(worker.handle, &params, &mut handle) })
        {
            Ok(()) => {
                *conn_handler
                    .state
                    .handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = handle as usize;
                Ok(Self {
                    state: Arc::clone(&conn_handler.state),
                    conn_handler: Some(conn_handler),
                })
            }
            Err(error) => Err(error),
        }
    }

    pub fn as_raw(&self) -> ucp_listener_h {
        *self
            .state
            .handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) as ucp_listener_h
    }

    pub fn query(&self) -> Result<ListenerAttr, ucs_status_t> {
        // SAFETY: zeroed C POD, then UCX fills the requested field.
        let mut attr = unsafe { MaybeUninit::<ucp_listener_attr>::zeroed().assume_init() };
        attr.field_mask = ucp_listener_attr_field::UCP_LISTENER_ATTR_FIELD_SOCKADDR as u64;
        // SAFETY: self.handle is live and attr is a valid writable UCX struct.
        status_to_result(unsafe { ucp_listener_query(self.as_raw(), &mut attr) }).map(|()| {
            ListenerAttr {
                sockaddr: attr.sockaddr,
                socket_addr: from_storage(&attr.sockaddr),
            }
        })
    }

    /// Reject a connection request, consuming its single-use handle.
    pub fn reject(&self, request: ConnRequest) -> Result<(), ucs_status_t> {
        // SAFETY: request is a live handle delivered by UCX to the callback.
        status_to_result(unsafe { ucp_listener_reject(self.as_raw(), request.handle) })
    }
}

impl ParamsBuilder {
    fn clone_params(&self) -> ucp_listener_params {
        self.params
    }
}

impl Drop for Listener {
    fn drop(&mut self) {}
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
#[derive(Debug)]
pub struct ConnRequest {
    handle: ucp_conn_request_h,
    state: Arc<ListenerState>,
}

impl ConnRequest {
    /// # Safety
    /// `handle` must be a valid request supplied by a UCX listener callback.
    pub unsafe fn from_raw(handle: ucp_conn_request_h) -> Self {
        Self {
            handle,
            state: Arc::new(ListenerState {
                handle: Mutex::new(0),
            }),
        }
    }
    pub fn as_raw(&self) -> ucp_conn_request_h {
        self.handle
    }
    /// Reject this connection request, consuming its single-use handle.
    pub fn reject(self, listener: &Listener) -> Result<(), ucs_status_t> {
        listener.reject(self)
    }
    /// Reject this callback-delivered request using its originating listener.
    /// The retained listener state makes this safe after the [`Listener`] is
    /// dropped. If the callback raced listener creation before its handle was
    /// published, UCX cannot safely reject the request and invalid-param is
    /// returned instead of dereferencing a null handle.
    pub fn reject_owned(self) -> Result<(), ucs_status_t> {
        let listener = match self.state.handle.lock() {
            Ok(listener) => *listener as ucp_listener_h,
            Err(_) => return Err(ucs_status_t::UCS_ERR_INVALID_PARAM),
        };
        if listener.is_null() {
            return Err(ucs_status_t::UCS_ERR_INVALID_PARAM);
        }
        status_to_result(unsafe { ucp_listener_reject(listener, self.handle) })
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
                sin6_flowinfo: v6.flowinfo().to_be(),
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

    #[test]
    fn ipv6_conversion_round_trip() {
        let addr = SocketAddr::from(([0xfe80, 0, 0, 0, 0, 0, 0, 1], 42));
        let (_storage, raw) = socket_address(&addr);
        assert_eq!(raw.addrlen as usize, std::mem::size_of::<sockaddr_in6>());
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

    #[test]
    fn panicking_connection_handler_is_contained() {
        let handler = Arc::new(ConnHandler {
            inner: Mutex::new(Box::new(|_: ConnRequest| panic!("handler"))),
            state: Arc::new(ListenerState {
                handle: Mutex::new(0),
            }),
        });
        let arg = Arc::as_ptr(&handler) as *mut std::ffi::c_void;
        unsafe { conn_trampoline(ptr::null_mut(), arg) };
    }
}
