//! Demonstrate self-endpoint RMA put/get using a packed and unpacked rkey.
//!
//! This exercises the intra-process loopback path. RMA support depends on the
//! active UCX transport configuration; setup errors are reported with expect().

use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::ep::ParamsBuilder as EpParamsBuilder;
use ucx_sys::memh::MemHandle;
use ucx_sys::rma::RemoteKey;
use ucx_sys::worker::ParamsBuilder as WorkerParamsBuilder;
use ucx_sys::RequestParamBuilder;

fn wait(worker: &ucx_sys::worker::Worker, request: ucx_sys::Request) {
    for _ in 0..1_000_000 {
        if request.check_finished().expect("RMA request failed") {
            return;
        }
        worker.progress();
    }
    panic!("RMA request timed out");
}

fn main() {
    let params = ParamsBuilder::new()
        .features(Flags::Rma)
        .mt_workers_shared(1)
        .estimated_num_eps(1)
        .build();
    let context = Context::new(&Config::read("", "").expect("config"), &params).expect("context");
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

    if let Some(request) = ep
        .rma_put(b"hello", target.as_mut_ptr() as u64, &rkey, &param)
        .expect("RMA put")
    {
        wait(&worker, request);
    }
    assert_eq!(&target, b"hello");

    let mut result = [0u8; 5];
    if let Some(request) = ep
        .rma_get(&mut result, target.as_mut_ptr() as u64, &rkey, &param)
        .expect("RMA get")
    {
        wait(&worker, request);
    }
    assert_eq!(&result, b"hello");
    ep.close(&worker, 0).expect("close endpoint");
}
