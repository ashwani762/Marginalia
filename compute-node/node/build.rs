//! Builds whisper.cpp (vendor/whisper.cpp) against the ggml that llama-cpp-sys-2 already compiles, so
//! the process carries exactly one ggml. Bindings for the small whisper C API surface we use are
//! generated with bindgen.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let whisper = manifest.join("../../vendor/whisper.cpp");
    let ggml_include = manifest.join("../vendor/llama-cpp-sys-2/llama.cpp/ggml/include");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../vendor/whisper.cpp/src/whisper.cpp");
    println!("cargo:rerun-if-changed=../../vendor/whisper.cpp/src/whisper-arch.h");
    println!("cargo:rerun-if-changed=../../vendor/whisper.cpp/include/whisper.h");
    println!("cargo:rerun-if-changed=../../vendor/whisper.cpp/src/aneforge");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .file(whisper.join("src/whisper.cpp"))
        .file(whisper.join("src/aneforge/whisper-aneforge.cpp"))
        .include(whisper.join("include"))
        .include(whisper.join("src"))
        .include(&ggml_include)
        .define("NDEBUG", None)
        .define("WHISPER_VERSION", "\"1.8.0\"")
        .opt_level(3)
        .flag_if_supported("-fPIC")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-function")
        .flag_if_supported("-Wno-deprecated-declarations")
        .warnings(false);
    if cfg!(feature = "cuda") {
        build.define("GGML_USE_CUDA", None);
    }
    if cfg!(feature = "vulkan") {
        build.define("GGML_USE_VULKAN", None);
    }
    build.compile("whisper");

    let bindings = bindgen::Builder::default()
        .header(whisper.join("include/whisper.h").to_string_lossy())
        .clang_arg(format!("-I{}", ggml_include.display()))
        .allowlist_function("whisper_.*")
        .allowlist_type("whisper_.*")
        .allowlist_var("WHISPER_.*")
        .allowlist_type("ggml_log_level")
        .allowlist_type("ggml_log_callback")
        .allowlist_type("ggml_abort_callback")
        .prepend_enum_name(false)
        .derive_default(false)
        .generate()
        .expect("generate whisper bindings");
    bindings.write_to_file(out.join("whisper_bindings.rs")).expect("write whisper bindings");
}
