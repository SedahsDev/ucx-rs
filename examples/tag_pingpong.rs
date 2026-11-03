//! Demonstrate a tagged send and receive on a self-connected endpoint.
//!
//! This uses UCX's single-process self-EP pattern; a real application would
//! create the endpoint from a peer's packed worker address.

use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::ep::ParamsBuilder as EpParamsBuilder;
use ucx_sys::worker::ParamsBuilder as WorkerParamsBuilder;
use ucx_sys::RequestParamBuilder;

fn wait(worker: &ucx_sys::worker::Worker, mut request: ucx_sys::tag::TagRequest<'_, '_>) {
    const MAX_ROUNDS: usize = 1_000_000;
    for _ in 0..MAX_ROUNDS {
        match request.check_finished().expect("tag request failed") {
            true => return,
            false => {
                worker.progress();
            }
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

    let mut received = [0u8; 5];
    let request_param = RequestParamBuilder::new().no_imm_cmpl().build();
    let receive = worker
        .tag_recv(&worker, &mut received, 0x44, u64::MAX, &request_param)
        .expect("post receive");
    let send = ep
        .tag_send(&worker, b"hello", 0x44, &request_param)
        .expect("post send");
    wait(&worker, receive);
    if let Some(request) = send {
        wait(&worker, request);
    }
    assert_eq!(&received, b"hello");
    ep.close(&worker, 0).expect("close endpoint");
}
