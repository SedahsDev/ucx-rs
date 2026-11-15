//! Demonstrate a bounded blocking flush built from the non-blocking API.

use ucx_sys::am::CbFlags;
use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::ep::ParamsBuilder as EpParamsBuilder;
use ucx_sys::worker::ParamsBuilder as WorkerParamsBuilder;
use ucx_sys::{Pending, RequestParamBuilder};

fn wait_request<R, E: std::fmt::Debug>(
    worker: &ucx_sys::worker::Worker,
    pending: Pending<R>,
    mut check_finished: impl FnMut(&mut R) -> Result<bool, E>,
) {
    let mut request = match pending {
        Pending::Complete => return,
        Pending::InProgress(request) => request,
    };
    const MAX_ROUNDS: usize = 1_000_000;
    for _ in 0..MAX_ROUNDS {
        match check_finished(&mut request) {
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
    let mut context =
        Context::new(&Config::read("", "").expect("config"), &params).expect("context");
    let mut worker = context
        .worker_create(&WorkerParamsBuilder::new().build())
        .expect("worker");
    let address = worker.pack_address().expect("address");
    let remote = ucx_sys::worker::RemoteWorkerAddress::new(address.to_vec());
    let ep = worker
        .create_ep(EpParamsBuilder::new().address(&remote).build())
        .expect("endpoint");
    drop(address);

    // Register an AM handler before sending; Ep::am_send requires one to be
    // registered for the AM id, otherwise the receive path may segfault.
    let am_handler = |_header: &[u8], data: &[u8]| {
        println!("received AM data: {data:?}");
        Ok(())
    };
    worker
        .am_register_handler(0, CbFlags::empty(), am_handler)
        .expect("register active-message handler");

    // Issue an AM operation, then synchronously wait for the endpoint flush.
    // A self endpoint is enough to make this example runnable without a peer.
    let request_param = RequestParamBuilder::new().build();
    let operation = ep
        .am_send(&worker, 0, &[], &[], &request_param)
        .expect("active-message send");
    let flush = ep.flush(&request_param).expect("start flush");
    wait_request(&worker, flush, |request| request.check_finished());
    wait_request(&worker, operation, |request| request.check_finished());
    ep.close(&worker, 0).expect("close endpoint");
}
