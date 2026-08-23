//! Demonstrate a bounded blocking flush built from the non-blocking API.

use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::ep::ParamsBuilder as EpParamsBuilder;
use ucx_sys::worker::ParamsBuilder as WorkerParamsBuilder;
use ucx_sys::{Request, RequestParamBuilder};

fn wait_request(worker: &ucx_sys::worker::Worker, request: Request) {
    const MAX_ROUNDS: usize = 1_000_000;
    for _ in 0..MAX_ROUNDS {
        match request.check_finished() {
            Ok(true) => return,
            Ok(false) => {
                worker.progress();
            }
            Err(status) => panic!("flush failed: {status:?}"),
        }
    }
    panic!("flush timed out");
}

fn main() {
    let params = ParamsBuilder::new()
        .features(Flags::Am | Flags::Rma | Flags::Amo32 | Flags::Amo64 | Flags::Tag)
        .mt_workers_shared(1)
        .estimated_num_eps(1)
        .build();
    let context = Context::new(&Config::read("", "").expect("config"), &params).expect("context");
    let worker = context
        .worker_create(&WorkerParamsBuilder::new().build())
        .expect("worker");
    let address = worker.pack_address().expect("address");
    let remote = ucx_sys::worker::RemoteWorkerAddress::new(address.to_vec());
    let ep = worker
        .create_ep(EpParamsBuilder::new().address(&remote).build())
        .expect("endpoint");
    drop(address);

    // Issue an AM operation, then synchronously wait for the endpoint flush.
    // A self endpoint is enough to make this example runnable without a peer.
    let request_param = RequestParamBuilder::new().build();
    let operation = ep
        .am_send(0, &[], &[], &request_param)
        .expect("active-message send");
    let flush = ep.flush(&request_param).expect("start flush");
    if let Some(request) = flush {
        wait_request(&worker, request);
    }
    if let Some(request) = operation {
        wait_request(&worker, request);
    }
    ep.close(&worker, 0).expect("close endpoint");
}
