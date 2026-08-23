//! Minimal UCX example: print version and create a UCP context + worker.
//!
//! ```text
//! export UCX_PREFIX=/path/to/ucx
//! cargo run --example version_and_context
//! ```

use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::version;
use ucx_sys::worker::ParamsBuilder as WorkerParamsBuilder;

fn main() {
    let (maj, min, rel) = version::get_version();
    println!(
        "UCX version {}.{}.{} ({})",
        maj,
        min,
        rel,
        version::get_version_string()
    );

    if let Ok(attr) = version::lib_query() {
        println!("max_thread_level = {:?}", attr.max_thread_level);
    }

    let mut pb = ParamsBuilder::new();
    pb.features(Flags::Tag | Flags::Rma | Flags::Am)
        .mt_workers_shared(1)
        .estimated_num_eps(2)
        .name("ucx-rs-example")
        .expect("context name");
    let params = pb.build();
    let config = Config::read("", "").expect("config read");

    let mut context = Context::new(&config, &params).expect("Context::new failed");
    println!("UCP context created");

    let worker_params = WorkerParamsBuilder::new().build();
    let worker = context
        .worker_create(&worker_params)
        .expect("worker_create failed");
    println!("UCP worker created");

    while worker.progress() {}

    let addr = worker.pack_address().expect("pack_address");
    println!("worker address bytes: {}", addr.to_slice().len());

    drop(addr);
    drop(worker);
    drop(context);
    println!("cleanup complete");
}
