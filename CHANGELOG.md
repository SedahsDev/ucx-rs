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
- **Breaking (#104, #108):** thread modes, memory types and endpoint error-handling modes
  use native enums instead of raw UCX enums.
  - New `ThreadMode`, `MemoryType` and `ErrorHandlerMode` enums, re-exported at the crate
    root. Setters such as `worker::ParamsBuilder::thread_mode`,
    `RequestParamBuilder::memory_type`, `memh::MemMapParamsBuilder::memory_type` and
    `ep::ParamsBuilder::err_mode` take them, and queries such as
    `version::LibAttr::max_thread_level` and `memh::MemAttr::mem_type` return them.
  - Removed the `ucs_thread_mode_t` and `ucs_memory_type_t` re-exports.
  - `ThreadMode` and `MemoryType` are `#[non_exhaustive]`, so variants can be added for
    newer UCX versions; a `match` on them outside this crate needs a wildcard arm.
  - `MemoryType::Unknown` (like leaving the memory type unset) asks UCX to detect the
    memory type. `ThreadMode::Unknown` is not a real thread mode: UCX never grants it, and
    `worker::ParamsBuilder::thread_mode` requests it as `ThreadMode::Multi`.
  - `listener::ListenerParams` used to be a type alias for `listener::ParamsBuilder`. It is
    now a distinct opaque type that only `ParamsBuilder::build()` can produce.
    `ParamsBuilder::build()` returns `ListenerParams` instead of the raw
    `ucp_listener_params`, and `Listener::create_with_params` takes `&ListenerParams`
    instead of `&ParamsBuilder`.
  - Migration: pass `ThreadMode::Multi` (or `Single` / `Serialized`) where code passed
    `ucs_thread_mode_t::UCS_THREAD_MODE_MULTI`, `MemoryType::Cuda` (and so on) where it
    passed `ucs_memory_type_t::UCS_MEMORY_TYPE_CUDA`, and `ErrorHandlerMode::Peer` / `None`
    where it passed `ucp_err_handling_mode_t::UCP_ERR_HANDLING_MODE_PEER` / `_NONE`. Call
    `.build()` on a listener `ParamsBuilder` before `Listener::create_with_params`, and
    replace `ListenerParams::new()` with `ParamsBuilder::new()`.
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

## 0.1.0

- Portable `build.rs` (`UCX_PREFIX` / include+lib env vars)
- Consolidated bindgen path with offline fallback
- Example: `version_and_context`
