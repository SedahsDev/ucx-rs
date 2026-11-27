# ucx-sys (ucx-rs)

Low-level Rust bindings for [UCX](https://openucx.org/) (Unified Communication X), focused on the UCP API.

Package name: **`ucx-sys`**. Repository directory: `ucx-rs`.

## Features

- bindgen-generated FFI with offline `src/bindings.rs` fallback
- RAII wrappers: context, worker, endpoint, tag, RMA, AMO, active messages
- Two-tier status helpers (`status_to_result`, `status_ptr_to_result`)
- Builder patterns for params

## Build

```bash
export UCX_PREFIX=/path/to/ucx
export LD_LIBRARY_PATH=$UCX_PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
cargo build
cargo test
cargo run --example version_and_context
```

Also: `UCX_INCLUDE_DIR` + `UCX_LIB_DIR`. Fallbacks: `/usr`, `/usr/local`, `/opt/ucx`.

See [`../BUILDING.md`](../BUILDING.md).

## UCX version support

This crate requires UCX version 1.19 through 1.22 (inclusive). The build script enforces this range at compile time using `ucp_get_version()`.

Note: The default UCX package available via `apt` on many distributions is often version 1.16, which is below the supported minimum. To use this crate, install UCX 1.19+ from source or a repository that provides newer versions.

## Minimal example

```rust
use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::version;

fn main() {
    let (maj, min, rel) = version::get_version();
    println!("UCX {}.{}.{} ({})", maj, min, rel, version::get_version_string());

    let params = ParamsBuilder::new()
        .features(Flags::Tag | Flags::Rma)
        .mt_workers_shared(1)
        .build();
    let config = Config::read("", "").expect("ucp_config_read");
    let ctx = Context::new(&config, &params).expect("ucp_init");
    drop(ctx);
}
```
## Owned-data completion

Non-blocking operations (RMA, tag, stream, AM, AMOs, flush, close) take
ownership of their buffers and return `Completion::Immediate(data)` or
`Completion::Pending(request)`. A pending request borrows the worker and hands
the data back from `try_complete` once UCX is done, including after `cancel()`.
There is no single-request `wait`. Progress the worker and poll everything in
flight together:

```sh
UCX_TLS=self cargo run --example owned_pipeline
```

The rules (lifetimes, cancel, leak-on-timeout `Drop`, no progress from
callbacks, flush scope) are in the crate docs, "Owned-data model". Every
changed signature is listed under "Migration" in [`CHANGELOG.md`](./CHANGELOG.md).

## Notes

- RMA needs a transport that supports it (`TLS=tcp` often has no RMA).
- Multi-process tests typically need `prterun` / a DVM.
- See [`REVIEW.md`](./REVIEW.md) for API completeness and safety notes.

## License

BSD-style (see `LICENSE`).

## Stream API

`stream` module provides UCP stream send/recv/poll wrappers. Enable `UCP_FEATURE_STREAM`
in context features when using them. See `src/stream.rs`.
