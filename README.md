 ## UCX version support

-This crate is validated against UCX **1.22.0** (latest upstream as of 2026-08-02) and supports versions in the range **>= 1.19, <= 1.22**. The build script enforces this range at compile time using `ucp_get_version()`.
+This crate supports UCX versions in the range **>= 1.19**. The build script enforces this minimum version at compile time using `ucp_get_version()`.
