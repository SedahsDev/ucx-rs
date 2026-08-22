use crate::ep::Ep;
use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Request;
use crate::RequestParam;
use bitflags::bitflags;

impl Ep {
    /// Send tagged message.
    pub fn tag_send(
        &self,
        data: &[u8],
        tag: u64,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_tag_send_nbx(
                self.handle,
                data.as_ptr() as _,
                data.len(),
                tag,
                &param.handle,
            )
        })
    }

    /// Tag send with synchronous completion (safe wrapper).
    ///
    /// Guarantees remote delivery before the request completes.
    pub fn tag_send_sync(&self, data: &[u8], tag: u64) -> Request {
        unsafe {
            let ptr = ucp_tag_send_sync_nbx(
                self.handle,
                data.as_ptr() as _,
                data.len(),
                tag,
                std::ptr::null(),
            );
            Request::from_raw(ptr)
        }
    }
}

pub struct MessageHandle {
    pub(crate) handle: ucp_tag_message_h,
    pub(crate) info: ucp_tag_recv_info_t,
    removed: bool,
}

impl MessageHandle {
    pub fn len(&self) -> usize {
        self.info.length
    }

    pub fn is_empty(&self) -> bool {
        self.info.length == 0
    }

    pub fn sender_tag(&self) -> u64 {
        self.info.sender_tag
    }
}

pub struct TagInfo {
    pub(crate) handle: ucp_tag_recv_info_t,
}

impl TagInfo {
    pub fn len(&self) -> usize {
        self.handle.length
    }

    pub fn is_empty(&self) -> bool {
        self.handle.length == 0
    }

    pub fn sender_tag(&self) -> u64 {
        self.handle.sender_tag
    }
}

impl Worker {
    pub fn tag_recv(
        &self,
        data: &mut [u8],
        tag: u64,
        mask: u64,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        status_ptr_to_result(unsafe {
            ucp_tag_recv_nbx(
                self.handle,
                data.as_ptr() as _,
                data.len(),
                tag,
                mask,
                &param.handle,
            )
        })
    }

    pub fn tag_probe(&self, tag: u64, tag_mask: u64, remove: bool) -> Option<MessageHandle> {
        let mut info = std::mem::MaybeUninit::<ucp_tag_recv_info_t>::uninit();
        let handle = unsafe {
            ucp_tag_probe_nb(self.handle, tag, tag_mask, remove as i32, info.as_mut_ptr())
        };

        if !handle.is_null() {
            Some(MessageHandle {
                handle,
                info: unsafe { info.assume_init() },
                removed: remove,
            })
        } else {
            None
        }
    }

    pub fn tag_msg_recv(
        &self,
        data: &mut [u8],
        message: &MessageHandle,
        param: &RequestParam,
    ) -> Result<Option<Request>, ucs_status_t> {
        if !message.removed {
            panic!("Tried to call tag_msg_recv() on a MessageHandle that didn't remove the entry!");
        }
        status_ptr_to_result(unsafe {
            ucp_tag_msg_recv_nbx(
                self.handle,
                data.as_ptr() as _,
                data.len(),
                message.handle,
                &param.handle,
            )
        })
    }
}

fn map_tag_recv_test_status(
    status: ucs_status_t,
    info: std::mem::MaybeUninit<ucp_tag_recv_info_t>,
) -> Result<Option<TagInfo>, ucs_status_t> {
    match status {
        ucs_status_t::UCS_OK => Ok(Some(TagInfo {
            // UCX initializes `info` only for a completed request.
            handle: unsafe { info.assume_init() },
        })),
        ucs_status_t::UCS_INPROGRESS => Ok(None),
        _ => Err(status),
    }
}

impl Request {
    /// Test a tagged receive request and return its receive information when complete.
    ///
    /// `Ok(Some(info))` means the receive completed and `info` contains the sender tag
    /// and length reported by UCX. `Ok(None)` means the request is still in progress.
    /// A request that was already freed or cancelled also returns `Ok(None)`: it can
    /// no longer complete, but there is no receive information to report.
    ///
    /// Callers should not use this method after the request has completed and the
    /// request has been freed.
    pub fn tag_recv_test(&mut self) -> Result<Option<TagInfo>, ucs_status_t> {
        let Some(h) = self.handle else {
            return Ok(None);
        };
        let mut info = std::mem::MaybeUninit::<ucp_tag_recv_info_t>::uninit();
        let status = unsafe { ucp_tag_recv_request_test(h.as_ptr(), info.as_mut_ptr()) };
        map_tag_recv_test_status(status, info)
    }
}

/// Tag send with synchronous completion.
///
/// # Safety
/// Caller must ensure `buffer` is valid for `count` bytes.
#[deprecated(since = "0.1.0", note = "Use Ep::tag_send_sync() instead")]
pub unsafe fn tag_send_sync_nbx(
    ep: ucp_ep_h,
    buffer: *const std::os::raw::c_void,
    count: usize,
    tag: ucp_tag_t,
) -> crate::Request {
    let ptr = ucp_tag_send_sync_nbx(ep, buffer, count, tag, std::ptr::null());
    crate::Request::from_raw(ptr)
}

/// Legacy tag message receive (non-nbx variant).
///
/// # Safety
/// Caller must ensure `buffer` has space for `count` elements of `datatype`.
pub unsafe fn tag_msg_recv_nb(
    worker: ucp_worker_h,
    buffer: *mut std::os::raw::c_void,
    count: usize,
    datatype: ucp_datatype_t,
    message: ucp_tag_message_h,
) -> crate::Request {
    let ptr = ucp_tag_msg_recv_nb(worker, buffer, count, datatype, message, None);
    crate::Request::from_raw(ptr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context;
    use crate::context::Context;
    use crate::ep;
    use crate::tests::setup_default;
    use crate::tests::CommsContext;
    use crate::worker;
    use crate::worker::RemoteWorkerAddress;
    use crate::RequestParamBuilder;
    use std::rc::Rc;

    const TAG_FULL: u64 = u64::MAX;

    #[test]
    fn tag_recv_test_maps_completed_status_to_info() {
        let info = ucp_tag_recv_info_t {
            sender_tag: 0xfeed,
            length: 37,
        };

        let result =
            map_tag_recv_test_status(ucs_status_t::UCS_OK, std::mem::MaybeUninit::new(info));

        let info = result
            .expect("completed status should succeed")
            .expect("completed status should include info");
        assert_eq!(info.sender_tag(), 0xfeed);
        assert_eq!(info.len(), 37);
    }

    #[test]
    fn tag_recv_test_maps_inprogress_status_to_none() {
        let result = map_tag_recv_test_status(
            ucs_status_t::UCS_INPROGRESS,
            std::mem::MaybeUninit::uninit(),
        );

        assert!(result.expect("in-progress status should succeed").is_none());
    }

    #[test]
    fn tag_recv_test_maps_error_status_to_error() {
        let result = map_tag_recv_test_status(
            ucs_status_t::UCS_ERR_IO_ERROR,
            std::mem::MaybeUninit::uninit(),
        );

        assert!(matches!(result, Err(ucs_status_t::UCS_ERR_IO_ERROR)));
    }

    /// Exercises the real UCX completion path with a self-endpoint tag exchange.
    #[test]
    fn tag_recv_test_real_completed_request() {
        let comms = setup_default();
        let mut recv_buffer = [0u8; 5];
        let tag = 0x1234;
        let param = RequestParamBuilder::new().no_imm_cmpl().build();
        let mut recv_request = comms
            .worker
            .tag_recv(&mut recv_buffer, tag, u64::MAX, &param)
            .expect("post tag receive")
            .expect("receive should remain outstanding");
        let send_buffer = *b"hello";
        let send_request = comms
            .ep
            .tag_send(&send_buffer, tag, &param)
            .expect("post tag send");

        let info = loop {
            if let Some(info) = recv_request.tag_recv_test().expect("test tag receive") {
                break info;
            }
            comms.worker.progress();
        };

        assert_eq!(info.sender_tag(), tag);
        assert_eq!(info.len(), send_buffer.len());
        assert_eq!(recv_buffer, send_buffer);
        drop(send_request);
    }

    #[test]
    fn tag_send() {
        let comms = setup_default();
        let mut recv_buff = vec![0];
        let send_buff = vec![32];
        let tag_flags = RequestParamBuilder::new().no_imm_cmpl().build();
        let _send_req = comms
            .ep
            .tag_send(send_buff.as_slice(), TAG_FULL, &tag_flags)
            .unwrap();
        let recv_req = comms
            .worker
            .tag_recv(recv_buff.as_mut_slice(), TAG_FULL, TAG_FULL, &tag_flags)
            .unwrap()
            .unwrap();
        while !recv_req.check_finished().unwrap() {
            comms.worker.progress();
        }
        assert_eq!(send_buff[0], recv_buff[0]);
    }

    #[test]
    fn tag_probe() {
        let comms = setup_default();
        let mut recv_buff = vec![0];
        let send_buff = vec![32];
        let tag_flags = RequestParamBuilder::new().no_imm_cmpl().build();
        let _send_req = comms
            .ep
            .tag_send(send_buff.as_slice(), TAG_FULL, &tag_flags)
            .unwrap();
        let mut msg = comms.worker.tag_probe(TAG_FULL, TAG_FULL, true);
        while msg.is_none() {
            comms.worker.progress();
            msg = comms.worker.tag_probe(TAG_FULL, TAG_FULL, true);
        }
        let msg = msg.unwrap();
        let recv_req = comms
            .worker
            .tag_msg_recv(recv_buff.as_mut_slice(), &msg, &tag_flags)
            .unwrap()
            .unwrap();
        while !recv_req.check_finished().unwrap() {
            comms.worker.progress();
        }
        assert_eq!(send_buff[0], recv_buff[0]);
    }
}
