use crate::context::Context;
use crate::ep::Ep;
use crate::ffi::*;
use crate::memh::MemHandle;
use crate::status_to_result;
use crate::worker::Worker;
use crate::Request;
use crate::Status;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// RAII wrapper around a UCP remote key handle (`ucp_rkey_h`), unpacked on
/// an endpoint and destroyed when dropped.
///
/// The key borrows its [`Ep`]: UCX requires an rkey to be destroyed after the
/// operations using it are flushed and *before* the endpoint it was unpacked
/// on is destroyed (`ucp_rkey_destroy`). The borrow makes closing or dropping
/// the endpoint while a key is alive a compile error:
///
/// ```compile_fail,E0505
/// # use ucx_sys::{ep::{CloseMode, Ep}, rma::RemoteKey, worker::Worker};
/// # fn demo(worker: &Worker, ep: Ep, packed: &[u8]) {
/// let key = RemoteKey::unpack(&ep, packed).unwrap();
/// let _close = ep.close(worker, CloseMode::Flush); // ep is still borrowed
/// drop(key);
/// # }
/// ```
///
/// ```compile_fail,E0505
/// # use ucx_sys::{ep::Ep, rma::RemoteKey};
/// # fn demo(ep: Ep, packed: &[u8]) {
/// let key = RemoteKey::unpack(&ep, packed).unwrap();
/// drop(ep); // the key must go first
/// drop(key);
/// # }
/// ```
pub struct RemoteKey<'ep> {
    pub(crate) handle: ucp_rkey_h,
    pub(crate) worker_alive: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) _ep: std::marker::PhantomData<&'ep Ep>,
}

fn frame_rkey_payload(payload: &[u8]) -> Result<Vec<u8>, Status> {
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| ucs_status_t::UCS_ERR_OUT_OF_RANGE)?;
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&payload_len.to_le_bytes());
    framed.extend_from_slice(payload);
    Ok(framed)
}

fn unframe_rkey_payload(buffer: &[u8]) -> Result<&[u8], Status> {
    if buffer.len() < 4 {
        return Err(Status(ucs_status_t::UCS_ERR_INVALID_PARAM));
    }
    let payload_len = u32::from_le_bytes(buffer[..4].try_into().unwrap()) as usize;
    if payload_len != buffer.len() - 4 {
        return Err(Status(ucs_status_t::UCS_ERR_INVALID_PARAM));
    }
    Ok(&buffer[4..])
}

impl<'ep> RemoteKey<'ep> {
    /// Pack a memory handle using the framed format consumed by [`Self::unpack`].
    ///
    /// The wire format is exactly `[4-byte little-endian payload length][payload]`.
    /// The payload is the opaque byte sequence returned by UCX's `ucp_rkey_pack`.
    pub fn pack(context: &Context, memh: &MemHandle) -> Result<Vec<u8>, Status> {
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
    ///
    /// The key borrows `ep` and must be dropped before the endpoint is closed
    /// or dropped (UCX destroy-order rule).
    pub fn unpack(ep: &'ep Ep, rkey_buffer: &[u8]) -> Result<RemoteKey<'ep>, Status> {
        let payload = unframe_rkey_payload(rkey_buffer)?;
        let mut rkey: ucp_rkey_h = std::ptr::null_mut();
        status_to_result(unsafe {
            ucp_ep_rkey_unpack(ep.handle, payload.as_ptr() as *const _, &mut rkey)
        })
        .map(|()| RemoteKey {
            handle: rkey,
            worker_alive: Arc::clone(&ep.worker_alive),
            _ep: std::marker::PhantomData,
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
    ///
    /// Fails with `UCS_ERR_INVALID_PARAM` once the worker is destroyed: the
    /// key's UCX state was released with it.
    pub fn rkey_ptr(&mut self, remote_addr: u64, len: usize) -> Result<&mut [u8], Status> {
        if !self.worker_alive.load(Ordering::Acquire) {
            return Err(Status(ucs_status_t::UCS_ERR_INVALID_PARAM));
        }
        let mut addr = std::ptr::null_mut();
        status_to_result(unsafe { ucp_rkey_ptr(self.handle, remote_addr, &mut addr) })?;
        if addr.is_null() {
            return Err(Status(ucs_status_t::UCS_ERR_INVALID_ADDR));
        }
        Ok(unsafe { std::slice::from_raw_parts_mut(addr as *mut u8, len) })
    }

    /// Compare this key with another key belonging to the same worker.
    /// UCX returns zero when the keys refer to the same memory region.
    pub fn compare(&self, other: &RemoteKey<'_>, worker: &Worker) -> Result<bool, Status> {
        if !self.worker_alive.load(Ordering::Acquire)
            || !other.worker_alive.load(Ordering::Acquire)
            || !Arc::ptr_eq(&self.worker_alive, &other.worker_alive)
            || !Arc::ptr_eq(&self.worker_alive, &worker.alive)
        {
            return Err(Status(ucs_status_t::UCS_ERR_INVALID_PARAM));
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

impl Drop for RemoteKey<'_> {
    fn drop(&mut self) {
        // The endpoint is still alive (borrowed), but `Ep` does not borrow its
        // worker, so the worker may already be destroyed: then leak the key
        // rather than call into freed UCX state.
        if !self.handle.is_null() && self.worker_alive.load(Ordering::Acquire) {
            // SAFETY: live key; its endpoint outlives it (borrow) and its
            // worker is alive.
            unsafe { ucp_rkey_destroy(self.handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rkey_framing_round_trip_preserves_payload() {
        let payload = [0x12, 0x34, 0xab, 0xcd];
        let framed = frame_rkey_payload(&payload).unwrap();
        assert_eq!(&framed[..4], &(payload.len() as u32).to_le_bytes());
        assert_eq!(unframe_rkey_payload(&framed).unwrap(), payload);
    }

    /// `Ep` does not borrow its worker, so the worker can be destroyed while
    /// the endpoint and a key unpacked on it are still alive. Dropping the
    /// key then must not call into the destroyed worker's UCX state.
    #[test]
    fn rkey_drop_after_worker_destroyed_skips_ucx() {
        use crate::context::{Config, Context, Flags, ParamsBuilder};
        use crate::worker::{ParamsBuilder as WorkerParamsBuilder, RemoteWorkerAddress};
        let params = ParamsBuilder::new()
            .features(Flags::Rma)
            .mt_workers_shared(1)
            .build();
        let mut context =
            Context::new(&Config::read("", "").expect("config"), &params).expect("context");
        let worker = context
            .worker_create(&WorkerParamsBuilder::new().build())
            .expect("worker");
        let address = worker.pack_address().expect("address");
        let remote = RemoteWorkerAddress::new(address.to_vec());
        drop(address);
        let ep = worker
            .create_ep(crate::ep::ParamsBuilder::new().address(&remote).build())
            .expect("endpoint");
        let mut target = vec![0u8; 64];
        let packed = {
            let memh = MemHandle::map_slice(&context, &mut target, 0).expect("map");
            RemoteKey::pack(&context, memh.mem_handle()).expect("pack")
        };
        let mut key = RemoteKey::unpack(&ep, &packed).expect("unpack");
        drop(worker);
        assert!(!key.worker_alive.load(Ordering::Acquire));
        // The safe accessor must not reach ucp_rkey_ptr on released state.
        assert_eq!(
            key.rkey_ptr(0, 0).err(),
            Some(Status(ucs_status_t::UCS_ERR_INVALID_PARAM))
        );
        drop(key); // must skip ucp_rkey_destroy
        drop(ep); // skips UCX too (worker dead)
    }

    #[test]
    fn rkey_unpack_accepts_public_framed_bytes() {
        let payload = [0x12, 0x34, 0xab, 0xcd];
        let framed = frame_rkey_payload(&payload).unwrap();
        assert_eq!(unframe_rkey_payload(&framed).unwrap(), payload);
        #[allow(clippy::extra_unused_lifetimes)]
        fn pin<'e>() {
            let _: fn(&'e Ep, &[u8]) -> Result<RemoteKey<'e>, Status> = RemoteKey::<'e>::unpack;
        }
        pin();
    }
}
