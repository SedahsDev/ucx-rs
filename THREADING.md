# Threading Model & Send/Sync Inventory

**Date:** 2026-08-23
Scope: `ucx-sys` public wrappers (`src/*.rs`, excluding generated `bindings.rs`).
Companion to the crate-root docs in `src/lib.rs` (`//! # Threading model`).

This document follows the same structure as the pmix-rs THREADING.md so the two
HPC binding crates keep one shared vocabulary.

---

## 1. Strategy (one-liner)

| Layer | Policy |
|--------|--------|
| **Context** | UCX documents `ucp_context_h` as shareable across workers/threads → **`Send + Sync`** target. |
| **Worker** | One owner, one progress loop. **`!Send + !Sync`** today. `UCS_THREAD_MODE_MULTI` makes UCX *calls* concurrent-safe, not these Rust values transferable. |
| **Ep** | Bound to its worker's thread. **`!Send + !Sync`**. Drop before worker (enforced by the `worker_alive` guard only as a last-resort net). |
| **Request** | Tied to the worker that owns its completion. **`!Send + !Sync`**. Freeing a request handle is always safe (UCX RELEASED-flag defers cleanup); the *reply buffer* must outlive completion regardless. |
| **MemHandle / RemoteKey / MemHandleGuard** | Registered-memory handles. `!Send + !Sync` until an audited story exists (they carry raw C handles plus borrowed-buffer `PhantomData`). |
| **WorkerAddress / StreamData / MessageHandle** | Borrowed or UCX-owned data guards tied to worker/ep lifetime → **`!Send`** by construction (lifetimes). |
| **Callbacks (AM handler, listener accept/conn)** | Run on the **progress context** — whichever thread calls `Worker::progress()` (or UCX-internal progress under MULTI). No blocking calls in-handler; hop off if needed. |
| **Global FFI mutex** | Not provided. `UCS_THREAD_MODE_SERIALIZED` puts the serialization duty on the application, exactly as UCX does. |

**Anti-patterns (do not introduce):**

- `unsafe impl Send for Worker` without a per-object thread-mode check.
- `PhantomData<*mut u8>` on a type advertised as transferable (the pmix-rs #60 bug class).
- Holding the borrow of a fetch-AMO reply buffer across `into_inner()` while
  the request is still in flight.

---

## 2. What UCX itself guarantees

From `ucs/type/thread_mode.h` and `ucp/api/ucp.h`:

* `UCS_THREAD_MODE_SINGLE` — only the creating ("master") thread may access.
* `UCS_THREAD_MODE_SERIALIZED` — multiple threads may access, one at a time.
* `UCS_THREAD_MODE_MULTI` — concurrent access where UCX documents it safe.
  Per `ucp.h`, UCP guarantees thread safety for **context-level** calls and,
  under MULTI, for worker operations; `progress()` remains externally
  serialized per worker even under MULTI.
* `ucp_context_attr.mt_workers_shared` reports whether the context needs
  internal thread-safety support (workers on different threads).

Consequence for the Rust layer: **thread-mode selection is a property of the
worker at creation** (`Worker::ParamsBuilder::thread_mode`), but auto-trait
safety must be decided per *type*, conservatively, at compile time.

---

## 3. Current inventory (verified on master @ e63883f)

The crate currently declares **no** `unsafe impl Send/Sync` anywhere
(`grep -rn 'unsafe impl' src --exclude bindings.rs` is empty). Auto-traits are
therefore derived structurally:

| Type | Derivation | Intended |
|------|-----------|----------|
| `Config` | contains owned `CString`-backed config handle | audit |
| `Params` / builders (`*ParamsBuilder`) | POD + `Option<CString>` | `Send` (plain data) |
| `Context` | raw `ucp_context_h` → `!Send !Sync` | **should be `Send + Sync`** |
| `Worker` | raw `ucp_worker_h` → `!Send !Sync` | stays `!Send !Sync` (v1) |
| `Ep` | raw `ucp_ep_h` + `Arc<AtomicBool>` → `!Send !Sync` | stays `!Send !Sync` |
| `Request` | `Option<NonNull<c_void>>` → `!Send !Sync` | stays `!Send !Sync` |
| `FetchAmoRequest<'w,'a,T>` | `&Worker` + `PhantomData<&mut T>` → `!Send` | stays `!Send` (reply buffer pinning) |
| `RemoteKey` | raw rkey + alive flag → `!Send !Sync` | stays until audited |
| `MemHandle` | raw memh + context handle → `!Send !Sync` | candidate `Send` after audit |
| `MemHandleGuard<'a>` | `PhantomData<&'a mut [u8]>` → `!Send` | correct as-is |

Nothing pins these decisions today: a refactor that adds/removes a field can
silently flip any auto-trait. Closing that gap is the first issue below.

---

## 4. Progress & callback rules

| Rule | Why |
|------|-----|
| Never call `Worker::progress()` concurrently on one worker | UCX requires external serialization of progress per worker in every mode. |
| Do not block inside an AM recv handler or listener accept callback | Callbacks run in the progress context; blocking stalls all completions on that worker. |
| Hop heavy work off callbacks onto an application thread/channel | Same pattern as pmix-rs `threading::spawn_from_callback`. |
| Keep fetch-AMO reply buffers borrowed until `check_finished() == Ok(true)` | UCX writes the result at completion time, independent of when the request handle was freed. |
| Drop order: Eps → Requests → Listener → Worker → Context | `Ep::Drop`'s `worker_alive` guard is a diagnostic safety net, not a license to reorder. |

---

## 5. Roadmap (tracked issues)

1. Compile-time `Send`/`Sync` assertion matrix (`src/threading_assert.rs`,
   `static_assertions` dev-dep) locking the table in §3.
2. Make `Context` (and audited plain-data types) explicitly `Send + Sync`.
3. Document + enforce the per-worker progress-serialization contract in the
   API surface (doc lints; optionally a debug lock under a feature flag).
4. Callback-context documentation and a safe AM-handler trampoline with typed
   user context (hop-off helper parity with pmix-rs `threading.rs`).
5. Future (opt-in): an `MtWorker` wrapper providing `Send + Sync` access under
   `UCS_THREAD_MODE_MULTI`/`SERIALIZED` with an internal mutex — not provided
   by this crate today.
