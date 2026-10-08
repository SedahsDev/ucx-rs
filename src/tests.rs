//! Helpers shared by the unit tests of several modules (compiled only for `cargo test`).

use crate::context;
use crate::context::Context;
use crate::ep;
use crate::worker;
use crate::worker::RemoteWorkerAddress;
use std::rc::Rc;

extern "C" fn init(_request: *mut ::std::os::raw::c_void) {}

extern "C" fn cleanup(_request: *mut ::std::os::raw::c_void) {}

/// A context, a worker and an endpoint connected to that worker itself.
///
/// Fields drop in declaration order: endpoint, then worker, then context.
pub struct CommsContext {
    pub ep: ep::Ep,
    pub worker: worker::Worker,
    #[allow(dead_code)]
    pub context: context::Context,
}

pub fn setup_default() -> Rc<CommsContext> {
    let features = context::Flags::Am
        | context::Flags::Rma
        | context::Flags::Amo32
        | context::Flags::Amo64
        | context::Flags::Tag;

    let params = context::ParamsBuilder::new()
        .features(features)
        .mt_workers_shared(1)
        .request_init(init)
        .request_cleanup(cleanup)
        .request_size(8)
        .name("My Awesome Test")
        .expect("context name")
        .tag_sender_mask(u64::MAX)
        .estimated_num_eps(4)
        .estimated_num_ppn(2)
        .build();

    let worker_features = worker::ParamsBuilder::new()
        .thread_mode(crate::ThreadMode::Multi)
        .build();

    let mut context = Context::new(
        &context::Config::read("", "").expect("config read"),
        &params,
    )
    .unwrap();

    let worker = context.worker_create(&worker_features).unwrap();
    let packed_addr = worker.pack_address().unwrap();
    let addr = RemoteWorkerAddress::new(packed_addr.to_vec());

    let ep_param = ep::ParamsBuilder::new().address(&addr).build();
    let ep = worker.create_ep(ep_param).unwrap();
    // If we don't drop this than the compiler complains about how the
    // worker is borrowed in the packed_addr.
    drop(packed_addr);

    let mut progressed = worker.progress();
    while progressed {
        progressed = worker.progress();
    }
    Rc::new(CommsContext {
        context,
        worker,
        ep,
    })
}
