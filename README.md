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

## Minimal example

```rust
use ucx_sys::context::{Config, Context, Flags, ParamsBuilder};
use ucx_sys::version;

fn main() {
    let (maj, min, rel) = version::get_version();
    println!("UCX {}.{}.{} ({})", maj, min, rel, version::get_version_string());

    let mut pb = ParamsBuilder::new();
    pb.features(Flags::Tag | Flags::Rma);
    let params = pb.build();
    let config = Config::default();
    let ctx = Context::new(&config, &params).expect("ucp_init");
    let _ = ctx; // drop cleans up
}
```

## Notes

- RMA needs a transport that supports it (`TLS=tcp` often has no RMA).
- Multi-process tests typically need `prterun` / a DVM.
- See [`REVIEW.md`](./REVIEW.md) for API completeness and safety notes.

## License

BSD-style (see `LICENSE`).


## Stream API

`stream` module provides UCP stream send/recv/poll wrappers. Enable `UCP_FEATURE_STREAM`
in context features when using them. See `src/stream.rs`.
