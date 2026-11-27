# Changelog

## Unreleased

- **Breaking (#208, part of #211):** flush, close and modify are owned,
  data-less operations returning `ep::OpCompletion<'w>` (= `Completion<(),
  OwningRequest<'w, ()>>`), driven by the same shared progress loop as data
  operations. There is no public single-request wait.
  - `Ep::flush(&self, worker, params)`, `Ep::modify(&self, worker, params)`,
    `Worker::flush(&self, params)`.
  - `Ep::close(self, worker, CloseMode)` returns `Result<OpCompletion,
    CloseError>`. The new `CloseMode { Flush, Force }` replaces raw `u32` flags.
    `CloseError::ep` hands the endpoint back when it was not consumed: a wrong
    worker was passed, or `Force` was refused for lack of peer error handling.
  - `CloseMode::Force` completes the endpoint's outstanding operations with
    `UCS_ERR_CANCELED`, which returns their buffers in `Failed`. It requires an
    endpoint created with `ErrorHandlerMode::Peer`, which `self`/`shm` don't
    support.
  - `flush`/`modify`/`close` check that the worker passed in is the
    endpoint's own (by its `alive` Arc identity, not the raw handle, which can be
    reused after a worker is destroyed). Otherwise they fail with
    `UCS_ERR_INVALID_PARAM`.
  - **Removed:** `Pending::wait`, `Worker::wait_request`,
    `Worker::cancel_request`, `MtWorker::wait_request`,
    `MtWorker::cancel_request`, `MtWorker::flush` (post and drive the
    flush inside `MtWorker::with_worker` with `w.progress()`; see its docs, since
    calling `MtWorker::progress` there deadlocks), and `Ep::disconnect_nb` (deprecated UCX API; use
    `close(_, CloseMode::Force)`). `Pending` remains only for the `unsafe`
    raw-pointer APIs.
  - Migration: `ep.close(&worker, 0)` becomes `ep.close(&worker,
    CloseMode::Flush)`, then drive the completion. `ep.flush(&p)` becomes
    `ep.flush(&worker, &p)`.
- **Fix (#214), safety:** the safe AM handler (`Worker::am_register_handler`)
  treated rendezvous data descriptors as payload, an out-of-bounds read from
  safe code. Rendezvous messages are now received into an owned buffer via
  `ucp_am_recv_data_nbx`, and the closure gets the real payload (possibly from a
  later `progress` call). For those messages the closure's status can't reach
  the sender and is ignored. A failed buffer allocation completes the sender
  with `UCS_ERR_NO_MEMORY`.
- **Breaking, safety:** `Worker::am_recv_data` is now `unsafe`. It accepted any
  `NonNull` pointer as a UCX descriptor from safe code.
- **Fix (#219):** `MemHandle::query` sets `field_mask`; it previously returned
  zero address and length for every mapped region.
- **Breaking (#207, part of #211):** all 20 safe AMO entry points own their
  operand and reply storage (new module `rma::amo`).
  - Fetching AMOs (`amo_fadd*`, `amo_fxor*`, `amo_fswap*`, `amo_fcswap*`, 32/64)
    no longer take `reply: &mut T`; they return
    `Completion<T, FetchAmoRequest<'w, T>>` with the old remote value
    (`FetchAmoCompletion<'w, T>`).
  - Non-fetching AMOs (`amo_add*`, `amo_xor*`, `amo_and*`, `amo_or*`,
    `amo_swap*`, `amo_cswap*`) take `&worker` as a new first argument and
    return `Completion<(), AmoRequest<'w>>` (`AmoCompletion<'w>`).
  - The old `FetchAmoRequest<'w, 'a, T>` borrowing guard and the
    callback-based `with_amo_reply` helper are removed.
  - **Fix:** every AMO passed UCX a pointer to its by-value operand on the
    callee's stack. UCX reads the operand until completion (`ucp_atomic_op_nbx`:
    "The user should not modify any part of the buffer ... until the operation
    completes"), so a deferred AMO read a dead stack slot. Operand and reply now
    live in one heap allocation owned by the request.
  - Fetch-CSWAP semantics from #196 are unchanged (swap value in the reply
    buffer on input, old value on output). Tests added for `amo_fcswap32/64`.

- **Breaking (#206, part of #211):** `Ep::am_send(&worker, id, header, data,
  &params)` takes ownership of the header and payload (`OwnedBuf`) and returns
  `Completion<(H, P), OwningRequest<'w, (H, P)>>`; `AmRequest` is removed.
  Byte literals still work (`b"..."`, `b""`).
  - `Worker::am_recv_data(desc, buffer, &params)` takes ownership of the
    receive buffer (`OwnedBufMut`) and returns `Completion<D, OwningRequest<'_,
    D>>`. **Fix:** it previously borrowed `&mut [u8]` and returned a bare
    request, so safe code could free the buffer while UCX wrote into it.
  - `OwnedBuf` is now also implemented for `&'static [u8]` and
    `&'static [u8; N]` (read-only, never moved or freed).
  - `am_recv_data` takes flags only from `params` (callback/user data are
    ignored for the same union-signature reason as tag/stream receives) and
    does not report the received length; use the AM callback's `length`.
  - Known issue: the safe AM handler mishandles rendezvous messages (#214).

- **Breaking (#205, part of #211):** tag and stream operations take ownership
  of their buffers (`OwnedBuf` / `OwnedBufMut`) and return `Completion`.
  `tag::TagRequest` and `stream::StreamRequest` are removed.
  - `Ep::tag_send(&worker, data, tag, &param)` and
    `Ep::tag_send_sync(&worker, data, tag, &param)` (new `worker` and `param`
    arguments) → `Completion<D, OwningRequest<'w, D>>`.
  - `Worker::tag_recv(data, tag, mask, &param)` (the redundant `worker`
    argument is gone) and `Worker::tag_msg_recv(data, message, &param)`
    (consumes the `MessageHandle`) → `Completion<(D, TagInfo),
    TagRecvRequest<'_, D>>`. The receive info comes back with the buffer on
    both the immediate and the deferred path.
  - `Ep::stream_send(&worker, data, &param)` →
    `Completion<D, OwningRequest<'w, D>>`; `Ep::stream_recv(&worker, buffer,
    &param)` → `Completion<(D, usize), StreamRecvRequest<'w, D>>` (bytes
    received).
  - New `RecvRequest<'w, D, I>` (aliases `TagRecvRequest`,
    `StreamRecvRequest`) with the same `try_complete` / `cancel` / `Drop`
    contract as `OwningRequest`.
  - **Fixes:** `tag_send_sync` and `tag_msg_recv` returned a bare request
    that did not borrow the buffer (use-after-free from safe code, #112);
    `tag_recv` wrote through a `*const` pointer; caller `RequestParam`
    datatype/reply/recv-info/memh/memory-type/request fields are now
    controlled by the call.
  - `TagInfo` now derives `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`.
  - Receive ops (`tag_recv`, `tag_msg_recv`, `stream_recv`) take **flags only**
    from `RequestParam`; a callback or user data is ignored. **Fix:** a
    `send_callback` was forwarded into UCX's callback union and invoked as a
    receive callback with a different signature (undefined behavior).
  - `tag_msg_recv` returns `TagMsgRecvError { status, message }`. The probed
    `MessageHandle` is handed back only when UCX provably did not consume it
    (`NO_RESOURCE` under `FORCE_IMM_CMPL`), so it can be retried; on every other
    error it may already be released, and it is not returned. `MessageHandle`
    now derives `Debug`.

- **Breaking (#204, part of #211):** `Ep::rma_put` / `Ep::rma_get` take
  ownership of the buffer and return
  `Result<Completion<D, OwningRequest<'w, D>>, Status>` (`D: OwnedBuf` for put,
  `D: OwnedBufMut` for get, e.g. `Vec<u8>`, `Box<[u8]>`, `Box<[u8; N]>`).
  The `rma::RmaRequest` borrowing guard is removed.
  - Migration: `ep.rma_put(&worker, data.to_vec(), addr, &rkey, &param)`;
    take the buffer back from `Completion::Immediate` or
    `OwningRequest::try_complete` after progressing the worker. A get's
    filled buffer comes back the same way.
  - **Fix:** the owned RMA calls now control the datatype (always contiguous
    bytes), reply buffer, memory handle, memory type, and request-allocation
    fields of the caller's `RequestParam`. Previously a caller datatype such as
    `dt_make_contig(2)` made UCX read `2 * len` bytes from a `len`-byte slice
    (out-of-bounds read from safe code). Flags, callback, and user data still
    apply.
  - `rma_put_ptr` / `rma_get_ptr` (raw and device pointers) are unchanged
    (#210).

- **New (#203, part of #211):** ownership-transferring completion model in
  `ucx_sys::completion` (re-exported at the crate root).
  - `Completion<D, R>`: `Immediate(D)` returns the data at once;
    `Pending(R)` holds an `OwningRequest` that owns it until UCX is done.
  - `OwningRequest<'w, D>`: tied to `&'w Worker`, so it cannot outlive its
    worker. `try_complete(self)` is a non-blocking poll returning
    `Ok(Ok(data))`, `Ok(Err(Failed { status, data }))` (errors, including
    `CANCELED`, hand the data back), or `Err(self)` while in flight.
    `cancel(&mut self)` only marks the request (`ucp_request_cancel`; no free,
    no progress). There is no public single-request wait.
  - `Drop` is the only progress-until-complete loop: cancel, at most
    `DROP_PROGRESS_BUDGET` (1,000,000) `Worker::progress()` rounds, then free
    and drop the data; on timeout both the request and the data are leaked.
    Never drop an `OwningRequest` inside a UCX callback.
  - `unsafe trait OwnedBuf` / `OwnedBufMut`: data must have a stable address
    across moves (`Vec<u8>`, `Box<[u8]>`, `Box<[u8; N]>`); inline storage
    such as `[u8; N]` or `u64` is rejected at compile time.
  - `unsafe fn status_ptr_with_data` adapts a raw `ucs_status_ptr_t` plus its
    owned data into a `Completion`.
  - `Failed<D>` `Debug`/`Display` show only the status, so `expect` works for
    any buffer type.
  - Operations move onto this model in #204–#208; `Pending` stays until then.

- **Breaking (#130, #112 slice 4):** `Ep::am_send` now returns a
  buffer-borrowing `am::AmRequest<'w, 'a>` guard, following the `TagRequest`
  and `StreamRequest` pattern.
  - The guard borrows the worker and both buffers (header and payload), and
    its `Drop` progresses the worker to completion before releasing the borrows.
  - New guard methods: `check_finished()`, `free()`, `into_inner()`, and `worker()`.
  - Migration: pass the worker as the first argument, e.g.,
    `ep.am_send(&worker, id, &header, &data, &params)`. Use
    `guard.check_finished()` and `guard.free()` instead of direct `Request`
    access. Dropping or reusing either buffer while an AM send is pending
    no longer compiles: this prevents the use-after-free tracked in #112.

- **Breaking (#127, #112 slice 1):** `Ep::rma_put` and `Ep::rma_get` now return
  buffer-borrowing `rma::RmaRequest<'w, 'a>` guards, following the
  `FetchAmoRequest` pattern.
  - The guard borrows the worker and buffer, and its `Drop` progresses the
    worker to completion before releasing the buffer borrow (bounded loop;
    on timeout the request is deliberately leaked).
  - New guard methods: `check_finished()`, `free()`, and `into_inner()`.
  - Migration: pass the worker as the first argument, e.g.
    `ep.rma_put(&worker, &data, ...)` and `ep.rma_get(&worker, &mut buf, ...)`.
    Use `guard.check_finished()` and `guard.free()` instead of direct
    `Request` access. Dropping or reusing the buffer while a request is
    pending no longer compiles: this prevents the use-after-free tracked
    in #112.

- **Fix (#110, slice 1):** Non-fetch AMO wrappers (`amo_add64/32`, `amo_xor64/32`,
  `amo_swap64/32`, `amo_and64/32`, `amo_or64/32`, `amo_cswap64/32`) now pass
  `count=1` and set the datatype internally via `ucp_dt_make_contig`, matching
  UCX's `UCP_AMO_CHECK_PARAM_NBX` requirements. Callers no longer need to set
  `.datatype()` for these operations. Added self-endpoint loopback tests for
  all 12 non-fetch AMO operations.

- UCX version support: 1.19 through 1.22 (inclusive). Versions below 1.19 or above 1.22 are not supported and will cause build-time errors.
- **Breaking (#128, #112):** `Ep::tag_send` and `Worker::tag_recv` now return
  buffer-borrowing `tag::TagRequest<'w, 'a>` guards, following the
  `stream::StreamRequest` pattern.
  - The guard explicitly borrows the worker and buffer slice, holds the
    underlying `Request`, and progresses the worker to completion on `Drop`
    before releasing the buffer borrow.
  - New guard methods: `check_finished()`, `free()`, `into_inner()`, and
    `worker()`.
  - Migration: pass the endpoint's `&Worker` as the first argument to
    `Ep::tag_send`. Use `guard.check_finished()` and `guard.free()` instead
    of direct `Request` access, and `guard.worker()` to access the borrowed
    worker. Use `unsafe { guard.into_inner() }` to recover the underlying
    `Request` only if the worker and buffer remain valid until completion.
    Dropping or reusing the buffer while a tag request is pending no longer
    compiles: this prevents the buffer use-after-free tracked in #112.
- **Breaking (#129):** `Ep::stream_send` and `Ep::stream_recv` now return a
  buffer-borrowing request guard, so safe code can no longer drop or reuse the
  buffer while a stream operation is in flight (use-after-free fix, #112 slice 3).
  - New `stream::StreamRequest<'w, 'a>` guard, following the same pattern as
    `rma::FetchAmoRequest`: it borrows the worker and the buffer, and its
    `Drop` progresses the worker to completion before releasing the buffer
    borrow (a bounded progress loop; on timeout the request is deliberately
    leaked rather than freed in flight).
  - `Ep::stream_send(worker, data, param)` now takes the endpoint's `&Worker`
    and returns `Result<Option<StreamRequest<'_, '_>>, Status>` instead of
    `Result<Option<Request>, Status>`.
  - `Ep::stream_recv(worker, buf, param)` now takes the endpoint's `&Worker`
    and returns `Result<StreamRequest<'_, '_>, Status>` instead of
    `Result<(Option<Request>, usize), Status>`. The received length is no
    longer an out-tuple field: read it from `StreamRequest::stream_recv_test()`
    (or keep using `Request::stream_recv_test` on the request extracted with
    `StreamRequest::into_inner`).
  - Migration: pass the worker as the first argument, e.g.
    `ep.stream_send(&worker, &data, &param)?` and
    `let guard = ep.stream_recv(&worker, &mut buf, &param)?;`. Code that kept
    the `Request` and progressed manually can call `guard.check_finished()`
    and `guard.free()`, or `unsafe { guard.into_inner() }` to recover the raw
    `Request` (the buffer must then stay valid until completion). Code that
    dropped the buffer while the request was pending no longer compiles: that
    was the use-after-free this change removes.
- **Breaking (#138):** `Worker::flush_nbx`, `Worker::flush_ep_nbx`, and `Ep::close`
  now return `Result<Pending, Status>` instead of an optional request result.
  - `Pending` enum has two variants: `Complete` (inline completion) and
    `InProgress(Request)` (operation in flight).
  - Migration: replace `if let Some(req) = ...` with `match ... { Pending::InProgress(req) => ..., Pending::Complete => ... }`.
  - Use `Pending::into_request()` to get an optional `Request` if needed, or `Pending::wait(&worker)` for a bounded progress loop.
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
- **Breaking (#107):** `ucp_mem_advice_t` and `ucp_conn_request_h` no longer appear in the public API.
  - New `MemAdvice` enum with `Normal` and `WillNeed` variants.
  - `memh::MemHandle::advice()` and `memh::MemAdviceParamsBuilder::advice()` take `MemAdvice`
    instead of `ucp_mem_advice_t`.
  - `ep::ParamsBuilder::conn_request()` takes `ConnRequest` by value instead of `ucp_conn_request_h`,
    consuming the request to prevent handle reuse.
  - Removed `ucp_mem_advice_t` from `memh.rs` public scope (it was already unreachable without `unsafe`).
  - Migration: use `MemAdvice::WillNeed` instead of `ucp_mem_advice_t::UCP_MADV_WILLNEED`,
    and pass `ConnRequest` directly to `conn_request()` instead of calling `.as_raw()`.
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
- **Fix (#126, slice 4 of #110):** the 14 deprecated atomic wrappers in `rma::atomic` now pass
  `count=1` with a proper contiguous datatype (`dt_make_contig(size_of::<T>())`) to
  `ucp_atomic_op_nbx` instead of byte sizes as `count`, which UCX rejected. Function signatures
  are unchanged; the fix is internal to each wrapper body.

## 0.1.0

- Portable `build.rs` (`UCX_PREFIX` / include+lib env vars)
- Consolidated bindgen path with offline fallback
- Example: `version_and_context`
