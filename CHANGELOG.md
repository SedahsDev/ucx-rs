# Changelog

## Unreleased

- Added opt-in, thread-mode-checked `worker::MtWorker` access.
- **Breaking (#73):** `ucs_status_t` no longer appears in the public API.
  - `Status` is `#[repr(transparent)]` and has associated constants for the UCX status codes
    (`Status::OK`, `Status::IN_PROGRESS`, `Status::INVALID_PARAM`, ...).
  - `am::AmRecvCb` callbacks return `Status`; `ep::ErrHandlerCb` and `SendCallback` callbacks
    receive `status: Status`.
  - `Worker::am_register_handler` closures return `Result<(), Status>`; `Ok(())` is reported to
    UCX as `UCS_OK`.
  - Removed the `am::ucs_status_t` re-export.
- **Breaking (#106):** parameter builders now borrow the addresses they point at, so safe code
  can no longer hand UCX a freed socket or worker address.
  - `listener::ParamsBuilder`, `listener::ListenerParamsBuilder` and `listener::ListenerParams`
    have a lifetime parameter (`ParamsBuilder<'a>`); `ParamsBuilder::sockaddr` takes
    `&'a SockAddr`. `Listener::create_with_params` takes `&ListenerParams<'_>`.
  - `ep::ParamsBuilder` and `ep::Params` have a lifetime parameter; `ParamsBuilder::sockaddr`
    takes `&'a SockAddr` and `ParamsBuilder::address` takes `&'a RemoteWorkerAddress`.
    `Ep::new`, `Worker::create_ep` and `MtWorker::create_ep` take `ep::Params<'_>`.
  - Migration: builder chains such as
    `Listener::create_with_params(&worker, &ParamsBuilder::new().sockaddr(&sa).build())` and
    `worker.create_ep(EpParamsBuilder::new().address(&remote).build())` are unchanged. Code
    that names these types in its own signatures or struct fields adds a lifetime (for
    example `ep::Params<'_>`). Code that dropped the `SockAddr` or `RemoteWorkerAddress`
    before the create call no longer compiles: keep the address alive until the listener or
    endpoint has been created (UCX copies it during creation, so it may be dropped after).
- The minimum supported Rust version is now 1.78 (`rust-version`), and CI checks it. The old
  value, 1.63, could never build the crate: the code uses `let`-`else` (Rust 1.65) and
  `std::os::fd` (1.66), the `bindgen` build-dependency needs 1.70, its `rustc-hash` 2.1
  dependency needs 1.77, and the committed `Cargo.lock` (format v4) needs Cargo 1.78.
- Added `Worker::am_unregister(id)` to remove an active-message handler (#99). Closure state of
  handlers registered with `Worker::am_register_handler` is still retained until the worker is
  dropped.
- **Breaking (#77):** the raw AM receive-parameter and stream-poll-entry structs no longer appear
  in the public API.
  - `am::AmRecvCb` callbacks receive `param: *const am::AmRecvParam` (a `#[repr(transparent)]`
    native wrapper) instead of `*const am::ucp_am_recv_param_t`. Read it with
    `AmRecvParam::recv_attr()` (new `am::AmRecvAttr` flags) and `AmRecvParam::reply_ep()`.
  - Removed the `am::ucp_am_recv_param_t` re-export (`am::ucs_status_t` stays until #73).
  - `stream::StreamPollEp` is now a native `#[repr(transparent)]` wrapper instead of a
    re-export of the raw struct, and its fields are private. Create entries with
    `StreamPollEp::default()` and read them with `ep_handle()`, `user_data()` and `flags()`.
  - Migration: in raw AM callbacks replace `*const ucp_am_recv_param_t` with
    `*const AmRecvParam`; replace `entry.ep` / `entry.user_data` / `entry.flags` field reads with
    the method calls.

## 0.1.0

- Portable `build.rs` (`UCX_PREFIX` / include+lib env vars)
- Consolidated bindgen path with offline fallback
- Example: `version_and_context`
