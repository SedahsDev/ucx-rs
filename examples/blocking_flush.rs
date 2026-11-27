//! Demonstrate a bounded blocking flush built from the non-blocking API.

use ucx_sys::am::CbFlags;
use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::ep::ParamsBuilder as EpParamsBuilder;
use ucx_sys::worker::ParamsBuilder as WorkerParamsBuilder;
use ucx_sys::{Completion, OwningRequest, RequestParamBuilder};

/// Drive one owned completion with the shared progress loop.
fn complete<D>(
    worker: &ucx_sys::worker::Worker,
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
            Ok(result) => return result.unwrap_or_else(|failed| panic!("{what}: {failed}")),
            Err(still) => request = still,
        }
    }
    panic!("{what} timed out");
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
        .am_send(&worker, 0, b"", b"", &request_param)
        .expect("active-message send");
    // The AM send owns its (empty) header and payload until it completes.
    let _buffers = complete(&worker, operation, "active-message send");
    let flush = ep.flush(&worker, &request_param).expect("start flush");
    complete(&worker, flush, "endpoint flush");
    let close = ep
        .close(&worker, ucx_sys::ep::CloseMode::Flush)
        .expect("close endpoint");
    complete(&worker, close, "endpoint close");
}
