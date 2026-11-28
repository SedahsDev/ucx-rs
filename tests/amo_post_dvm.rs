//! Cross-PE fire-and-forget AMO (`ucp_atomic_post`) test for the PMIx DVM.
//!
//! Launch with prterun and at least 2 ranks, exporting the size so every
//! rank can discover it (same convention as the openshmem-rs examples):
//!
//! ```text
//! LD_LIBRARY_PATH="$HOME/.local/ucx/lib:$HOME/.local/ucc/lib" \
//! prterun -np 2 -x PMIX_SIZE=2 \
//!     cargo test --test amo_post_dvm -- --nocapture
//! ```
//!
//! The test skips itself when `PMIX_RANK` is absent, so a plain
//! `cargo test` run does not need a DVM.
//!
//! Rank and size come from `PMIX_RANK` / `PMIX_SIZE`. Worker addresses,
//! packed rkeys, and target base addresses are exchanged through files in a
//! run-scoped directory keyed by `PMIX_NAMESPACE` (unique per prterun
//! session), so the test needs no PMIx client library. Each rank posts a
//! ring of fire-and-forget AMOs to PE `(rank + 1) % size` (memory allocated
//! by UCX so the peer reaches it through the shared-memory RMA lane),
//! flushes once — the whole point of the post semantic: no per-operation
//! completion — and the receiver verifies its own targets after a file
//! barrier. Loopback (UCX_TLS=self) cannot host this: it needs real
//! cross-process endpoints.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ucx_sys::completion::OwnedBuf;
use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::ep::{CloseMode, ParamsBuilder as EpParamsBuilder};
use ucx_sys::memh::AllocatedBuf;
use ucx_sys::rma::RemoteKey;
use ucx_sys::worker::{ParamsBuilder as WorkerParamsBuilder, RemoteWorkerAddress, Worker};
use ucx_sys::{RequestParamBuilder, Status};

/// Fire-and-forget posts per op family. Large enough to prove pipelining:
/// all of them complete with one flush and no per-operation requests.
const POSTS: usize = 256;

/// 24 bytes: u64 add target @0, u64 xor target @8, u32 or target @16,
/// u32 and target @20.
const TARGET_LEN: usize = 24;
const ADD_OFFSET: u64 = 0;
const XOR_OFFSET: u64 = 8;
const OR_OFFSET: u64 = 16;
const AND_OFFSET: u64 = 20;

/// One rank's published data: length-prefixed worker address, length-prefixed
/// packed rkey, and the target base address.
struct PeerInfo {
    address: Vec<u8>,
    rkey: Vec<u8>,
    base: u64,
}

fn encode(address: &[u8], rkey: &[u8], base: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + address.len() + 4 + rkey.len() + 8);
    out.extend_from_slice(&(address.len() as u32).to_ne_bytes());
    out.extend_from_slice(address);
    out.extend_from_slice(&(rkey.len() as u32).to_ne_bytes());
    out.extend_from_slice(rkey);
    out.extend_from_slice(&base.to_ne_bytes());
    out
}

fn decode(bytes: &[u8]) -> PeerInfo {
    let mut cursor = 0;
    let take = |cursor: &mut usize, len: usize| -> &[u8] {
        let slice = &bytes[*cursor..*cursor + len];
        *cursor += len;
        slice
    };
    let addr_len = u32::from_ne_bytes(take(&mut cursor, 4).try_into().unwrap()) as usize;
    let address = take(&mut cursor, addr_len).to_vec();
    let rkey_len = u32::from_ne_bytes(take(&mut cursor, 4).try_into().unwrap()) as usize;
    let rkey = take(&mut cursor, rkey_len).to_vec();
    let base = u64::from_ne_bytes(take(&mut cursor, 8).try_into().unwrap());
    PeerInfo {
        address,
        rkey,
        base,
    }
}

fn write_file(path: &Path, bytes: &[u8]) {
    // Unique tmp name per file (a plain ".tmp" suffix races between ranks
    // when both write `done.<rank>` concurrently and map to the same tmp).
    let tmp = PathBuf::from(format!("{}.tmp.{}", path.display(), std::process::id()));
    std::fs::write(&tmp, bytes).unwrap_or_else(|e| panic!("write {}: {e}", tmp.display()));
    // Atomic publish so a polling peer never reads a partial file.
    std::fs::rename(&tmp, path).unwrap_or_else(|e| panic!("rename {}: {e}", path.display()));
}

/// Poll until `path` exists, progressing `worker` so incoming UCX traffic
/// (including the peer's flush) is serviced while we wait.
fn wait_for_file(worker: &Worker, path: &Path, what: &str) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        worker.progress();
        match std::fs::read(path) {
            Ok(bytes) => return bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("read {}: {error}", path.display()),
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Drive an endpoint flush / close to completion.
fn wait_op(worker: &Worker, completion: ucx_sys::ep::OpCompletion<'_>, what: &str) {
    let ucx_sys::Completion::Pending(mut request) = completion else {
        return;
    };
    for _ in 0..1_000_000 {
        worker.progress();
        match request.try_complete() {
            Ok(Ok(())) => return,
            Ok(Err(failed)) => panic!("{what} failed: {}", failed.status),
            Err(still) => request = still,
        }
    }
    panic!("{what} did not complete");
}

#[test]
fn amo_post_cross_pe() {
    let (rank, size) = match (std::env::var("PMIX_RANK"), std::env::var("PMIX_SIZE")) {
        (Ok(rank), Ok(size)) => (
            rank.parse::<usize>().expect("PMIX_RANK"),
            size.parse::<usize>().expect("PMIX_SIZE"),
        ),
        _ => {
            eprintln!("skipping amo_post_cross_pe: run under prterun -np N -x PMIX_SIZE=N");
            return;
        }
    };
    assert!(size >= 2, "amo_post_cross_pe needs at least 2 ranks");
    let target_pe = (rank + 1) % size; // we post to the next PE in the ring
    eprintln!("[amo_post_dvm] rank {rank}/{size}, posting to PE {target_pe}");

    // Run-scoped exchange directory: PMIX_NAMESPACE is unique per prterun
    // session, so stale files from earlier runs can never be picked up.
    let namespace = std::env::var("PMIX_NAMESPACE").expect("PMIX_NAMESPACE");
    let dir: PathBuf = std::env::temp_dir().join(format!("ucx-rs-amo-post-dvm-{namespace}"));
    std::fs::create_dir_all(&dir).expect("create exchange dir");
    let info_path = |rank: usize| dir.join(format!("info.{rank}"));
    let done_path = |rank: usize| dir.join(format!("done.{rank}"));

    let params = ParamsBuilder::new()
        .features(Flags::Rma | Flags::Amo32 | Flags::Amo64)
        .mt_workers_shared(1)
        .estimated_num_eps(size)
        .build();
    let mut context =
        Context::new(&Config::read("", "").expect("config"), &params).expect("context");
    let worker = context
        .worker_create(&WorkerParamsBuilder::new().build())
        .expect("worker");

    // UCX-allocated target memory: on a no-RDMA box UCX allocates it in a
    // shared segment, so the peer's RMA lane (sysv/posix) can reach it.
    let target =
        AllocatedBuf::new(&context, TARGET_LEN, ucx_sys::MemoryType::Unknown).expect("allocate");
    let base = target.as_ptr() as u64;
    // Initialize before publishing: the peer only posts after reading our
    // file, and the file is written after this, so the ordering is safe.
    unsafe {
        (base as *mut u64).write(0); // add target
        ((base + XOR_OFFSET) as *mut u64).write(0); // xor target
        ((base + OR_OFFSET) as *mut u32).write(0); // or target
        ((base + AND_OFFSET) as *mut u32).write(u32::MAX); // and target
    }

    // Pack the worker address only after registering the memory, so it
    // advertises the shared-memory lane for cross-process RMA/AMO.
    let address = worker.pack_address().expect("pack address");
    let address = address.to_vec();
    let rkey = RemoteKey::pack(&context, target.mem_handle()).expect("pack rkey");
    write_file(&info_path(rank), &encode(&address, &rkey, base));
    eprintln!(
        "[amo_post_dvm] rank {rank} published address+rkey ({} bytes)",
        address.len()
    );

    let peer = decode(&wait_for_file(&worker, &info_path(target_pe), "peer info"));
    let remote = RemoteWorkerAddress::new(peer.address);
    let ep = worker
        .create_ep(EpParamsBuilder::new().address(&remote).build())
        .expect("endpoint");
    let peer_rkey = RemoteKey::unpack(&ep, &peer.rkey).expect("unpack rkey");

    // ── Post phase: all four families, fire-and-forget, no requests. ──
    for i in 0..POSTS {
        ep.amo_add64_post(&worker, 1, peer.base + ADD_OFFSET, &peer_rkey)
            .unwrap_or_else(|status| panic!("amo_add64_post[{i}]: {status}"));
    }
    for _ in 0..7 {
        ep.amo_xor64_post(
            &worker,
            0x0F0F_0F0F_0F0F_0F0F,
            peer.base + XOR_OFFSET,
            &peer_rkey,
        )
        .unwrap_or_else(|status| panic!("amo_xor64_post: {status}"));
    }
    for i in 0..32 {
        ep.amo_or32_post(&worker, 1u32 << i, peer.base + OR_OFFSET, &peer_rkey)
            .unwrap_or_else(|status| panic!("amo_or32_post[{i}]: {status}"));
    }
    for i in 0..32 {
        ep.amo_and32_post(&worker, !(1u32 << i), peer.base + AND_OFFSET, &peer_rkey)
            .unwrap_or_else(|status| panic!("amo_and32_post[{i}]: {status}"));
    }
    eprintln!(
        "[amo_post_dvm] rank {rank} posted {}+7+32+32 AMOs, no requests",
        POSTS
    );

    // The single flush lands and orders everything posted above.
    let param = RequestParamBuilder::new().build();
    let flush = ep
        .flush(&worker, &param)
        .unwrap_or_else(|status: Status| panic!("post ep flush: {status}"));
    wait_op(&worker, flush, "endpoint flush");

    // File barrier: only verify once the sender has flushed.
    write_file(&done_path(rank), &[1]);
    wait_for_file(&worker, &done_path(target_pe), "peer done marker");

    // ── Verify phase: read OUR targets, written by PE (rank - 1) % size. ──
    unsafe {
        let add = (base as *const u64).read();
        let xor = ((base + XOR_OFFSET) as *const u64).read();
        let or = ((base + OR_OFFSET) as *const u32).read();
        let and = ((base + AND_OFFSET) as *const u32).read();
        assert_eq!(add, POSTS as u64, "add64 target after {POSTS} posts");
        assert_eq!(xor, 0x0F0F_0F0F_0F0F_0F0F, "xor64 target (7 odd posts)");
        assert_eq!(or, u32::MAX, "or32 target after 32 posts");
        assert_eq!(and, 0, "and32 target after 32 posts");
    }
    eprintln!("[amo_post_dvm] rank {rank} verified all post-AMO targets");

    // Cleanup in reverse dependency order: rkey before ep (UCX destroy
    // order), then close; `target`, `worker` and `context` drop in reverse
    // declaration order at scope end (worker before context).
    drop(peer_rkey);
    let close = ep.close(&worker, CloseMode::Flush).expect("close ep");
    wait_op(&worker, close, "endpoint close");
    // Safe to remove our info file: the peer read it before posting (the
    // done barrier is past by now on both sides). The done markers are NOT
    // removed — the peer may not have observed ours yet, and the exchange
    // dir is keyed by PMIX_NAMESPACE, so a stale file can never satisfy a
    // future run's wait.
    let _ = std::fs::remove_file(info_path(rank));
}
