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
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    let src_path = PathBuf::from("src").join("bindings.rs");
    println!("cargo:rerun-if-changed={}", src_path.display());

    if env::var("UCX_GENERATE_BINDINGS").as_deref() != Ok("1") {
        std::fs::copy(&src_path, &out_path)
            .unwrap_or_else(|e| panic!("Failed to copy pre-generated bindings.rs to OUT_DIR: {e}"));
        println!("cargo:warning=using pre-generated src/bindings.rs (set UCX_GENERATE_BINDINGS=1 to regenerate)");
        return;
    }

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

    match builder.generate() {
        Ok(bindings) => {
            println!("cargo:warning=bindgen succeeded — fresh bindings written to OUT_DIR only");
            bindings
                .write_to_file(&out_path)
                .expect("Failed to write bindings to OUT_DIR");
        }
        Err(e) => {
            println!("cargo:warning=bindgen failed ({e}) — using pre-generated src/bindings.rs as fallback");
            std::fs::copy(&src_path, &out_path)
                .expect("Failed to copy fallback bindings to OUT_DIR");
        }
    }

    // Validate UCX version at build time
    let version_check_code = r#"
#include <ucp/api/ucp.h>
#include <stdio.h>

int main() {
    unsigned version = ucp_get_version();
    int major = version / 10000;
    int minor = (version % 10000) / 100;
    int release = version % 100;
    printf("UCX_VERSION=%d.%d.%d\n", major, minor, release);
    return 0;
}
"#;

    let tmpdir = std::env::temp_dir();
    let src_file = tmpdir.join("ucx_version_check.c");
    std::fs::write(&src_file, version_check_code).expect("Failed to write version check C file");

    let bin_file = tmpdir.join("ucx_version_check");
    let status = std::process::Command::new("cc")
        .arg(&src_file)
        .arg("-o")
        .arg(&bin_file)
        .arg(format!("-I{}", include_dir.display()))
        .status()
        .expect("Failed to compile version check program");

    if !status.success() {
        println!("cargo:warning=Failed to compile UCX version check — skipping version validation");
        return;
    }

    let output = std::process::Command::new(&bin_file)
        .output()
        .expect("Failed to run version check program");

    let version_str = String::from_utf8_lossy(&output.stdout);
    let version_line = version_str
        .lines()
        .find(|l| l.starts_with("UCX_VERSION="))
        .unwrap_or("UCX_VERSION=0.0.0");

    let parts: Vec<&str> = version_line
        .trim_start_matches("UCX_VERSION=")
        .split('.')
        .collect();
    if parts.len() < 3 {
        println!("cargo:warning=Could not parse UCX version — skipping version validation");
        return;
    }

    let (major, minor): (u32, u32) = match (parts[0].parse(), parts[1].parse()) {
        (Ok(maj), Ok(min)) => (maj, min),
        _ => {
            println!("cargo:warning=Could not parse UCX version — skipping version validation");
            return;
        }
    };

    if major < MIN_MAJOR || (major == MIN_MAJOR && minor < MIN_MINOR) {
        panic!(
            "UCX version {}.{}, but minimum supported is {}.{}",
            major, minor, MIN_MAJOR, MIN_MINOR
        );
    }

    if major > MAX_MAJOR || (major == MAX_MAJOR && minor > MAX_MINOR) {
        panic!(
            "UCX version {}.{}, but maximum supported is {}.{}",
            major, minor, MAX_MAJOR, MAX_MINOR
        );
    }

    println!("cargo:warning=UCX version {}.{}.{} is within supported range ({}.{}, {}.{})",
             major, minor, parts[2], MIN_MAJOR, MIN_MINOR, MAX_MAJOR, MAX_MINOR);
}
