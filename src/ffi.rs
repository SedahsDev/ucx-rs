#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
// `unnecessary_transmutes` only exists in newer compilers; older ones (MSRV) warn about the
// unknown lint name, so allow that too.
#![allow(unknown_lints)]
#![allow(unnecessary_transmutes)]
#![allow(clippy::upper_case_acronyms)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
