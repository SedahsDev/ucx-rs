 ## Unreleased

+- **Docs:** Updated README to state supported UCX range as **>= 1.19**, consistent with `build.rs`.
 - Added opt-in, thread-mode-checked `worker::MtWorker` access.
 - **Breaking (#73):** `ucs_status_t` no longer appears in the public API.
   - `Status` is `#[repr(transparent)]` and has associated constants for the UCX status codes
