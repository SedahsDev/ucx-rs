# Changelog

## Unreleased

- Added opt-in, thread-mode-checked `worker::MtWorker` access.
- The minimum supported Rust version is now 1.78 (`rust-version`), and CI checks it. The old
  value, 1.63, could never build the crate: the code uses `let`-`else` (Rust 1.65) and
  `std::os::fd` (1.66), the `bindgen` build-dependency needs 1.70, its `rustc-hash` 2.1
  dependency needs 1.77, and the committed `Cargo.lock` (format v4) needs Cargo 1.78.
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
