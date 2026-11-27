//! Demonstrate a tagged send and receive on a self-connected endpoint.
//!
//! This uses UCX's single-process self-EP pattern; a real application would
//! create the endpoint from a peer's packed worker address.

use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::ep::ParamsBuilder as EpParamsBuilder;
use ucx_sys::worker::ParamsBuilder as WorkerParamsBuilder;
use ucx_sys::worker::Worker;
use ucx_sys::{Completion, RequestParamBuilder};

/// Drive one owned completion to its result with a shared progress loop.
fn wait<D, R>(
    worker: &Worker,
    completion: Completion<D, R>,
    mut poll: impl FnMut(R) -> Result<D, R>,
) -> D {
    let mut request = match completion {
        Completion::Immediate(data) => return data,
        Completion::Pending(request) => request,
    };
    for _ in 0..1_000_000 {
        worker.progress();
        match poll(request) {
            Ok(data) => return data,
            Err(still) => request = still,
        }
    }
    panic!("tag request timed out");
}

fn main() {
    let params = ParamsBuilder::new()
        .features(Flags::Tag)
        .mt_workers_shared(1)
        .estimated_num_eps(1)
        .build();
    let mut context =
        Context::new(&Config::read("", "").expect("config"), &params).expect("context");
    let worker = context
        .worker_create(&WorkerParamsBuilder::new().build())
        .expect("worker");
    let address = worker.pack_address().expect("pack address");
    let remote = ucx_sys::worker::RemoteWorkerAddress::new(address.to_vec());
    let ep = worker
        .create_ep(EpParamsBuilder::new().address(&remote).build())
        .expect("endpoint");
    drop(address);

    let request_param = RequestParamBuilder::new().no_imm_cmpl().build();
    // Both operations take ownership of their buffers and hand them back on
    // completion; the buffers cannot be touched while UCX is using them.
    let receive = worker
        .tag_recv(vec![0u8; 5], 0x44, u64::MAX, &request_param)
        .expect("post receive");
    let send = ep
        .tag_send(&worker, b"hello".to_vec(), 0x44, &request_param)
        .expect("post send");
    let (received, info) = wait(&worker, receive, |request| {
        request
            .try_complete()
            .map(|result| result.expect("tag receive"))
    });
    let _sent = wait(&worker, send, |request| {
        request
            .try_complete()
            .map(|result| result.expect("tag send"))
    });
    assert_eq!(&received, b"hello");
    assert_eq!((info.len(), info.sender_tag()), (5, 0x44));
    let close = ep
        .close(&worker, ucx_sys::ep::CloseMode::Flush)
        .expect("close endpoint");
    wait(&worker, close, |request| {
        request
            .try_complete()
            .map(|result| result.expect("endpoint close"))
    });
}
