use crate::completion::{owned_data_param, status_ptr_with_data};
use crate::dt::dt_make_contig;
use crate::ep::Ep;
use crate::ffi::*;
use crate::request::Pending;
use crate::status_ptr_to_result;
use crate::worker::Worker;
use crate::RequestParam;
use crate::Status;
use crate::{Completion, OwnedBuf, OwnedBufMut, OwningRequest};

use super::RemoteKey;

/// Safe RMA and AMO methods on endpoints.
///
/// All methods take `&self` and safe types (`&[u8]`, `&mut [u8]`, `u64`, `&RemoteKey`),
/// hiding the `unsafe` FFI calls internally. Follows the same pattern as `Ep::tag_send`.
impl Ep {
    // ── Put / Get ──

    /// Put `buffer` to a remote memory location, taking ownership of it.
    ///
    /// Returns [`Completion::Immediate`] with the buffer when UCX finishes
    /// inline, or [`Completion::Pending`] with an [`OwningRequest`] that hands
    /// the buffer back from [`OwningRequest::try_complete`]. Local completion
    /// only means the source may be reused; flush before relying on remote
    /// visibility.
    ///
    /// `param` contributes flags, callback, and user data. The datatype
    /// (always contiguous bytes), reply buffer, memory handle, memory type, and
    /// request allocation fields are controlled by this method and ignored if
    /// set: the memory type and handle come from `buffer`
    /// ([`OwnedBuf::memory_type`], [`OwnedBuf::memh`]), so device memory and
    /// registered buffers ([`crate::memh::AllocatedBuf`],
    /// [`crate::memh::RegisteredBuf`]) work here too. A callback may run before this returns, must not progress the
    /// worker, and must not free the request. An empty buffer always completes
    /// immediately, even with `NO_IMM_CMPL`.
    ///
    /// ```no_run
    /// # use ucx_sys::{ep::Ep, rma::RemoteKey, worker::Worker, RequestParamBuilder, Completion};
    /// # fn demo(ep: &Ep, worker: &Worker, rkey: &RemoteKey, addr: u64) {
    /// let param = RequestParamBuilder::new().build();
    /// match ep.rma_put(worker, b"hello".to_vec(), addr, rkey, &param).unwrap() {
    ///     Completion::Immediate(buffer) => drop(buffer),
    ///     Completion::Pending(request) => { /* progress + try_complete */ }
    /// }
    /// # }
    /// ```
    pub fn rma_put<'w, D: OwnedBuf>(
        &self,
        worker: &'w Worker,
        buffer: D,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Completion<D, OwningRequest<'w, D>>, Status> {
        let raw = owned_data_param(param, &buffer, worker)?;
        // SAFETY: the request (if any) reads only `buffer`'s stable storage
        // (`OwnedBuf`), which the returned value owns until UCX is done; the
        // contiguous-bytes datatype bounds the access to `buffer.len()`.
        unsafe {
            let ptr = ucp_put_nbx(
                self.handle,
                buffer.as_ptr() as _,
                buffer.len(),
                remote_addr,
                rkey.handle,
                &raw,
            );
            status_ptr_with_data(ptr, buffer, worker)
        }
    }

    /// Get remote memory into `buffer`, taking ownership of it.
    ///
    /// The filled buffer is returned through [`Completion::Immediate`] or
    /// [`OwningRequest::try_complete`]. On failure it comes back inside
    /// [`crate::Failed`] with unspecified contents. `param` is handled as in
    /// [`Ep::rma_put`].
    pub fn rma_get<'w, D: OwnedBufMut>(
        &self,
        worker: &'w Worker,
        mut buffer: D,
        remote_addr: u64,
        rkey: &RemoteKey,
        param: &RequestParam,
    ) -> Result<Completion<D, OwningRequest<'w, D>>, Status> {
        let raw = owned_data_param(param, &buffer, worker)?;
        // SAFETY: as in `rma_put`, for writes into `buffer`'s stable storage.
        unsafe {
            let ptr = ucp_get_nbx(
                self.handle,
                buffer.as_mut_ptr() as _,
                buffer.len(),
                remote_addr,
                rkey.handle,
                &raw,
            );
            status_ptr_with_data(ptr, buffer, worker)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::context::{Config, Context, Flags, ParamsBuilder};
    use crate::ep::{Ep, ParamsBuilder as EpParamsBuilder};
    use crate::memh::MemHandle;
    use crate::request::Pending;
    use crate::rma::RemoteKey;
    use crate::worker::{ParamsBuilder as WorkerParamsBuilder, RemoteWorkerAddress, Worker};
    use crate::RequestParamBuilder;

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
    /// Test-only driver for data-less completions (flush, close).
    fn wait(worker: &Worker, completion: crate::ep::OpCompletion<'_>, what: &str) {
        let Completion::Pending(mut request) = completion else {
            return;
        };
        for _ in 0..1_000_000 {
            worker.progress();
            match request.try_complete() {
                Ok(Ok(())) => return,
                Ok(Err(failed)) => panic!("{what} failed with {} ({})", failed.status, transport()),
                Err(still) => request = still,
            }
        }
        panic!("{what} did not complete ({})", transport());
    }

    fn close(worker: &Worker, ep: crate::ep::Ep) {
        let completion = ep
            .close(worker, crate::ep::CloseMode::Flush)
            .unwrap_or_else(|error| panic!("{error} ({})", transport()));
        wait(worker, completion, "endpoint close");
    }

    use crate::{Completion, OwningRequest};

    /// Test-only driver: progress until an owned RMA completion returns its data.
    fn drive_owned<D>(
        worker: &Worker,
        completion: Completion<D, OwningRequest<'_, D>>,
        what: &str,
    ) -> D {
        let mut request = match completion {
            Completion::Immediate(data) => return data,
            Completion::Pending(request) => request,
        };
        for _ in 0..1_000_000 {
            worker.progress();
            match request.try_complete() {
                Ok(Ok(data)) => return data,
                Ok(Err(failed)) => panic!("{what} failed with {} ({})", failed.status, transport()),
                Err(still) => request = still,
            }
        }
        panic!("{what} did not complete ({})", transport());
    }

    /// Self endpoint plus an rkey over `target`.
    fn rma_self_setup(
        target: &mut [u8],
    ) -> (
        Context,
        Worker,
        Ep,
        crate::memh::MemHandleGuard<'_>,
        Vec<u8>,
    ) {
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
        drop(address);
        let ep = worker
            .create_ep(EpParamsBuilder::new().address(&remote).build())
            .expect("endpoint");
        let memh = MemHandle::map_slice(&context, target, 0).expect("map target");
        let packed = RemoteKey::pack(&context, memh.mem_handle())
            .expect("pack rkey")
            .to_vec();
        (context, worker, ep, memh, packed)
    }

    /// Write a diagnostic straight to fd 2. libtest captures `eprintln!`, and
    /// a failure that ends in a UCX abort (worker destroyed under leaked
    /// requests) loses captured output, so CI would show no message.
    fn diag(message: &str) {
        use std::io::Write;
        let _ = writeln!(std::io::stderr().lock(), "{message}");
    }

    /// #208 acceptance (shared progress loop): N in-flight RMA puts and a
    /// pending endpoint flush are completed by ONE loop, and the flush only
    /// completes once every put has (UCX flush semantics cover RMA). Then
    /// the rkey is dropped (UCX requires that before the endpoint goes away)
    /// and the endpoint is closed through the same loop shape. Pinned to tcp:
    /// on `self` the puts complete inline.
    #[test]
    fn rma_puts_flush_and_close_share_one_progress_loop() {
        let (n, len) = (4usize, 1usize << 20);
        let mut target = vec![0u8; n * len];
        let target_addr = target.as_mut_ptr() as u64;
        let params = ParamsBuilder::new()
            .features(Flags::Rma)
            .mt_workers_shared(1)
            .build();
        let config = Config::read("", "").expect("config");
        // SAFETY: live config handle; NUL-terminated name and value.
        let status = unsafe {
            crate::ffi::ucp_config_modify(config.handle, c"TLS".as_ptr(), c"tcp".as_ptr())
        };
        assert_eq!(status, crate::ffi::ucs_status_t::UCS_OK, "pin TLS=tcp");
        let mut context = Context::new(&config, &params).expect("context");
        let worker = context
            .worker_create(&WorkerParamsBuilder::new().build())
            .expect("worker");
        let address = worker.pack_address().expect("pack address");
        let remote = RemoteWorkerAddress::new(address.to_vec());
        drop(address);
        let ep = worker
            .create_ep(EpParamsBuilder::new().address(&remote).build())
            .expect("endpoint");
        let memh = MemHandle::map_slice(&context, &mut target, 0).expect("map target");
        let packed = RemoteKey::pack(&context, memh.mem_handle())
            .expect("pack rkey")
            .to_vec();
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().no_imm_cmpl().build();
        let mut puts: Vec<_> = (0..n)
            .map(|i| {
                match ep
                    .rma_put(
                        &worker,
                        vec![i as u8 + 1; len],
                        target_addr + (i * len) as u64,
                        &rkey,
                        &param,
                    )
                    .expect("post put")
                {
                    Completion::Pending(request) => request,
                    Completion::Immediate(_) => panic!("NO_IMM_CMPL put must defer"),
                }
            })
            .collect();
        let mut flush = match ep.flush(&worker, &RequestParamBuilder::new().build()) {
            Ok(Completion::Pending(request)) => Some(request),
            Ok(Completion::Immediate(())) => None,
            Err(status) => panic!("flush: {status} ({})", transport()),
        };
        diag(&format!(
            "rma shared loop: flush pending={} ({})",
            flush.is_some(),
            transport()
        ));
        let mut flushed = flush.is_none();
        let mut puts_done = 0;
        for _ in 0..1_000_000 {
            if flushed && puts.is_empty() {
                break;
            }
            worker.progress();
            if let Some(request) = flush.take() {
                match request.try_complete() {
                    Ok(result) => {
                        result.unwrap_or_else(|failed| panic!("flush: {failed}"));
                        flushed = true;
                    }
                    Err(pending) => flush = Some(pending),
                }
            }
            let flushed_now = flushed;
            puts = puts
                .into_iter()
                .filter_map(|request| match request.try_complete() {
                    Ok(result) => {
                        let buffer = result.unwrap_or_else(|failed| panic!("put: {failed}"));
                        assert_eq!(buffer.len(), len);
                        puts_done += 1;
                        None
                    }
                    Err(pending) => {
                        if flushed_now {
                            // Flush completion implies every earlier put is
                            // complete; its request must report so now.
                            diag("put still pending after its flush completed");
                        }
                        Some(pending)
                    }
                })
                .collect();
            assert!(
                !flushed_now || puts.is_empty(),
                "flush completed while puts were still pending ({})",
                transport()
            );
        }
        assert!(flushed, "flush did not complete ({})", transport());
        assert_eq!(puts_done, n, "every put handed its buffer back");
        drop(rkey);
        drop(memh);
        close(&worker, ep);
        for i in 0..n {
            assert!(
                target[i * len..(i + 1) * len]
                    .iter()
                    .all(|&byte| byte == i as u8 + 1),
                "put {i} landed"
            );
        }
    }

    /// `NO_IMM_CMPL` forces the deferred path: both put and get return an
    /// `OwningRequest` that hands the buffer back from `try_complete`.
    #[test]
    fn rma_owned_put_get_pending_path_returns_buffers() {
        let mut target = vec![0u8; 64];
        let target_addr = target.as_mut_ptr() as u64;
        let (_context, worker, ep, memh, packed) = rma_self_setup(&mut target);
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().no_imm_cmpl().build();
        let payload: Vec<u8> = (0..64u8).collect();

        let put = ep
            .rma_put(
                &worker,
                payload.clone().into_boxed_slice(),
                target_addr,
                &rkey,
                &param,
            )
            .expect("post put");
        assert!(put.is_pending(), "NO_IMM_CMPL put must be deferred");
        assert_eq!(&*drive_owned(&worker, put, "put"), &payload[..]);
        let flush = ep
            .flush(&worker, &RequestParamBuilder::new().build())
            .expect("flush");
        wait(&worker, flush, "endpoint flush");

        let get = ep
            .rma_get(&worker, Box::new([0u8; 64]), target_addr, &rkey, &param)
            .expect("post get");
        assert!(get.is_pending(), "NO_IMM_CMPL get must be deferred");
        let fetched = drive_owned(&worker, get, "get");
        assert_eq!(&fetched[..], &payload[..]);
        drop(rkey);
        drop(memh);
        assert_eq!(target, payload);
        close(&worker, ep);
    }

    /// `FORCE_IMM_CMPL` must keep working: a small self put completes inline
    /// and returns the buffer (UCX's short-put path must not be disabled by
    /// the owned param handling).
    #[test]
    fn rma_owned_put_force_imm_cmpl_completes_inline() {
        let mut target = vec![0u8; 8];
        let target_addr = target.as_mut_ptr() as u64;
        let params = ParamsBuilder::new()
            .features(Flags::Rma)
            .mt_workers_shared(1)
            .estimated_num_eps(1)
            .build();
        let config = Config::read("", "").expect("config");
        // SAFETY: live config handle; NUL-terminated name and value.
        let status = unsafe {
            crate::ffi::ucp_config_modify(config.handle, c"TLS".as_ptr(), c"self".as_ptr())
        };
        assert_eq!(status, crate::ffi::ucs_status_t::UCS_OK, "pin TLS=self");
        let mut context = Context::new(&config, &params).expect("context");
        drop(config);
        let worker = context
            .worker_create(&WorkerParamsBuilder::new().build())
            .expect("worker");
        let address = worker.pack_address().expect("pack address");
        let remote = RemoteWorkerAddress::new(address.to_vec());
        drop(address);
        let ep = worker
            .create_ep(EpParamsBuilder::new().address(&remote).build())
            .expect("endpoint");
        let memh = MemHandle::map_slice(&context, &mut target, 0).expect("map target");
        let packed = RemoteKey::pack(&context, memh.mem_handle())
            .expect("pack rkey")
            .to_vec();
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().force_imm_cmpl().build();
        let put = ep
            .rma_put(&worker, vec![0x42u8; 8], target_addr, &rkey, &param)
            .unwrap_or_else(|status| panic!("force_imm put: {status} ({})", transport()));
        assert_eq!(
            put.into_immediate().expect("must complete inline"),
            vec![0x42u8; 8]
        );
        let flush = ep
            .flush(&worker, &RequestParamBuilder::new().build())
            .expect("flush");
        wait(&worker, flush, "endpoint flush");
        drop(rkey);
        drop(memh);
        assert_eq!(target, vec![0x42u8; 8]);
        close(&worker, ep);
    }

    /// The owned API forces contiguous bytes: a caller datatype of 2-byte
    /// elements must not make UCX write `2 * len` bytes past the buffer.
    #[test]
    fn rma_owned_put_ignores_caller_datatype() {
        let mut target = vec![0xEEu8; 16];
        let target_addr = target.as_mut_ptr() as u64;
        let (_context, worker, ep, memh, packed) = rma_self_setup(&mut target);
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new()
            .datatype(crate::dt::dt_make_contig(2))
            .build();
        let put = ep
            .rma_put(&worker, vec![0x11u8; 8], target_addr, &rkey, &param)
            .expect("post put");
        drive_owned(&worker, put, "put");
        let flush = ep
            .flush(&worker, &RequestParamBuilder::new().build())
            .expect("flush");
        wait(&worker, flush, "endpoint flush");
        drop(rkey);
        drop(memh);
        assert_eq!(&target[..8], &[0x11u8; 8]);
        assert_eq!(
            &target[8..],
            &[0xEEu8; 8],
            "bytes past the buffer untouched"
        );
        close(&worker, ep);
    }

    /// Dropping a pending owned get is safe: the fail-safe completes it before
    /// the buffer is freed (no use-after-free from safe code, #112).
    #[test]
    fn rma_owned_get_drop_while_pending_is_safe() {
        let mut target = vec![7u8; 4096];
        let target_addr = target.as_mut_ptr() as u64;
        let (_context, worker, ep, memh, packed) = rma_self_setup(&mut target);
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().no_imm_cmpl().build();
        let get = ep
            .rma_get(&worker, vec![0u8; 4096], target_addr, &rkey, &param)
            .expect("post get");
        assert!(
            get.is_pending(),
            "NO_IMM_CMPL get must exercise the fail-safe"
        );
        drop(get);
        drop(rkey);
        drop(memh);
        close(&worker, ep);
    }

    fn setup_amo_test() -> (Context, Worker, Ep) {
        // UCP_FEATURE_AMO32 and UCP_FEATURE_AMO64.
        let params = ParamsBuilder::new()
            .features(Flags::Rma | Flags::Amo32 | Flags::Amo64)
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

        (context, worker, ep)
    }

    fn run_amo_test<T>(
        initial: T,
        expected: T,
        what: &str,
        post: impl for<'w> FnOnce(
            &Ep,
            &'w Worker,
            u64,
            &RemoteKey,
            &crate::RequestParam,
        ) -> Result<crate::rma::AmoCompletion<'w>, crate::Status>,
    ) where
        T: Copy + PartialEq + std::fmt::Debug,
    {
        if std::env::var("UCX_TLS").as_deref() != Ok("self") {
            eprintln!("skipping {what}: run with UCX_TLS=self");
            return;
        }
        let mut target = initial;
        let (context, worker, ep) = setup_amo_test();
        // The caller owns the aligned target until after the mapping is dropped.
        let remote_addr = &mut target as *mut T as u64;
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                &mut target as *mut T as *mut u8,
                std::mem::size_of::<T>(),
            )
        };
        let memh = MemHandle::map_slice(&context, bytes, 0).expect("map target");
        let packed = RemoteKey::pack(&context, memh.mem_handle()).expect("pack rkey");
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().build();

        let completion = post(&ep, &worker, remote_addr, &rkey, &param)
            .unwrap_or_else(|status| panic!("post {what}: {status} ({})", transport()));
        if let Completion::Pending(mut request) = completion {
            let mut completed = false;
            for _ in 0..1_000_000 {
                worker.progress();
                match request.try_complete() {
                    Ok(result) => {
                        result
                            .unwrap_or_else(|status| panic!("{what}: {status} ({})", transport()));
                        completed = true;
                        break;
                    }
                    Err(still) => request = still,
                }
            }
            assert!(completed, "{what} did not complete ({})", transport());
        }
        let flush = ep
            .flush(&worker, &param)
            .unwrap_or_else(|status| panic!("post endpoint flush: {status} ({})", transport()));
        wait(&worker, flush, "endpoint flush");

        let result = unsafe { *(remote_addr as *const T) };
        assert_eq!(result, expected, "{what} result mismatch ({})", transport());

        drop(rkey);
        drop(memh);
        close(&worker, ep);
    }

    #[test]
    fn amo_self_add64_round_trip() {
        run_amo_test(
            10u64,
            15u64,
            "amo_add64",
            |ep, worker, addr, rkey, param| ep.amo_add64(worker, 5, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_add32_round_trip() {
        run_amo_test(
            10u32,
            15u32,
            "amo_add32",
            |ep, worker, addr, rkey, param| ep.amo_add32(worker, 5, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_xor64_round_trip() {
        run_amo_test(
            0b1010u64,
            0b0110u64,
            "amo_xor64",
            |ep, worker, addr, rkey, param| ep.amo_xor64(worker, 0b1100, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_xor32_round_trip() {
        run_amo_test(
            0b1010u32,
            0b0110u32,
            "amo_xor32",
            |ep, worker, addr, rkey, param| ep.amo_xor32(worker, 0b1100, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_swap64_round_trip() {
        run_amo_test(
            10u64,
            5u64,
            "amo_swap64",
            |ep, worker, addr, rkey, param| ep.amo_swap64(worker, 5, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_swap32_round_trip() {
        run_amo_test(
            10u32,
            5u32,
            "amo_swap32",
            |ep, worker, addr, rkey, param| ep.amo_swap32(worker, 5, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_and64_round_trip() {
        run_amo_test(
            0b1010u64,
            0b1000u64,
            "amo_and64",
            |ep, worker, addr, rkey, param| ep.amo_and64(worker, 0b1100, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_and32_round_trip() {
        run_amo_test(
            0b1010u32,
            0b1000u32,
            "amo_and32",
            |ep, worker, addr, rkey, param| ep.amo_and32(worker, 0b1100, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_or64_round_trip() {
        run_amo_test(
            0b1010u64,
            0b1110u64,
            "amo_or64",
            |ep, worker, addr, rkey, param| ep.amo_or64(worker, 0b1100, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_or32_round_trip() {
        run_amo_test(
            0b1010u32,
            0b1110u32,
            "amo_or32",
            |ep, worker, addr, rkey, param| ep.amo_or32(worker, 0b1100, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_cswap64_round_trip() {
        run_amo_test(
            10u64,
            20u64,
            "amo_cswap64 match",
            |ep, worker, addr, rkey, param| ep.amo_cswap64(worker, 10, 20, addr, rkey, param),
        );
        run_amo_test(
            10u64,
            10u64,
            "amo_cswap64 mismatch",
            |ep, worker, addr, rkey, param| ep.amo_cswap64(worker, 11, 20, addr, rkey, param),
        );
    }

    #[test]
    fn amo_self_cswap32_round_trip() {
        run_amo_test(
            10u32,
            20u32,
            "amo_cswap32 match",
            |ep, worker, addr, rkey, param| ep.amo_cswap32(worker, 10, 20, addr, rkey, param),
        );
        run_amo_test(
            10u32,
            10u32,
            "amo_cswap32 mismatch",
            |ep, worker, addr, rkey, param| ep.amo_cswap32(worker, 11, 20, addr, rkey, param),
        );
    }

    fn run_fetch_amo_test<T>(
        initial: T,
        expected: T,
        what: &str,
        post: impl for<'w> FnOnce(
            &Ep,
            &'w Worker,
            u64,
            &RemoteKey,
        )
            -> Result<crate::rma::FetchAmoCompletion<'w, T>, crate::Status>,
    ) where
        T: crate::rma::AmoWord + PartialEq + std::fmt::Debug,
    {
        if std::env::var("UCX_TLS").as_deref() != Ok("self") {
            eprintln!("skipping {what}: run with UCX_TLS=self");
            return;
        }
        let mut target = initial;
        let (context, worker, ep) = setup_amo_test();
        // The caller owns the aligned target until after the mapping is dropped.
        let remote_addr = &mut target as *mut T as u64;
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                &mut target as *mut T as *mut u8,
                std::mem::size_of::<T>(),
            )
        };
        let memh = MemHandle::map_slice(&context, bytes, 0).expect("map target");
        let packed = RemoteKey::pack(&context, memh.mem_handle()).expect("pack rkey");
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().build();

        let completion = post(&ep, &worker, remote_addr, &rkey)
            .unwrap_or_else(|status| panic!("post {what}: {status} ({})", transport()));
        let old = match completion {
            Completion::Immediate(old) => old,
            Completion::Pending(mut request) => {
                let mut old = None;
                for _ in 0..1_000_000 {
                    worker.progress();
                    match request.try_complete() {
                        Ok(result) => {
                            old = Some(result.unwrap_or_else(|status| {
                                panic!("{what}: {status} ({})", transport())
                            }));
                            break;
                        }
                        Err(still) => request = still,
                    }
                }
                old.unwrap_or_else(|| panic!("{what} did not complete ({})", transport()))
            }
        };
        let flush = ep
            .flush(&worker, &param)
            .unwrap_or_else(|status| panic!("post endpoint flush: {status} ({})", transport()));
        wait(&worker, flush, "endpoint flush");

        assert_eq!(old, initial, "{what} old value mismatch ({})", transport());
        let result = unsafe { *(remote_addr as *const T) };
        assert_eq!(result, expected, "{what} result mismatch ({})", transport());

        drop(rkey);
        drop(memh);
        close(&worker, ep);
    }

    #[test]
    fn amo_fcswap64_self_round_trip() {
        run_fetch_amo_test(
            10u64,
            20u64,
            "amo_fcswap64 match",
            |ep, worker, addr, rkey| ep.amo_fcswap64(worker, 10, 20, addr, rkey),
        );
        run_fetch_amo_test(
            10u64,
            10u64,
            "amo_fcswap64 mismatch",
            |ep, worker, addr, rkey| ep.amo_fcswap64(worker, 11, 20, addr, rkey),
        );
    }

    #[test]
    fn amo_fcswap32_self_round_trip() {
        run_fetch_amo_test(
            10u32,
            20u32,
            "amo_fcswap32 match",
            |ep, worker, addr, rkey| ep.amo_fcswap32(worker, 10, 20, addr, rkey),
        );
        run_fetch_amo_test(
            10u32,
            10u32,
            "amo_fcswap32 mismatch",
            |ep, worker, addr, rkey| ep.amo_fcswap32(worker, 11, 20, addr, rkey),
        );
    }

    /// Smoke test for the deferred path: a `NO_IMM_CMPL` AMO posted from a
    /// helper frame is still pending after that frame returns, and dropping it
    /// completes it (fail-safe) before the slots are freed. This does not, by
    /// itself, prove the operand is heap-owned: on `UCX_TLS=self` UCX may pack
    /// the operand inside the call. The heap ownership is structural
    /// (`rma::amo::post_amo`).
    #[test]
    fn amo_deferred_operand_outlives_posting_frame() {
        if std::env::var("UCX_TLS").as_deref() != Ok("self") {
            eprintln!("skipping: run with UCX_TLS=self");
            return;
        }
        let mut target = 1u64;
        let (context, worker, ep) = setup_amo_test();
        let remote_addr = &mut target as *mut u64 as u64;
        let bytes =
            unsafe { std::slice::from_raw_parts_mut(&mut target as *mut u64 as *mut u8, 8) };
        let memh = MemHandle::map_slice(&context, bytes, 0).expect("map target");
        let packed = RemoteKey::pack(&context, memh.mem_handle()).expect("pack rkey");
        let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().no_imm_cmpl().build();
        // Post from a helper frame so no caller stack slot could hold the operand.
        fn post<'w>(
            ep: &Ep,
            worker: &'w Worker,
            addr: u64,
            rkey: &RemoteKey,
            param: &crate::RequestParam,
        ) -> crate::rma::AmoCompletion<'w> {
            let operand = 41u64;
            ep.amo_add64(worker, operand, addr, rkey, param)
                .expect("post")
        }
        let completion = post(&ep, &worker, remote_addr, &rkey, &param);
        assert!(completion.is_pending(), "NO_IMM_CMPL must defer the AMO");
        // Scribble over the stack region the helper frame used.
        let scribble = [0xFFu8; 4096];
        std::hint::black_box(&scribble);
        drop(completion); // Drop fail-safe completes it before freeing the slots
        let flush = ep
            .flush(&worker, &RequestParamBuilder::new().build())
            .expect("flush");
        wait(&worker, flush, "endpoint flush");
        assert_eq!(unsafe { *(remote_addr as *const u64) }, 42);
        drop(rkey);
        drop(memh);
        close(&worker, ep);
    }

    #[test]
    fn amo_fadd64_self_round_trip() {
        run_fetch_amo_test(10u64, 15u64, "amo_fadd64", |ep, worker, addr, rkey| {
            ep.amo_fadd64(worker, 5, addr, rkey)
        });
    }

    #[test]
    fn amo_fadd32_self_round_trip() {
        run_fetch_amo_test(10u32, 15u32, "amo_fadd32", |ep, worker, addr, rkey| {
            ep.amo_fadd32(worker, 5, addr, rkey)
        });
    }

    #[test]
    fn amo_fxor64_self_round_trip() {
        run_fetch_amo_test(
            0b1010u64,
            0b0110u64,
            "amo_fxor64",
            |ep, worker, addr, rkey| ep.amo_fxor64(worker, 0b1100, addr, rkey),
        );
    }

    #[test]
    fn amo_fxor32_self_round_trip() {
        run_fetch_amo_test(
            0b1010u32,
            0b0110u32,
            "amo_fxor32",
            |ep, worker, addr, rkey| ep.amo_fxor32(worker, 0b1100, addr, rkey),
        );
    }

    #[test]
    fn amo_fswap64_self_round_trip() {
        run_fetch_amo_test(10u64, 5u64, "amo_fswap64", |ep, worker, addr, rkey| {
            ep.amo_fswap64(worker, 5, addr, rkey)
        });
    }

    #[test]
    fn amo_fswap32_self_round_trip() {
        run_fetch_amo_test(10u32, 5u32, "amo_fswap32", |ep, worker, addr, rkey| {
            ep.amo_fswap32(worker, 5, addr, rkey)
        });
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
        // Default params on purpose: UCX 1.16 (Ubuntu 24.04's libucx) transfers zero
        // bytes for a put with NO_IMM_CMPL and no explicit datatype (fixed upstream in
        // openucx ba426c890, UCX 1.17). `wait` handles immediate completion.
        let param = RequestParamBuilder::new().build();

        let put = ep
            .rma_put(&worker, b"ucx-rs!!".to_vec(), target_addr, &rkey, &param)
            .unwrap_or_else(|status| panic!("post RMA put: {status} ({})", transport()));
        assert_eq!(drive_owned(&worker, put, "RMA put"), b"ucx-rs!!");
        // A completed put only means the source buffer may be reused. The data is
        // guaranteed to be at the target, and visible to a later get, only after a
        // flush (see the ucp_put_nbx docs).
        let flush = ep
            .flush(&worker, &param)
            .unwrap_or_else(|status| panic!("post endpoint flush: {status} ({})", transport()));
        wait(&worker, flush, "endpoint flush");

        let get = ep
            .rma_get(&worker, vec![0u8; 8], target_addr, &rkey, &param)
            .unwrap_or_else(|status| panic!("post RMA get: {status} ({})", transport()));
        let fetched = drive_owned(&worker, get, "RMA get");

        drop(rkey);
        drop(memh);
        assert_eq!(
            (&target[..], &fetched[..]),
            (&b"ucx-rs!!"[..], &b"ucx-rs!!"[..]),
            "(RMA put target, RMA get result) ({})",
            transport()
        );
        close(&worker, ep);
    }
}
