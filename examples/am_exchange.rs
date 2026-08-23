//! Demonstrate an active-message handler receiving a self-sent message.
//!
//! The handler writes the AM header into a local byte buffer. Real handlers
//! should also account for the callback's lifetime and any concurrent access.

use std::ffi::c_void;
use ucx_sys::am::{CbFlags, HandlerParamsBuilder};
use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::ep::ParamsBuilder as EpParamsBuilder;
use ucx_sys::worker::ParamsBuilder as WorkerParamsBuilder;
use ucx_sys::{Request, RequestParamBuilder};

unsafe extern "C" fn receive(
    arg: *mut c_void,
    header: *const c_void,
    header_length: usize,
    _data: *mut c_void,
    _length: usize,
    _param: *const ucx_sys::ucp_am_recv_param_t,
) -> ucx_sys::ucs_status_t {
    if arg.is_null() || header.is_null() || header_length == 0 {
        return ucx_sys::ucs_status_t::UCS_ERR_INVALID_PARAM;
    }
    // SAFETY: UCX supplies a valid header for this callback; arg points to the
    // one-byte buffer retained by main until the callback has run.
    unsafe {
        *(arg as *mut u8) = *(header as *const u8);
    }
    ucx_sys::ucs_status_t::UCS_OK
}

fn wait(worker: &ucx_sys::worker::Worker, request: Request) {
    for _ in 0..1_000_000 {
        if request.check_finished().expect("AM request failed") {
            return;
        }
        worker.progress();
    }
    panic!("AM request timed out");
}

fn main() {
    let params = ParamsBuilder::new()
        .features(Flags::Am)
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

    let mut received = [0u8; 1];
    let handler = HandlerParamsBuilder::new()
        .id(7)
        .flags(CbFlags::WholeMsg)
        .cb(receive)
        .arg(received.as_mut_ptr() as *mut c_void)
        .build();
    worker.am_register(&handler).expect("register AM handler");
    let request_param = RequestParamBuilder::new().no_imm_cmpl().build();
    if let Some(request) = ep.am_send(7, b"Z", &[], &request_param).expect("AM send") {
        wait(&worker, request);
    }
    assert_eq!(received, [b'Z']);
    ep.close(&worker, 0).expect("close endpoint");
}

// The ffi module is private in the library, so the callback signature cannot
// name its types from an external example. This alias is supplied below by the
// public API's inferred callback parameter.
