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
