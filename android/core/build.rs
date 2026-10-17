//! Builds whisper.cpp (vendor/whisper.cpp). Without the `llama` feature this also builds the ggml
//! core it runs on (CPU kernels only), straight from the vendored ggml sources; with `llama` the
//! ggml that llama-cpp-sys-2 compiles is shared, so the process carries exactly one. Bindings for
//! the small whisper C API surface we use are generated with bindgen.

use std::env;
use std::path::PathBuf;

/// ggml on its own (CPU backend), for whisper.cpp.
fn build_ggml(ggml_src: &PathBuf) {
    let target = env::var("TARGET").unwrap_or_default();
    let mut cfg = cmake::Config::new(ggml_src);
    cfg.define("GGML_BUILD_TESTS", "OFF")
        .define("GGML_BUILD_EXAMPLES", "OFF")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("GGML_OPENMP", "OFF")
        .define("GGML_LLAMAFILE", "OFF")
        .define("GGML_NATIVE", "OFF")
        .profile("Release");
    // GGML_* from the environment (for example GGML_CPU_ARM_ARCH=armv8.6-a+i8mm+dotprod+fp16).
    for (k, v) in env::vars() {
        if k.starts_with("GGML_") {
            println!("cargo:rerun-if-env-changed={k}");
            cfg.define(&k, &v);
        }
    }
    if target.contains("android") {
        let ndk = env::var("ANDROID_NDK_HOME").or_else(|_| env::var("CARGO_NDK_ANDROID_NDK")).expect("ANDROID_NDK_HOME");
        let api = env::var("ANDROID_API_LEVEL").unwrap_or_else(|_| "28".into());
        let abi = if target.contains("aarch64") { "arm64-v8a" } else if target.contains("x86_64") { "x86_64" } else { "armeabi-v7a" };
        cfg.define("CMAKE_TOOLCHAIN_FILE", format!("{ndk}/build/cmake/android.toolchain.cmake"))
            .define("ANDROID_PLATFORM", format!("android-{api}"))
            .define("ANDROID_ABI", abi)
            .define("ANDROID_STL", "c++_shared");
        match abi {
            "arm64-v8a" => {
                cfg.cflag("-march=armv8-a").cxxflag("-march=armv8-a");
            }
            "x86_64" => {
                // The emulator runs on a desktop CPU; let it use the vector units.
                cfg.cflag("-march=x86-64").cxxflag("-march=x86-64");
                for f in ["GGML_AVX2", "GGML_FMA", "GGML_F16C", "GGML_BMI2", "GGML_AVX"] {
                    cfg.define(f, "ON");
                }
            }
            _ => {}
        }
        println!("cargo:rustc-link-lib=log");
        println!("cargo:rustc-link-lib=android");
    }
    let dst = cfg.build();
    for d in ["lib", "lib64"] {
        println!("cargo:rustc-link-search=native={}", dst.join(d).display());
    }
    // Static libraries: dependents first.
    for l in ["ggml", "ggml-cpu", "ggml-base"] {
        println!("cargo:rustc-link-lib=static={l}");
    }
    println!("cargo:rerun-if-changed=../../vendor/llama-cpp-sys-2/llama.cpp/ggml");
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let whisper = manifest.join("../../vendor/whisper.cpp");
    let ggml_include = manifest.join("../../vendor/llama-cpp-sys-2/llama.cpp/ggml/include");

    println!("cargo:rerun-if-changed=build.rs");
    if !cfg!(feature = "llama") {
        build_ggml(&manifest.join("../../vendor/llama-cpp-sys-2/llama.cpp/ggml"));
    }
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
    if cfg!(feature = "vulkan") {
        build.define("GGML_USE_VULKAN", None);
    }
    build.compile("whisper");

    let target = env::var("TARGET").unwrap_or_default();
    let mut builder = bindgen::Builder::default();
    if target.contains("android") {
        // NDK r27+ headers refuse an unversioned target triple, and cargo-ndk's own
        // BINDGEN_EXTRA_CLANG_ARGS carries one, so drop those and pass ours.
        for (k, _) in env::vars() {
            if k.starts_with("BINDGEN_EXTRA_CLANG_ARGS") {
                env::remove_var(k);
            }
        }
        let api = env::var("ANDROID_API_LEVEL").unwrap_or_else(|_| "28".into());
        let ndk = env::var("ANDROID_NDK_HOME")
            .or_else(|_| env::var("CARGO_NDK_ANDROID_NDK"))
            .expect("ANDROID_NDK_HOME");
        let sysroot = format!("{ndk}/toolchains/llvm/prebuilt/linux-x86_64/sysroot");
        builder = builder
            .clang_arg(format!("--target={target}{api}"))
            .clang_arg(format!("--sysroot={sysroot}"));
    }
    let bindings = builder
        .header(whisper.join("include/whisper.h").to_string_lossy())
        .clang_arg(format!("-I{}", ggml_include.display()))
        .allowlist_function("whisper_.*")
        .allowlist_type("whisper_.*")
        .allowlist_var("WHISPER_.*")
        .allowlist_type("ggml_log_level")
        .allowlist_type("ggml_log_callback")
        .prepend_enum_name(false)
        .derive_default(false)
        .generate()
        .expect("generate whisper bindings");
    bindings
        .write_to_file(out.join("whisper_bindings.rs"))
        .expect("write whisper bindings");
}
