//! The owned-data completion model end to end, on a self endpoint.
//!
//! Many operations of different kinds (RMA puts, tag sends and receives, and
//! a fetching AMO when UCX defers it) are posted into ONE pending set and
//! completed by ONE shared `worker.progress()` + `try_complete` loop. The
//! fetch-AMO API takes no request params, so `NO_IMM_CMPL` can't force it to
//! defer: on the self transport it usually completes inline. A subset is canceled, the
//! endpoint is flushed and closed through the same loop shape, and every
//! buffer comes back to the caller, including those of canceled operations.
//!
//! Rules this example follows (see the crate docs, "Owned-data model"):
//! - Each operation takes ownership of its buffer and returns a
//!   `Completion`: `Immediate(data)` or `Pending(request)`. A request borrows
//!   the worker, so it can never outlive it.
//! - There is no public single-request `wait`. Progress the worker and poll
//!   everything you have in flight.
//! - `cancel()` is honored only by tag receives. A software tag receive
//!   completes (with `UCS_ERR_CANCELED`) inside the `cancel()` call, and an
//!   offloaded one from a later progress call. Either way `try_complete` hands
//!   the buffer back. Other operations ignore cancel.
//! - The rkey is dropped before the endpoint is closed (UCX destroy order).
//! - Never progress the worker or drop a pending request inside a UCX
//!   callback.
//!
//! Run with `UCX_TLS=self cargo run --example owned_pipeline`. Fetching AMOs
//! need a transport with atomics; where UCX has none the AMO is skipped.

use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::ep::{CloseMode, OpCompletion, ParamsBuilder as EpParamsBuilder};
use ucx_sys::memh::MemHandle;
use ucx_sys::rma::{FetchAmoRequest, RemoteKey};
use ucx_sys::tag::TagInfo;
use ucx_sys::worker::{ParamsBuilder as WorkerParamsBuilder, RemoteWorkerAddress, Worker};
use ucx_sys::{Completion, Failed, OwningRequest, RequestParamBuilder, Status, TagRecvRequest};

const PUTS: usize = 4;
const PUT_LEN: usize = 512;
const UNMATCHED_TAG: u64 = 0xDEAD;

/// One in-flight operation of any kind. Each variant's request borrows the
/// worker (`'w`), so the whole pending set does too.
enum Op<'w> {
    Put(usize, OwningRequest<'w, Vec<u8>>),
    Send(u64, OwningRequest<'w, Vec<u8>>),
    Recv(u64, TagRecvRequest<'w, Vec<u8>>),
    FetchAdd(FetchAmoRequest<'w, u64>),
}

/// What a completed operation handed back.
enum Done {
    Put(usize, Vec<u8>),
    Sent(u64, Vec<u8>),
    Received(Vec<u8>, TagInfo),
    Canceled(u64, Vec<u8>),
    Fetched(u64),
}

impl<'w> Op<'w> {
    /// Non-blocking poll: the outcome, or the operation back if still in flight.
    fn poll(self) -> Result<Done, Op<'w>> {
        match self {
            Op::Put(i, request) => match request.try_complete() {
                Ok(result) => Ok(Done::Put(i, result.unwrap_or_else(|f| panic!("put: {f}")))),
                Err(request) => Err(Op::Put(i, request)),
            },
            Op::Send(tag, request) => match request.try_complete() {
                Ok(result) => Ok(Done::Sent(
                    tag,
                    result.unwrap_or_else(|f| panic!("send: {f}")),
                )),
                Err(request) => Err(Op::Send(tag, request)),
            },
            Op::Recv(tag, request) => match request.try_complete() {
                Ok(Ok((buffer, info))) => Ok(Done::Received(buffer, info)),
                // A canceled receive still returns its buffer.
                Ok(Err(Failed { status, data })) if status == Status::CANCELED => {
                    Ok(Done::Canceled(tag, data))
                }
                Ok(Err(failed)) => panic!("recv {tag:#x}: {failed}"),
                Err(request) => Err(Op::Recv(tag, request)),
            },
            Op::FetchAdd(request) => match request.try_complete() {
                Ok(result) => Ok(Done::Fetched(
                    result.unwrap_or_else(|s| panic!("fadd: {s}")),
                )),
                Err(request) => Err(Op::FetchAdd(request)),
            },
        }
    }

    fn cancel(&mut self) {
        match self {
            Op::Put(_, request) | Op::Send(_, request) => request.cancel(),
            Op::Recv(_, request) => request.cancel(),
            Op::FetchAdd(request) => request.cancel(),
        }
    }
}

/// The shared progress loop: progress once, poll everything, keep what is
/// still in flight. Bounded so a broken transport fails instead of hanging.
fn drain<'w>(worker: &'w Worker, mut pending: Vec<Op<'w>>) -> Vec<Done> {
    let mut done = Vec::new();
    for _ in 0..1_000_000 {
        if pending.is_empty() {
            return done;
        }
        worker.progress();
        let mut still = Vec::with_capacity(pending.len());
        for op in pending.drain(..) {
            match op.poll() {
                Ok(outcome) => done.push(outcome),
                Err(op) => still.push(op),
            }
        }
        pending = still;
    }
    panic!("{} operations did not complete", pending.len());
}

/// Drive a data-less completion (flush, close) with the same loop shape.
fn finish(worker: &Worker, completion: OpCompletion<'_>, what: &str) {
    let Completion::Pending(mut request) = completion else {
        return;
    };
    for _ in 0..1_000_000 {
        worker.progress();
        match request.try_complete() {
            Ok(result) => return result.unwrap_or_else(|failed| panic!("{what}: {failed}")),
            Err(still) => request = still,
        }
    }
    panic!("{what} did not complete");
}

fn main() {
    let params = ParamsBuilder::new()
        .features(Flags::Rma | Flags::Tag | Flags::Amo64)
        .mt_workers_shared(1)
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
        .expect("self endpoint");

    // Registered target memory: an 8-byte-aligned counter, then the put area.
    let mut target = vec![0u8; 8 + 8 + PUTS * PUT_LEN];
    let base = target.as_mut_ptr() as u64;
    let counter = ((8 - (base % 8)) % 8) as usize; // aligned offset, no unsafe
    let puts_at = counter + 8;
    target[counter..counter + 8].copy_from_slice(&40u64.to_ne_bytes());
    let memh = MemHandle::map_slice(&context, &mut target, 0).expect("map target");
    let packed = RemoteKey::pack(&context, memh.mem_handle()).expect("pack rkey");
    let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");

    // NO_IMM_CMPL makes every operation take the deferred path, so the loop
    // below really has work to do even on the inline-happy self transport.
    let deferred = RequestParamBuilder::new().no_imm_cmpl().build();
    let mut pending: Vec<Op<'_>> = Vec::new();
    for i in 0..PUTS {
        let payload = vec![i as u8 + 1; PUT_LEN];
        let addr = base + (puts_at + i * PUT_LEN) as u64;
        match ep
            .rma_put(&worker, payload, addr, &rkey, &deferred)
            .expect("post put")
        {
            Completion::Pending(request) => pending.push(Op::Put(i, request)),
            Completion::Immediate(_) => unreachable!("NO_IMM_CMPL put"),
        }
    }
    for tag in [1u64, 2] {
        match worker
            .tag_recv(vec![0u8; 64], tag, u64::MAX, &deferred)
            .expect("post recv")
        {
            Completion::Pending(request) => pending.push(Op::Recv(tag, request)),
            Completion::Immediate(_) => unreachable!("NO_IMM_CMPL receive"),
        }
        let message = format!("hello #{tag}").into_bytes();
        match ep
            .tag_send(&worker, message, tag, &deferred)
            .expect("post send")
        {
            Completion::Pending(request) => pending.push(Op::Send(tag, request)),
            Completion::Immediate(_) => {}
        }
    }
    // Two receives nobody will ever match: canceled below.
    for k in 0..2 {
        let tag = UNMATCHED_TAG + k;
        match worker
            .tag_recv(vec![0u8; 64], tag, u64::MAX, &deferred)
            .expect("post recv")
        {
            Completion::Pending(request) => pending.push(Op::Recv(tag, request)),
            Completion::Immediate(_) => unreachable!("NO_IMM_CMPL receive"),
        }
    }
    let counter_addr = base + counter as u64;
    let amo = match ep.amo_fadd64(&worker, 2, counter_addr, &rkey) {
        Ok(Completion::Pending(request)) => {
            pending.push(Op::FetchAdd(request));
            true
        }
        Ok(Completion::Immediate(old)) => {
            assert_eq!(old, 40, "fetch-add returns the previous value");
            println!("fetch-add completed inline, old value {old}");
            true
        }
        Err(status) => {
            println!("skipping fetch-add: {status} (no atomics on this transport)");
            false
        }
    };

    // Cancel a subset: only the unmatched receives. On the software tag path
    // UCX completes them with CANCELED inside cancel() itself (offloaded ones
    // would need progress); either way the shared loop below collects them.
    for op in &mut pending {
        if matches!(op, Op::Recv(tag, _) if *tag >= UNMATCHED_TAG) {
            op.cancel();
        }
    }

    println!("{} operations in flight", pending.len());
    let done = drain(&worker, pending);
    let (mut puts, mut sent, mut received, mut canceled) = (0, 0, 0, 0);
    for outcome in done {
        match outcome {
            Done::Put(i, buffer) => {
                assert_eq!(buffer, vec![i as u8 + 1; PUT_LEN], "put buffer handed back");
                puts += 1;
            }
            Done::Sent(tag, buffer) => {
                assert_eq!(buffer, format!("hello #{tag}").into_bytes());
                sent += 1;
            }
            Done::Received(buffer, info) => {
                let text = std::str::from_utf8(&buffer[..info.len()]).expect("utf-8");
                println!("received {text:?} (tag {})", info.sender_tag());
                received += 1;
            }
            Done::Canceled(tag, buffer) => {
                assert_eq!(buffer.len(), 64, "canceled receive returns its buffer");
                println!("receive {tag:#x} canceled; buffer returned");
                canceled += 1;
            }
            Done::Fetched(old) => {
                assert_eq!(old, 40, "fetch-add returns the previous value");
                println!("fetch-add returned old value {old}");
            }
        }
    }
    assert_eq!((puts, sent, received, canceled), (PUTS, 2, 2, 2));

    // Remote visibility: flush the endpoint (same completion loop shape).
    let flush = ep
        .flush(&worker, &RequestParamBuilder::new().build())
        .expect("flush");
    finish(&worker, flush, "endpoint flush");
    drop(rkey); // after its operations are flushed, before the endpoint goes
    drop(memh);
    for i in 0..puts {
        let at = puts_at + i * PUT_LEN;
        assert!(
            target[at..at + PUT_LEN].iter().all(|&b| b == i as u8 + 1),
            "put {i}"
        );
    }
    if amo {
        let value = u64::from_ne_bytes(target[counter..counter + 8].try_into().unwrap());
        assert_eq!(value, 42, "fetch-add applied");
    }

    let close = ep.close(&worker, CloseMode::Flush).expect("close endpoint");
    finish(&worker, close, "endpoint close");
    println!("done: {puts} puts, {sent} sends, {received} receives, {canceled} canceled");
}
