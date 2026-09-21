use crate::ffi::*;
use crate::status_to_result;
use crate::worker::Worker;
use std::ptr::NonNull;

#[derive(Debug, Clone)]
pub struct WorkerAddressAttr {
    pub address: *mut ucp_address_t,
    pub length: usize,
}

/// Worker query attribute result. Fields are present only when requested.
#[derive(Debug, Clone)]
pub struct WorkerAttr {
    pub thread_mode: Option<ucs_thread_mode_t>,
    pub address: Option<WorkerAddressAttr>,
    pub address_flags: Option<u32>,
    pub max_am_header: Option<usize>,
    pub name: Option<String>,
    pub max_info_string: Option<usize>,
}

/// Query worker address attributes.
///
/// Field mask: UCP_WORKER_ADDRESS_ATTR_FIELD_UID = 1
pub fn address_query(address: *const ucp_address_t) -> Result<u64, ucs_status_t> {
    let mut attr: ucp_worker_address_attr = unsafe { std::mem::zeroed() };
    attr.field_mask = 1; // UCP_WORKER_ADDRESS_ATTR_FIELD_UID
    crate::status_to_result(unsafe { ucp_worker_address_query(address as *mut _, &mut attr) })
        .map(|()| attr.worker_uid)
}

pub struct RemoteWorkerAddress {
    address: Vec<u8>,
}

impl RemoteWorkerAddress {
    pub fn new(address: Vec<u8>) -> RemoteWorkerAddress {
        RemoteWorkerAddress { address }
    }

    pub fn get_handle(&self) -> (*const ucp_address_t, usize) {
        (
            self.address.as_ptr() as *const ucp_address_t,
            self.address.len(),
        )
    }
}

pub struct WorkerAddress<'a> {
    pub(crate) handle: *const ucp_address_t,
    pub(crate) size: usize,
    pub(crate) parent: &'a Worker,
}

impl WorkerAddress<'_> {
    pub fn to_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.handle as *const u8, self.size) }
    }
    pub fn to_vec(&self) -> Vec<u8> {
        self.to_slice().to_vec()
    }
}

impl Drop for WorkerAddress<'_> {
    fn drop(&mut self) {
        unsafe {
            ucp_worker_release_address(self.parent.handle, self.handle as *mut ucp_address_t)
        };
    }
}
