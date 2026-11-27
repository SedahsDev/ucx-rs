use std::env;
use std::path::{Path, PathBuf};

/// Minimum supported UCX version (major, minor)
const MIN_MAJOR: u32 = 1;
const MIN_MINOR: u32 = 19;

/// Maximum supported UCX version (major, minor) — inclusive upper bound
const MAX_MAJOR: u32 = 1;
const MAX_MINOR: u32 = 22;

/// Discover UCX include/lib dirs.
/// Order: UCX_PREFIX → UCX_INCLUDE_DIR/UCX_LIB_DIR → common prefixes → /usr
fn discover_ucx() -> (PathBuf, PathBuf) {
    println!("cargo:rerun-if-env-changed=UCX_PREFIX");
    println!("cargo:rerun-if-env-changed=UCX_INCLUDE_DIR");
    println!("cargo:rerun-if-env-changed=UCX_LIB_DIR");

    if let Ok(prefix) = env::var("UCX_PREFIX") {
        let prefix = PathBuf::from(prefix);
        return (prefix.join("include"), prefix.join("lib"));
    }

    let include = env::var("UCX_INCLUDE_DIR").ok().map(PathBuf::from);
    let lib = env::var("UCX_LIB_DIR").ok().map(PathBuf::from);
    if let (Some(inc), Some(lib)) = (include, lib) {
        return (inc, lib);
    }

    let candidates = ["/usr", "/usr/local", "/opt/ucx"];
    for c in candidates {
        let p = Path::new(c);
        let inc = p.join("include");
        let lib = p.join("lib");
        if inc.join("ucp").join("api").join("ucp.h").exists()
            || inc.join("ucp.h").exists()
            || lib.join("libucp.so").exists()
        {
            return (inc, lib);
        }
    }

    (PathBuf::from("/usr/include"), PathBuf::from("/usr/lib"))
}

fn main() {
    let (include_dir, lib_dir) = discover_ucx();

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    println!("cargo:rustc-link-lib=ucp");
    println!("cargo:rustc-link-lib=uct");
    println!("cargo:rustc-link-lib=ucm");
    println!("cargo:rustc-link-lib=ucs");
    println!("cargo:rerun-if-changed=wrapper.h");

    // The committed src/bindings.rs is the source of truth. Bindgen is opt-in
    // via UCX_GENERATE_BINDINGS=1 and writes ONLY to OUT_DIR — it must never
    // overwrite files in src/ (issue #1).
    //
    // With UCX_GENERATE_BINDINGS=1 the build is strict: the headers must be a
    // supported UCX version and bindgen must succeed, or the build fails. There
    // is no silent fallback to the committed bindings, so CI (#132) really
    // tests the bindgen path and the version guard.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    let src_path = PathBuf::from("src").join("bindings.rs");
    println!("cargo:rerun-if-changed={}", src_path.display());

    println!("cargo:rerun-if-env-changed=UCX_GENERATE_BINDINGS");
    if env::var("UCX_GENERATE_BINDINGS").as_deref() != Ok("1") {
        std::fs::copy(&src_path, &out_path)
            .unwrap_or_else(|e| panic!("Failed to copy pre-generated bindings.rs to OUT_DIR: {e}"));
        println!("cargo:warning=using pre-generated src/bindings.rs (set UCX_GENERATE_BINDINGS=1 to regenerate)");
        return;
    }

    // UCX_GENERATE_BINDINGS=1: check the version of the headers bindgen is
    // about to read before running it, so an unsupported UCX fails with a clear
    // message rather than a bindgen or compile error.
    check_ucx_version(&include_dir);

    // Prefer UCP API include layout when present
    let mut clang_args = vec![format!("-I{}", include_dir.display())];
    let ucp_api = include_dir.join("ucp").join("api");
    if ucp_api.exists() {
        clang_args.push(format!("-I{}", ucp_api.display()));
    }

    let mut builder = bindgen::Builder::default()
        .generate_comments(false)
        .rustified_enum(".*")
        .must_use_type("ucs_status_t")
        .must_use_type("ucs_status_ptr_t")
        // Keep glibc stdio internals from producing bogus size asserts
        // (_IO_FILE layout differs between libcs); treat it as opaque.
        .opaque_type("_IO_FILE")
        .header("wrapper.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));

    for arg in &clang_args {
        builder = builder.clang_arg(arg);
    }

    let bindings = builder.generate().unwrap_or_else(|e| {
        panic!(
            "UCX_GENERATE_BINDINGS=1 but bindgen failed against {}: {e}",
            include_dir.display()
        )
    });
    bindings
        .write_to_file(&out_path)
        .expect("Failed to write bindings to OUT_DIR");
    println!("cargo:warning=bindgen succeeded — fresh bindings written to OUT_DIR only");
}

/// Read `UCP_API_MAJOR` / `UCP_API_MINOR` from `ucp_version.h` under
/// `include_dir` (either `ucp/api/ucp_version.h` or a flat layout).
///
/// These are the versions of the headers bindgen reads. The header carries no
/// release number. Nothing has to be compiled or linked, so the check also
/// works when the UCX libraries are not on the loader path at build time.
fn header_version(include_dir: &Path) -> Result<(u32, u32, PathBuf), String> {
    let candidates = [
        include_dir.join("ucp").join("api").join("ucp_version.h"),
        include_dir.join("ucp_version.h"),
    ];
    let Some(header) = candidates.iter().find(|p| p.exists()) else {
        return Err(format!(
            "no ucp_version.h under {} (looked for ucp/api/ucp_version.h and ucp_version.h)",
            include_dir.display()
        ));
    };
    println!("cargo:rerun-if-changed={}", header.display());
    let text = std::fs::read_to_string(header)
        .map_err(|e| format!("cannot read {}: {e}", header.display()))?;
    let define = |name: &str| -> Option<u32> {
        text.lines().find_map(|line| {
            let mut words = line.split_whitespace();
            match (words.next(), words.next(), words.next()) {
                (Some("#define"), Some(n), Some(value)) if n == name => value.parse().ok(),
                _ => None,
            }
        })
    };
    match (define("UCP_API_MAJOR"), define("UCP_API_MINOR")) {
        (Some(major), Some(minor)) => Ok((major, minor, header.clone())),
        _ => Err(format!(
            "{} does not define numeric UCP_API_MAJOR and UCP_API_MINOR",
            header.display()
        )),
    }
}

/// The UCX version guard: panics unless the headers under `include_dir` are a
/// supported UCX version. Runs only with `UCX_GENERATE_BINDINGS=1`.
fn check_ucx_version(include_dir: &Path) {
    let (major, minor, header) = header_version(include_dir).unwrap_or_else(|e| {
        panic!("UCX version guard: {e}");
    });
    if (major, minor) < (MIN_MAJOR, MIN_MINOR) {
        panic!(
            "UCX version {}.{} ({}), but minimum supported is {}.{}",
            major,
            minor,
            header.display(),
            MIN_MAJOR,
            MIN_MINOR
        );
    }
    if (major, minor) > (MAX_MAJOR, MAX_MINOR) {
        panic!(
            "UCX version {}.{} ({}), but maximum supported is {}.{}",
            major,
            minor,
            header.display(),
            MAX_MAJOR,
            MAX_MINOR
        );
    }
    println!(
        "cargo:warning=UCX version {}.{} ({}) is within supported range ({}.{}, {}.{})",
        major,
        minor,
        header.display(),
        MIN_MAJOR,
        MIN_MINOR,
        MAX_MAJOR,
        MAX_MINOR
    );
}
