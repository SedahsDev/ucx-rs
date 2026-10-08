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
