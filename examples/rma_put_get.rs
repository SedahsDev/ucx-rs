//! Demonstrate self-endpoint RMA put/get using a packed and unpacked rkey.
//!
//! This exercises the intra-process loopback path. RMA support depends on the
//! active UCX transport configuration; setup errors are reported with expect().

use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::ep::ParamsBuilder as EpParamsBuilder;
use ucx_sys::memh::MemHandle;
use ucx_sys::rma::RemoteKey;
use ucx_sys::worker::ParamsBuilder as WorkerParamsBuilder;
use ucx_sys::worker::Worker;
use ucx_sys::{Completion, OwningRequest, Pending, RequestParamBuilder};

/// Progress the worker until an owned operation hands its buffer back.
fn complete<D>(worker: &Worker, completion: Completion<D, OwningRequest<'_, D>>) -> D {
    let mut request = match completion {
        Completion::Immediate(data) => return data,
        Completion::Pending(request) => request,
    };
    loop {
        worker.progress();
        match request.try_complete() {
            Ok(result) => return result.unwrap_or_else(|failed| panic!("{failed}")),
            Err(still) => request = still,
        }
    }
}

fn main() {
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
    let remote = ucx_sys::worker::RemoteWorkerAddress::new(address.to_vec());
    let ep = worker
        .create_ep(EpParamsBuilder::new().address(&remote).build())
        .expect("endpoint");
    drop(address);

    let mut target = [0u8; 5];
    let memh = MemHandle::map_slice(&context, &mut target, 0).expect("map target memory");
    let packed = RemoteKey::pack(&context, memh.mem_handle()).expect("pack rkey");
    let rkey = RemoteKey::unpack(&ep, &packed).expect("unpack rkey");
    let param = RequestParamBuilder::new().no_imm_cmpl().build();

    let put = ep
        .rma_put(
            &worker,
            b"hello".to_vec(),
            target.as_mut_ptr() as u64,
            &rkey,
            &param,
        )
        .expect("RMA put");
    let _source = complete(&worker, put);
    let flush = ep
        .flush(&RequestParamBuilder::new().build())
        .expect("flush");
    if let Pending::InProgress(request) = flush {
        while !request.check_finished().expect("flush") {
            worker.progress();
        }
    }
    assert_eq!(&target, b"hello");

    let get = ep
        .rma_get(
            &worker,
            vec![0u8; 5],
            target.as_mut_ptr() as u64,
            &rkey,
            &param,
        )
        .expect("RMA get");
    let result = complete(&worker, get);
    assert_eq!(&result, b"hello");
    ep.close(&worker, 0).expect("close endpoint");
}
