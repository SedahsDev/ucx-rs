use crate::ffi::*;
use bitflags::bitflags;

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
    pub local_sockaddr: Option<sockaddr_storage>,
    pub remote_sockaddr: Option<sockaddr_storage>,
    pub transports: Option<ucp_transports_t>,
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

