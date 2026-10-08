use crate::ep::Ep;
use crate::ffi::*;
use crate::status_ptr_to_result;
use crate::worker::Worker;
use crate::Request;
use crate::RequestParam;
use crate::Status;

use super::RemoteKey;

/// Safe RMA and AMO methods on endpoints.
///
/// All methods take `&self` and safe types (`&[u8]`, `&mut [u8]`, `u64`, `&RemoteKey`),
/// hiding the `unsafe` FFI calls internally. Follows the same pattern as `Ep::tag_send`.
impl Ep {
    // ── Put / Get ──

    /// Put data to a remote memory location.
    pub fn rma_put(
        &self,
        buffer: &[u8],
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_put_nbx(
                self.handle,
                buffer.as_ptr() as _,
                buffer.len(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Get data from a remote memory location.
    pub fn rma_get(
        &self,
        buffer: &mut [u8],
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_get_nbx(
                self.handle,
                buffer.as_ptr() as _,
                buffer.len(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Put `len` bytes from an arbitrary address, including device (GPU) memory.
    ///
    /// The slice-based [`Ep::rma_put`] cannot express accelerator buffers — a device pointer
    /// must never be turned into a host `&[u8]`. See [`Ep::tag_send_ptr`] for the rationale.
    /// Set `RequestParamBuilder::memory_type(UCS_MEMORY_TYPE_CUDA)` for device buffers.
    ///
    /// # Safety
    ///
    /// - `ptr` must be valid for reads of `len` bytes and readable by UCX for the whole
    ///   operation (device allocation on the current device, or managed memory).
    /// - The memory must stay alive and unpublished until the request completes.
    /// - `len` must not exceed the underlying allocation.
    pub unsafe fn rma_put_ptr(
        &self,
        ptr: *const u8,
        len: usize,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_put_nbx(
                self.handle,
                ptr as _,
                len,
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Get `len` bytes into an arbitrary address, including device (GPU) memory.
    ///
    /// Counterpart of [`Ep::rma_put_ptr`].
    ///
    /// # Safety
    ///
    /// - `ptr` must be valid for writes of `len` bytes and writable by UCX for the whole
    ///   operation (device allocation on the current device, or managed memory).
    /// - The memory must stay alive and unpublished until the request completes.
    pub unsafe fn rma_get_ptr(
        &self,
        ptr: *mut u8,
        len: usize,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_get_nbx(
                self.handle,
                ptr as _,
                len,
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    // ── AMO — no-fetch variants ──

    /// Atomic add 64-bit on remote memory (no fetch of old value).
    pub fn amo_add64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic XOR 64-bit on remote memory (no fetch of old value).
    pub fn amo_xor64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic swap 64-bit on remote memory (no fetch of old value).
    pub fn amo_swap64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic AND 64-bit on remote memory (no fetch of old value).
    pub fn amo_and64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_AND,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic OR 64-bit on remote memory (no fetch of old value).
    pub fn amo_or64(
        &self,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_OR,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic compare-and-swap 64-bit (no fetch — use fetch variant if you need the old value).
    pub fn amo_cswap64(
        &self,
        expected: u64,
        replacement: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        let operand = [expected, replacement];
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
                operand.as_ptr() as *const _,
                std::mem::size_of::<[u64; 2]>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    // ── AMO — 32-bit no-fetch variants ──

    /// Atomic add 32-bit on remote memory (no fetch of old value).
    pub fn amo_add32(
        &self,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic XOR 32-bit on remote memory (no fetch of old value).
    pub fn amo_xor32(
        &self,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic swap 32-bit on remote memory (no fetch of old value).
    pub fn amo_swap32(
        &self,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic AND 32-bit on remote memory (no fetch of old value).
    pub fn amo_and32(
        &self,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_AND,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic OR 32-bit on remote memory (no fetch of old value).
    pub fn amo_or32(
        &self,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_OR,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    /// Atomic compare-and-swap 32-bit (no fetch — use fetch variant if you need the old value).
    pub fn amo_cswap32(
        &self,
        expected: u32,
        replacement: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Option<Request>, Status> {
        let operand = [expected, replacement];
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
                operand.as_ptr() as *const _,
                std::mem::size_of::<[u32; 2]>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
    }

    // ── AMO — fetch variants ──

    /// Atomic fetch-and-add 64-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fadd64<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u64,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, Status> {
        let param = RequestParam::fetch_params(reply);
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| super::fetch_amo_result(request, worker))
    }

    /// Atomic fetch-and-xor 64-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fxor64<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u64,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, Status> {
        let param = RequestParam::fetch_params(reply);
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| super::fetch_amo_result(request, worker))
    }

    /// Atomic fetch-and-swap 64-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fswap64<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u64,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, Status> {
        let param = RequestParam::fetch_params(reply);
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
                &operand as *const _ as *const _,
                std::mem::size_of::<u64>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| super::fetch_amo_result(request, worker))
    }

    /// Atomic fetch compare-and-swap 64-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fcswap64<'w, 'a>(
        &self,
        worker: &'w Worker,
        compare: u64,
        swap: u64,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u64,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u64>, Status> {
        let operand = [compare, swap];
        let param = RequestParam::fetch_params(reply);
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
                operand.as_ptr() as *const _,
                std::mem::size_of::<[u64; 2]>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| super::fetch_amo_result(request, worker))
    }

    /// Atomic fetch-and-add 32-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fadd32<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u32,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, Status> {
        let param = RequestParam::fetch_params(reply);
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_ADD,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| super::fetch_amo_result(request, worker))
    }

    /// Atomic fetch-and-xor 32-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fxor32<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u32,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, Status> {
        let param = RequestParam::fetch_params(reply);
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_XOR,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| super::fetch_amo_result(request, worker))
    }

    /// Atomic fetch-and-swap 32-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fswap32<'w, 'a>(
        &self,
        worker: &'w Worker,
        operand: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u32,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, Status> {
        let param = RequestParam::fetch_params(reply);
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_SWAP,
                &operand as *const _ as *const _,
                std::mem::size_of::<u32>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| super::fetch_amo_result(request, worker))
    }

    /// Atomic fetch compare-and-swap 32-bit; writes the previous value to `reply`.
    /// The reply buffer must remain valid until the request is resolved.
    pub fn amo_fcswap32<'w, 'a>(
        &self,
        worker: &'w Worker,
        expected: u32,
        swap: u32,
        remote_addr: u64,
        rkey: &RemoteKey,
        reply: &'a mut u32,
    ) -> Result<super::FetchAmoRequest<'w, 'a, u32>, Status> {
        let operand = [expected, swap];
        let param = RequestParam::fetch_params(reply);
        status_ptr_to_result(unsafe {
            ucp_atomic_op_nbx(
                self.handle,
                ucp_atomic_op_t::UCP_ATOMIC_OP_CSWAP,
                operand.as_ptr() as *const _,
                std::mem::size_of::<[u32; 2]>(),
                remote_addr,
                rkey.handle,
                &param.handle,
            )
        })
        .map(|request| super::fetch_amo_result(request, worker))
    }
}

#[cfg(test)]
mod tests {
    use crate::context::{Config, Context, Flags, ParamsBuilder};
    use crate::ep::ParamsBuilder as EpParamsBuilder;
    use crate::memh::MemHandle;
    use crate::rma::RemoteKey;
    use crate::worker::{ParamsBuilder as WorkerParamsBuilder, RemoteWorkerAddress, Worker};
    use crate::{Request, RequestParamBuilder};

    /// UCX version and transport selection, for failure messages.
    fn transport() -> String {
        format!(
            "UCX {}, UCX_TLS={:?}",
            crate::version::get_version_string(),
            std::env::var("UCX_TLS").ok()
        )
    }

    /// Progress `worker` until `request` completes. Bounded, so a transport that
    /// cannot complete the operation fails the test instead of hanging it.
    fn wait(worker: &Worker, request: Option<Request>, what: &str) {
        let Some(request) = request else {
            return;
        };
        for _ in 0..1_000_000 {
            match request.check_finished() {
                Ok(true) => return,
                Ok(false) => {
                    worker.progress();
                }
                Err(status) => panic!("{what} failed with {status} ({})", transport()),
            }
        }
        panic!("{what} did not complete ({})", transport());
    }

    /// RMA put and get through an endpoint connected to its own worker, using a
    /// packed and unpacked rkey. CI also runs this test on its own with
    /// `UCX_TLS=self` (the "RMA smoke test (self TLS)" job selects it by its full
    /// path, `rma::ep_ops::tests::rma_self_put_get_round_trip`; keep them in sync).
    #[test]
    fn rma_self_put_get_round_trip() {
        let params = ParamsBuilder::new()
            .features(Flags::Rma)
            .mt_workers_shared(1)
            .estimated_num_eps(1)
            .build();
        let mut context =
            Context::new(&Config::read("", "").expect("config"), &params).expect("context");
        let worker = context
            .worker_create(&WorkerParamsBuilder::new().build())
            .expect("worker");
        let address = worker.pack_address().expect("pack address");
        let remote = RemoteWorkerAddress::new(address.to_vec());
        let ep = worker
            .create_ep(EpParamsBuilder::new().address(&remote).build())
            .expect("endpoint");
        drop(address);

        let mut target = [0u8; 8];
        let target_addr = target.as_mut_ptr() as u64;
        let memh = MemHandle::map_slice(&context, &mut target, 0).expect("map target");
        let packed = RemoteKey::pack(&context, memh.mem_handle()).expect("pack rkey");
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().no_imm_cmpl().build();

        let put = ep
            .rma_put(b"ucx-rs!!", target_addr, &rkey, &param)
            .unwrap_or_else(|status| panic!("post RMA put: {status} ({})", transport()));
        wait(&worker, put, "RMA put");

        let mut fetched = [0u8; 8];
        let get = ep
            .rma_get(&mut fetched, target_addr, &rkey, &param)
            .unwrap_or_else(|status| panic!("post RMA get: {status} ({})", transport()));
        wait(&worker, get, "RMA get");
        assert_eq!(&fetched, b"ucx-rs!!", "RMA get result ({})", transport());

        drop(rkey);
        drop(memh);
        assert_eq!(&target, b"ucx-rs!!", "RMA put target ({})", transport());
        ep.close(&worker, 0).expect("close endpoint");
    }
}
