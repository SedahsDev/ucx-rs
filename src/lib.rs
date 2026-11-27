//! Rust bindings for UCX's UCP API.
//!
//! # Threading model
//!
//! The default policy is a single-threaded progress loop. `Context` is
//! `Send + Sync` (UCX documents the context as shareable; worker creation still
//! needs exclusive `&mut Context`). `Worker`, `Ep`, the owned requests and the
//! handle wrappers such as `MemHandle` and `RemoteKey` are intentionally `!Send`
//! and `!Sync` (pinned in `src/threading_assert.rs`). For cross-thread access to
//! one worker, `worker::MtWorker` is the audited `Send + Sync` wrapper; making
//! the other handles transferable under `ThreadMode::Multi` is future work. See
//! `THREADING.md` for the full inventory.
//!
//! `worker::ParamsBuilder::thread_mode` selects the UCX contract for calls on
//! that worker:
//!
//! * `ThreadMode::Single` permits calls from one thread only.
//! * `ThreadMode::Serialized` permits multiple callers, but the application
//!   must serialize UCX calls.
//! * `ThreadMode::Multi` permits concurrent UCX calls where UCX documents
//!   them as thread-safe, but it does not make these Rust wrapper values
//!   transferable or remove application-level protocol synchronization.
//!
//! `Worker::progress()` should have one owning progress thread per worker. In
//! an MT-enabled UCX build with `ThreadMode::Multi`, concurrent progress is
//! safe but contended by UCX's internal spinlock; in non-MT builds or
//! `ThreadMode::Single`, it is undefined behavior. See `THREADING.md` §2.1.
//! Use `Worker::arm()` and `Worker::get_efd()` to wake the owning loop instead
//! of polling concurrently from multiple threads.
//! Operations own their buffers until UCX has completed them (see the next
//! section). Always drop/close endpoints before their `Worker`.
//! `Ep::Drop` checks the runtime `worker_alive` guard and skips a late cleanup as
//! a safety net; that guard is not a license to violate the required drop order.
//!
//! # Owned-data model
//!
//! Non-blocking operations take **ownership** of their data and return a
//! [`Completion`]: `Immediate(data)` when UCX finished inside the call, or
//! `Pending(request)`, an [`OwningRequest`] (or a typed wrapper such as
//! [`TagRecvRequest`] or [`rma::FetchAmoRequest`]) that holds the data until
//! the operation has really finished. Flush, close and modify are the
//! data-less case ([`ep::OpCompletion`]). This replaces the old borrowing
//! wrappers (#112): a borrowed buffer could be dropped while UCX was still
//! using it, and freeing an in-flight request does not stop the operation
//! (#68).
//!
//! The rules:
//!
//! * **Request lifetime ⊆ worker lifetime.** Requests borrow the worker
//!   (`&'w Worker`), so a request can't outlive it. Likewise
//!   [`rma::RemoteKey`] must be dropped before the endpoint it was unpacked on
//!   (UCX's destroy order).
//! * **Progress many requests together.** There is no public single-request
//!   `wait`. Call `worker.progress()`, then `try_complete` every request in
//!   flight, and keep the ones that hand themselves back. UCX already
//!   serializes `ucp_worker_progress` internally. See
//!   `examples/owned_pipeline.rs`:
//!
//!   ```ignore
//!   while !pending.is_empty() {
//!       worker.progress();
//!       let mut still = Vec::with_capacity(pending.len());
//!       for request in pending.drain(..) {
//!           match request.try_complete() {
//!               Ok(result) => handle(result), // data back, or Failed { status, data }
//!               Err(request) => still.push(request),
//!           }
//!       }
//!       pending = still;
//!   }
//!   ```
//! * **Never progress the worker, and never drop a pending request, inside a
//!   UCX callback** (AM handlers, send/receive callbacks, listener callbacks).
//!   A request's `Drop` progresses the worker.
//! * **`cancel()` is not a general abort.** In UCX 1.18–1.22 only tag
//!   receives honor it: a software tag receive completes inside the
//!   `cancel()` call itself, and an offloaded one from a later progress call.
//!   The status is usually `UCS_ERR_CANCELED` (data back in [`Failed`]), but
//!   success if the receive was already matched or the offloaded cancel lost
//!   the race — then the data comes back on the success side. Sends, RMA,
//!   AMO, AM, stream and flush requests ignore cancel and complete normally
//!   (see [`OwningRequest::cancel`]). To
//!   abort an endpoint's operations, close it with [`ep::CloseMode::Force`].
//!   Force requires an endpoint created with [`ErrorHandlerMode::Peer`], which
//!   `self`/`shm` don't support.
//! * **`Drop` is a bounded fail-safe, not a wait.** Dropping a pending request
//!   cancels it and progresses the worker up to [`DROP_PROGRESS_BUDGET`]
//!   times. If it still hasn't completed, the request **and its data are
//!   leaked** rather than freed while UCX may touch them. Destroying the
//!   worker afterwards can then trip UCX's own assertions (for example the
//!   registration-cache `refcount == 0` check, SIGABRT). Drain your requests
//!   instead of relying on `Drop`.
//! * **Stable addresses only.** Buffers implement [`OwnedBuf`] /
//!   [`OwnedBufMut`]: heap storage (`Vec<u8>`, `Box<[u8]>`, `Box<[u8; N]>`)
//!   or `'static` data, whose address doesn't change when the request moves.
//! * **Flush scope.** A flush ([`ep::Ep::flush`], [`worker::Worker::flush`])
//!   covers RMA and AMO operations at both ends. It does not wait for an
//!   unmatched rendezvous tag send, which is only a request-to-send until a
//!   receiver matches it.
//!
//! Requests are `!Send` and `!Sync`: complete them on the thread that drives
//! the worker. With [`worker::MtWorker`], post, progress and complete inside
//! one `with_worker` closure (see its docs).
//!

#![allow(unused_imports)]

mod ffi;
#[cfg(test)]
mod tests;
mod threading_assert;
use crate::ffi::*;

pub mod am;
pub mod config;
pub mod context;
pub mod dt;
pub mod ep;
pub mod error_handler_mode;
pub mod listener;
pub mod memh;
pub mod memory_type;
pub mod rma;
pub mod stream;
pub mod tag;
pub mod thread_mode;
pub mod version;
pub mod worker;

pub mod completion;
pub mod request;
pub mod status;

pub use completion::{
    Completion, Failed, OwnedBuf, OwnedBufMut, OwningRequest, RecvInfo, RecvRequest,
    StreamRecvRequest, TagRecvRequest, DROP_PROGRESS_BUDGET,
};
pub use request::*;
pub use status::*;

// Re-export wrapped types
pub use error_handler_mode::ErrorHandlerMode;
pub use memh::MemAdvice;
pub use memory_type::MemoryType;
pub use thread_mode::ThreadMode;

use std::ffi::CString;
use std::ptr::NonNull;
