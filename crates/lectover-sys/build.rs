//! Builds ONE static ggml (from our llama.cpp fork's ggml/ subtree) plus our qwentts.cpp,
//! qwen3-asr.cpp and reuse.cpp forks against it, links them statically and generates the bindings. See
//! native/CMakeLists.txt for how the single ggml is arranged.

use std::env;
use std::path::{Path, PathBuf};

/// Static libraries in link order: dependents before their dependencies (GNU ld resolves
/// left to right). Entries are (library, requirement): a backend library is required when its
/// cargo feature is on and skipped otherwise; `ggml-blas` is always optional.
const LINK_ORDER: &[(&str, Need)] = &[
    ("reuse", Need::Always),
    ("qwen-core", Need::Always),
    ("qwen3-asr", Need::Always),
    ("ggml", Need::Always),
    ("ggml-metal", Need::Feature("metal")),
    ("ggml-cuda", Need::Feature("cuda")),
    ("ggml-hip", Need::Feature("hip")),
    ("ggml-vulkan", Need::Feature("vulkan")),
    ("ggml-blas", Need::Optional),
    ("ggml-cpu", Need::Always),
    ("ggml-base", Need::Always),
];

enum Need {
    Always,
    Feature(&'static str),
    Optional,
}

/// Must match `add_compile_definitions(GGML_MAX_NAME=...)` in native/CMakeLists.txt: it sizes
/// `ggml_tensor::name`, so the bindings' struct layout depends on it.
const GGML_MAX_NAME: u32 = 128;

/// Environment variables passed through to cmake as cache entries.
const PASSTHROUGH_ENV: &[&str] = &["GPU_TARGETS", "GGML_NATIVE", "CMAKE_CUDA_ARCHITECTURES"];

fn on(b: bool) -> &'static str {
    if b { "ON" } else { "OFF" }
}

fn feature(name: &str) -> bool {
    env::var_os(format!("CARGO_FEATURE_{}", name.to_uppercase())).is_some()
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let llama = manifest.join("vendor/llama.cpp");
    let qwentts = manifest.join("vendor/qwentts.cpp");
    let asr = manifest.join("vendor/qwen3-asr.cpp");
    let reuse = manifest.join("vendor/reuse.cpp");
    for (dir, marker, name) in [
        (&llama, "ggml/CMakeLists.txt", "llama.cpp"),
        (&qwentts, "CMakeLists.txt", "qwentts.cpp"),
        (&asr, "CMakeLists.txt", "qwen3-asr.cpp"),
        (&reuse, "CMakeLists.txt", "reuse.cpp"),
    ] {
        if !dir.join(marker).exists() {
            panic!(
                "{name} sources missing at {}: run `git submodule update --init` in the repository root",
                dir.display()
            );
        }
    }
    for p in ["native/CMakeLists.txt", "wrapper.h", "build.rs"] {
        println!("cargo:rerun-if-changed={p}");
    }
    for d in ["vendor/llama.cpp/ggml",
              "vendor/qwentts.cpp/CMakeLists.txt", "vendor/qwentts.cpp/src",
              "vendor/qwen3-asr.cpp/CMakeLists.txt", "vendor/qwen3-asr.cpp/src",
              "vendor/qwen3-asr.cpp/include",
              "vendor/reuse.cpp/CMakeLists.txt", "vendor/reuse.cpp/src", "vendor/reuse.cpp/include"] {
        println!("cargo:rerun-if-changed={d}");
    }
    for v in PASSTHROUGH_ENV.iter().chain(&["HIPCXX", "HIP_PATH", "CUDA_PATH", "VULKAN_SDK"]) {
        println!("cargo:rerun-if-env-changed={v}");
    }

    let (metal, cuda, hip, vulkan) = (feature("metal"), feature("cuda"), feature("hip"), feature("vulkan"));

    let mut cfg = cmake::Config::new(manifest.join("native"));
    cfg.define("LLAMA_DIR", &llama)
        .define("QWENTTS_DIR", &qwentts)
        .define("QWEN3_ASR_DIR", &asr)
        .define("REUSE_DIR", &reuse)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("CMAKE_INSTALL_LIBDIR", "lib")
        .define("GGML_BUILD_TESTS", "OFF")
        .define("GGML_BUILD_EXAMPLES", "OFF")
        // Library targets only: no CLIs, servers or test executables from the model libraries.
        .define("QWEN_BUILD_TOOLS", "OFF")
        .define("QWEN_SHARED", "OFF")
        .define("QWEN3_ASR_BUILD_TOOLS", "OFF")
        .define("QWEN3_ASR_TEST", "OFF")
        .define("QWEN3_ASR_SHARED", "OFF")
        .define("QWEN3_ASR_TIMING", "OFF")
        .define("REUSE_BUILD_TESTS", "OFF")
        .define("REUSE_BUILD_TOOLS", "OFF")
        // ggml's own thread pool; avoids linking libgomp/libomp into the product.
        .define("GGML_OPENMP", "OFF")
        .define("GGML_METAL", on(metal))
        .define("GGML_METAL_EMBED_LIBRARY", on(metal))
        .define("GGML_CUDA", on(cuda))
        .define("GGML_HIP", on(hip))
        .define("GGML_VULKAN", on(vulkan))
        .profile("Release");
    for v in PASSTHROUGH_ENV {
        if let Ok(val) = env::var(v) {
            cfg.define(v, val);
        }
    }
    let dst = cfg.build();

    // Spec D2: no model library may have configured its own ggml. A configured
    // subdirectory always gets a cmake_install.cmake; an empty directory is only the leftover
    // of a failed configure and is ignored.
    for lib in ["qwentts.cpp", "qwen3-asr.cpp", "reuse.cpp"] {
        let second_ggml = dst.join("build").join(lib).join("ggml");
        if second_ggml.join("cmake_install.cmake").exists() {
            panic!(
                "{lib} configured its own ggml at {} — the binary would carry two \
                 (if this is a stale build directory: `cargo clean -p lectover-sys`)",
                second_ggml.display()
            );
        }
    }

    link(&dst.join("lib"), metal, cuda, hip, vulkan);
    bindings(&manifest, &llama, &qwentts, &asr, &reuse);
}

fn link(lib: &Path, metal: bool, cuda: bool, hip: bool, vulkan: bool) {
    println!("cargo:rustc-link-search=native={}", lib.display());
    for (name, need) in LINK_ORDER {
        if lib.join(format!("lib{name}.a")).exists() {
            println!("cargo:rustc-link-lib=static={name}");
            continue;
        }
        match need {
            Need::Always => panic!("expected static library lib{name}.a in {}", lib.display()),
            Need::Feature(f) if feature(f) => panic!(
                "cargo feature `{f}` is on but the build produced no lib{name}.a in {}",
                lib.display()
            ),
            _ => {}
        }
    }
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os == "macos" {
        println!("cargo:rustc-link-lib=c++");
        for fw in ["Foundation", "Accelerate", "CoreAudio", "AudioToolbox"] {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
        if metal {
            for fw in ["Metal", "MetalKit"] {
                println!("cargo:rustc-link-lib=framework={fw}");
            }
        }
    } else {
        for l in ["stdc++", "m", "pthread", "dl"] {
            println!("cargo:rustc-link-lib={l}");
        }
    }
    if cuda {
        let root = env::var("CUDA_PATH").unwrap_or_else(|_| "/usr/local/cuda".into());
        for dir in [format!("{root}/lib64"), format!("{root}/lib"), format!("{root}/targets/x86_64-linux/lib")] {
            if Path::new(&dir).is_dir() {
                println!("cargo:rustc-link-search=native={dir}");
            }
        }
        for l in ["cudart", "cublas", "cublasLt", "cuda"] {
            println!("cargo:rustc-link-lib={l}");
        }
    }
    if hip {
        // HIP_PATH first, then `hipconfig -R`, then the default ROCm location.
        let root = env::var("HIP_PATH").ok().filter(|r| !r.is_empty()).or_else(|| {
            let out = std::process::Command::new("hipconfig").arg("-R").output().ok()?;
            let root = String::from_utf8(out.stdout).ok()?.trim().to_string();
            (out.status.success() && !root.is_empty()).then_some(root)
        }).unwrap_or_else(|| "/opt/rocm".into());
        let dir = format!("{root}/lib");
        if Path::new(&dir).is_dir() {
            println!("cargo:rustc-link-search=native={dir}");
        }
        for l in ["amdhip64", "hipblas", "rocblas"] {
            println!("cargo:rustc-link-lib={l}");
        }
    }
    if vulkan {
        println!("cargo:rustc-link-lib=vulkan");
    }
}

fn bindings(manifest: &Path, llama: &Path, qwentts: &Path, asr: &Path, reuse: &Path) {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindgen::Builder::default()
        .header(manifest.join("wrapper.h").to_string_lossy())
        .clang_arg(format!("-DGGML_MAX_NAME={GGML_MAX_NAME}"))
        .clang_arg(format!("-I{}", llama.join("ggml/include").display()))
        .clang_arg(format!("-I{}", qwentts.join("src").display()))
        .clang_arg(format!("-I{}", asr.join("include").display()))
        .clang_arg(format!("-I{}", reuse.join("include").display()))
        .allowlist_function("(ggml|gguf|qt|qwen3|reuse)_.*")
        .allowlist_type("(ggml|gguf|qt|qwen3|reuse)_.*")
        .allowlist_var("(GGML|GGUF|QT|REUSE)_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen failed on wrapper.h")
        .write_to_file(out.join("bindings.rs"))
        .expect("could not write bindings.rs");
}
