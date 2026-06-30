use std::env;
use std::path::PathBuf;

fn main() {
    let bindings_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    let src_bindings = PathBuf::from("src").join("bindings.rs");

    // Try to generate bindings from system UCX headers; fall back to pre-generated src/bindings.rs
    let has_headers = std::path::Path::new("/usr/include/ucp/api/ucp.h").exists()
        || std::path::Path::new("/usr/include/ucp.h").exists();

    if has_headers {
        let bindings = bindgen::Builder::default()
            .generate_comments(false)
            .rustified_enum(".*")
            .clang_arg("-I/usr/include/ucp/api/")
            .clang_arg("-I/usr/include/")
            .must_use_type("ucs_status_t")
            .must_use_type("ucs_status_ptr_t")
            .header("wrapper.h")
            .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
            .generate()
            .expect("Unable to generate bindings");

        bindings
            .write_to_file(&bindings_path)
            .expect("Couldn't write bindings!");

        // Update src/bindings.rs with fresh bindings
        std::fs::copy(&bindings_path, &src_bindings).expect("Failed to copy bindings to src/");
        println!("cargo:rerun-if-changed={}", src_bindings.display());
    } else {
        // No system headers — use pre-generated src/bindings.rs as offline fallback
        if src_bindings.exists() {
            println!(
                "cargo:warning=UCX system headers not found, using pre-generated offline bindings"
            );
            std::fs::copy(&src_bindings, &bindings_path)
                .expect("Failed to copy pre-generated bindings to OUT_DIR");
        } else {
            panic!(
                "UCX headers not found and no pre-generated src/bindings.rs. \
                Install libucx-dev or provide offline bindings."
            );
        }
    }

    // Link against system UCX libraries
    println!("cargo:rustc-link-lib=ucp");
    println!("cargo:rustc-link-lib=uct");
    println!("cargo:rustc-link-lib=ucm");
    println!("cargo:rustc-link-lib=ucs");

    println!("cargo:rerun-if-changed=wrapper.h");

    let src_path = PathBuf::from("src").join("bindings.rs");
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");

    // Try to generate bindings with bindgen; fall back to pre-generated src/bindings.rs
    let bindings_generated = bindgen::Builder::default()
        .generate_comments(false)
        .rustified_enum(".*")
        .clang_arg("-I/usr/include/ucp/api/")
        .clang_arg("-I/usr/include/")
        .must_use_type("ucs_status_t")
        .must_use_type("ucs_status_ptr_t")
        .header("wrapper.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate();

    match bindings_generated {
        Ok(bindings) => {
            println!("cargo:warning=bindgen succeeded — generating fresh UCX bindings");
            bindings
                .write_to_file(&out_path)
                .expect("Failed to write bindings to OUT_DIR");
            // Also update src/bindings.rs so offline builds work
            std::fs::copy(&out_path, &src_path).expect("Failed to copy bindings to src/");
        }
        Err(e) => {
            println!(
                "cargo:warning=bindgen failed ({}) — using pre-generated src/bindings.rs as fallback",
                e
            );
            if src_path.exists() {
                // Copy pre-generated bindings to OUT_DIR so compilation proceeds
                std::fs::copy(&src_path, &out_path)
                    .expect("Failed to copy fallback bindings to OUT_DIR");
            } else {
                panic!(
                    "bindgen failed and no pre-generated src/bindings.rs found.\n\
                     Please install libclang-dev or run bindgen manually to generate src/bindings.rs."
                );
            }
        }
    }

    println!("cargo:rerun-if-changed={}", src_path.display());
}
