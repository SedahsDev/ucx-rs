use crate::ffi::*;
use bitflags::bitflags;
use libc::{sockaddr_in, sockaddr_in6};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct EpAttrFields: u64 {
        const NAME = 1 << 0;
        const LOCAL_SOCKADDR = 1 << 1;
        const REMOTE_SOCKADDR = 1 << 2;
        const TRANSPORTS = 1 << 3;
        const USER_DATA = 1 << 4;
    }
}

pub struct EpAttr {
    pub name: String,
    pub local_sockaddr: Option<SockAddrStorage>,
    pub remote_sockaddr: Option<SockAddrStorage>,
    pub transports: Option<Transports>,
    pub user_data: Option<*mut std::os::raw::c_void>,
}

/// This is the native wrapper used in place of the raw C `sockaddr_storage`
/// type in public APIs.
#[derive(Clone, Copy)]
pub struct SockAddrStorage(pub(crate) sockaddr_storage);

impl std::fmt::Debug for SockAddrStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SockAddrStorage")
            .field("family", &self.0.ss_family)
            .finish()
    }
}

impl From<sockaddr_storage> for SockAddrStorage {
    fn from(value: sockaddr_storage) -> Self {
        SockAddrStorage(value)
    }
}

/// Native wrapper around the transport list returned by a query.
/// Holds the raw FFI object; the caller supplies the reference for reads.
#[derive(Clone, Copy)]
pub struct Transports(pub(crate) ucp_transports_t);

impl std::fmt::Debug for Transports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transports")
            .field("num_entries", &self.0.num_entries)
            .finish()
    }
}

impl From<ucp_transports_t> for Transports {
    fn from(value: ucp_transports_t) -> Self {
        Transports(value)
    }
}

/// An owned socket address used to bind endpoints and listeners.
/// Owns the backing storage so the address stays valid while it is alive.
pub struct SockAddr {
    storage: SocketStorage,
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

impl SockAddr {
    /// Build a native socket address from a standard Rust `SocketAddr`.
    pub fn new(addr: &std::net::SocketAddr) -> Self {
        match addr {
            std::net::SocketAddr::V4(v4) => {
                let sin = sockaddr_in {
                    sin_family: libc::AF_INET as _,
                    sin_port: v4.port().to_be(),
                    sin_addr: libc::in_addr {
                        s_addr: u32::from_ne_bytes(v4.ip().octets()),
                    },
                    sin_zero: [0; 8],
                };
                SockAddr { storage: SocketStorage::V4(Box::new(sin)) }
            }
            std::net::SocketAddr::V6(v6) => {
                let sin6 = sockaddr_in6 {
                    sin6_family: libc::AF_INET6 as _,
                    sin6_port: v6.port().to_be(),
                    sin6_flowinfo: v6.flowinfo().to_be(),
                    sin6_addr: libc::in6_addr { s6_addr: v6.ip().octets() },
                    sin6_scope_id: v6.scope_id(),
                };
                SockAddr { storage: SocketStorage::V6(Box::new(sin6)) }
            }
        }
    }

    /// The raw C address struct, valid for as long as `self` is alive.
    pub(crate) fn to_ffi(&self) -> ucs_sock_addr_t {
        ucs_sock_addr_t {
            addr: self.storage.as_ptr(),
            addrlen: self.storage.len(),
        }
    }
}

impl SocketStorage {
    fn len(&self) -> u32 {
        match self {
            Self::V4(_) => std::mem::size_of::<sockaddr_in>() as u32,
            Self::V6(_) => std::mem::size_of::<sockaddr_in6>() as u32,
        }
    }
}
