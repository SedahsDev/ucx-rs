use crate::context::Context;
use crate::ep::Ep;
use crate::ffi::*;
use crate::memh::MemHandle;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Request;
use std::marker::PhantomData;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// A fetch-AMO completion tied to both its worker and reply buffer.
///
/// The reply buffer must remain valid until the request is resolved. Dropping
/// a pending value progresses its worker to completion before releasing the
/// reply borrow, unless the bounded progress loop times out; in that
/// pathological case the request is deliberately leaked and the reply buffer
/// must not be reused.
pub struct FetchAmoRequest<'w, 'a, T> {
    request: Option<crate::Request>,
    worker: &'w Worker,
    _reply: PhantomData<&'a mut T>,
}

impl<'w, 'a, T> FetchAmoRequest<'w, 'a, T> {
    /// Extract the underlying UCX request without completing it.
    ///
    /// # Safety
    /// The returned request may be in flight and still own `reply_buffer` for
    /// the caller-provided reply buffer. The caller must keep the reply buffer
    /// valid until the returned request reaches completion
    /// (`check_finished() == Ok(true)`) and must not drop it (which calls
    /// `ucp_request_free`) while in flight. Prefer `check_finished()` + `free()`
    /// on the wrapper, or let `Drop` handle it.
    pub unsafe fn into_inner(mut self) -> Option<crate::Request> {
        self.request.take()
    }
    pub fn check_finished(&self) -> Result<bool, ucs_status_t> {
        self.request
            .as_ref()
            .map_or(Ok(true), crate::Request::check_finished)
    }
    /// Progress the request to completion before releasing its reply borrow.
    ///
    /// If completion cannot be observed within the bounded progress loop, the
    /// request is deliberately leaked rather than freed while it may still be
    /// in flight. This preserves the same safety contract as [`Drop`].
    pub fn free(mut self) {
        if let Some(request) = self.request.take() {
            const MAX_PROGRESS: usize = 1_000_000;
            for _ in 0..MAX_PROGRESS {
                match request.check_finished() {
                    Ok(true) | Err(_) => {
                        request.free();
                        return;
                    }
                    Ok(false) => {
                        self.worker.progress();
                    }
                }
            }
            // SAFETY: The request is still in flight. It must not be freed;
            // the caller must not reuse the reply buffer after this timeout.
            std::mem::forget(request);
        }
    }
}

impl<T> Drop for FetchAmoRequest<'_, '_, T> {
    fn drop(&mut self) {
        if let Some(request) = self.request.take() {
            const MAX_PROGRESS: usize = 1_000_000;
            for _ in 0..MAX_PROGRESS {
                match request.check_finished() {
                    Ok(true) => {
                        request.free();
                        return;
                    }
                    Ok(false) => {
                        self.worker.progress();
                    }
                    Err(_) => {
                        request.free();
                        return;
                    }
                }
            }
            // SAFETY: The request is still in flight. It must not be freed;
            // the caller must not reuse the reply buffer after this timeout.
            std::mem::forget(request);
        }
    }
}

pub(crate) fn fetch_amo_result<'w, 'a, T>(
    request: Option<crate::Request>,
    worker: &'w Worker,
) -> FetchAmoRequest<'w, 'a, T> {
    FetchAmoRequest {
        request,
        worker,
        _reply: PhantomData,
    }
}

/// RAII wrapper around a UCP remote key handle (`ucp_rkey_h`).
/// The rkey is automatically destroyed when dropped.
pub struct RemoteKey {
    pub(crate) handle: ucp_rkey_h,
    pub(crate) worker_alive: Arc<std::sync::atomic::AtomicBool>,
}

fn frame_rkey_payload(payload: &[u8]) -> Result<Vec<u8>, ucs_status_t> {
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| ucs_status_t::UCS_ERR_OUT_OF_RANGE)?;
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&payload_len.to_le_bytes());
    framed.extend_from_slice(payload);
    Ok(framed)
}

fn unframe_rkey_payload(buffer: &[u8]) -> Result<&[u8], ucs_status_t> {
    if buffer.len() < 4 {
        return Err(ucs_status_t::UCS_ERR_INVALID_PARAM);
    }
    let payload_len = u32::from_le_bytes(buffer[..4].try_into().unwrap()) as usize;
    if payload_len != buffer.len() - 4 {
        return Err(ucs_status_t::UCS_ERR_INVALID_PARAM);
    }
    Ok(&buffer[4..])
}

impl RemoteKey {
    /// Pack a memory handle using the framed format consumed by [`Self::unpack`].
    ///
    /// The wire format is exactly `[4-byte little-endian payload length][payload]`.
    /// The payload is the opaque byte sequence returned by UCX's `ucp_rkey_pack`.
    pub fn pack(context: &Context, memh: &MemHandle) -> Result<Vec<u8>, ucs_status_t> {
        let mut buffer = std::ptr::null_mut();
        let mut size = 0usize;
        status_to_result(unsafe {
            ucp_rkey_pack(context.handle, memh.as_raw(), &mut buffer, &mut size)
        })?;
        let result =
            frame_rkey_payload(unsafe { std::slice::from_raw_parts(buffer as *const u8, size) });
        unsafe { ucp_rkey_buffer_release(buffer) };
        result
    }

    /// Unpack `[4-byte little-endian payload length][opaque UCX payload]`.
    pub fn unpack(ep: &Ep, rkey_buffer: &[u8]) -> Result<RemoteKey, ucs_status_t> {
        let payload = unframe_rkey_payload(rkey_buffer)?;
        let mut rkey: ucp_rkey_h = std::ptr::null_mut();
        status_to_result(unsafe {
            ucp_ep_rkey_unpack(ep.handle, payload.as_ptr() as *const _, &mut rkey)
        })
        .map(|()| RemoteKey {
            handle: rkey,
            worker_alive: Arc::clone(&ep.worker_alive),
        })
    }

    /// Get a local pointer to remote memory for intra-node one-sided access.
    ///
    /// UCX returns only a pointer, so `len` must be the caller's independently
    /// known valid length. The slice is valid only while this key and its remote
    /// allocation remain valid. The returned slice must not overlap any other
    /// live reference to that memory; the caller must ensure no other
    /// `rkey_ptr` slice or local reference aliases it. Remote writes through
    /// UCX RMA/AMO while the slice is borrowed are outside Rust's aliasing model;
    /// synchronize externally and do not hold overlapping `&mut` references
    /// across such operations.
    pub fn rkey_ptr(&mut self, remote_addr: u64, len: usize) -> Result<&mut [u8], ucs_status_t> {
        let mut addr = std::ptr::null_mut();
        status_to_result(unsafe { ucp_rkey_ptr(self.handle, remote_addr, &mut addr) })?;
        if addr.is_null() {
            return Err(ucs_status_t::UCS_ERR_INVALID_ADDR);
        }
        Ok(unsafe { std::slice::from_raw_parts_mut(addr as *mut u8, len) })
    }

    /// Get the raw rkey handle.
    #[inline]
    pub fn as_raw(&self) -> ucp_rkey_h {
        self.handle
    }

    /// Compare this key with another key belonging to the same worker.
    /// UCX returns zero when the keys refer to the same memory region.
    pub fn compare(&self, other: &RemoteKey, worker: &Worker) -> Result<bool, ucs_status_t> {
        if !self.worker_alive.load(Ordering::Acquire)
            || !other.worker_alive.load(Ordering::Acquire)
            || !Arc::ptr_eq(&self.worker_alive, &other.worker_alive)
            || !Arc::ptr_eq(&self.worker_alive, &worker.alive)
        {
            return Err(ucs_status_t::UCS_ERR_INVALID_PARAM);
        }
        let params = ucp_rkey_compare_params_t { field_mask: 0 };
        let mut result = 0;
        // SAFETY: the alive flags establish that both keys came from this live
        // worker, and the parameter and result storage live for the call.
        status_to_result(unsafe {
            ucp_rkey_compare(
                worker.handle,
                self.handle,
                other.handle,
                &params,
                &mut result,
            )
        })
        .map(|()| result == 0)
    }
}

impl Drop for RemoteKey {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ucp_rkey_destroy(self.handle) };
        }
    }
}
