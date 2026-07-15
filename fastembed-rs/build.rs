use std::env;
use std::path::{Path, PathBuf};

fn main() {
    if env::var_os("CARGO_FEATURE_TENSORRT").is_none() {
        return;
    }

    let trt_libs = env::var("TENSORRT_LIBRARIES")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            find_lib_dir("libnvinfer.so")
                .expect("TENSORRT_LIBRARIES not set and libnvinfer.so not found")
        });
    let cuda_libs = env::var("CUDA_LIBRARIES")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            find_lib_dir("libcudart.so").expect("CUDA_LIBRARIES not set and libcudart.so not found")
        });
    let cuda_incl = env::var("CUDA_INCLUDE_DIRS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            find_include_dir("cuda_runtime.h")
                .expect("CUDA_INCLUDE_DIRS not set and cuda_runtime.h not found")
        });
    let trt_incl = env::var("TENSORRT_INCLUDE_DIRS")
        .map(PathBuf::from)
        .ok()
        .or_else(|| find_include_dir("NvInfer.h"));

    // Ambient include/library variables can silently mix TensorRT versions
    // (for example TensorRT 10 headers from /opt with TensorRT 11 libs from
    // /usr). Use the explicit paths above instead.
    for name in ["CPATH", "CPLUS_INCLUDE_PATH", "LIBRARY_PATH"] {
        println!("cargo:rerun-if-env-changed={name}");
        env::remove_var(name);
    }

    let mut builder = cxx_build::bridge("src/tensorrt.rs");
    builder
        .file("src/tensorrt_engine.cpp")
        .include(&cuda_incl)
        .flag_if_supported("-std=c++17")
        .flag("-O3")
        .flag("-Wall")
        .flag_if_supported("-Wno-deprecated-declarations")
        .flag_if_supported("-Wno-unused-parameter");

    if let Some(path) = &trt_incl {
        builder.include(path);
    }
    for path in ["/usr/include", "/usr/local/include"] {
        if Path::new(path).exists() {
            builder.include(path);
        }
    }

    builder.compile("fastembed-tensorrt");

    println!("cargo:rustc-link-search={}", trt_libs.display());
    println!("cargo:rustc-link-search={}", cuda_libs.display());
    println!("cargo:rustc-link-lib=cudart");
    println!("cargo:rustc-link-lib=nvinfer");
    println!("cargo:rustc-link-lib=nvinfer_plugin");

    println!("cargo:rerun-if-changed=src/tensorrt.rs");
    println!("cargo:rerun-if-changed=src/tensorrt_engine.cpp");
    println!("cargo:rerun-if-changed=src/tensorrt_engine.h");
    println!("cargo:rerun-if-env-changed=TENSORRT_INCLUDE_DIRS");
    println!("cargo:rerun-if-env-changed=TENSORRT_LIBRARIES");
    println!("cargo:rerun-if-env-changed=CUDA_INCLUDE_DIRS");
    println!("cargo:rerun-if-env-changed=CUDA_LIBRARIES");
}

fn find_lib_dir(name: &str) -> Option<PathBuf> {
    [
        "/usr/lib/x86_64-linux-gnu",
        "/usr/local/cuda/lib64",
        "/usr/local/cuda/lib",
        "/opt/tensorrt/lib",
        "/usr/lib",
    ]
    .iter()
    .map(Path::new)
    .find(|dir| dir.join(name).exists())
    .map(Path::to_path_buf)
}

fn find_include_dir(name: &str) -> Option<PathBuf> {
    [
        "/usr/include/x86_64-linux-gnu",
        "/usr/include",
        "/usr/local/cuda/include",
        "/opt/tensorrt/include",
        "/usr/local/include",
    ]
    .iter()
    .map(Path::new)
    .find(|dir| dir.join(name).exists())
    .map(Path::to_path_buf)
}
