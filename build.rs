fn main() {
    if std::env::var("CARGO_FEATURE_LOCAL_EMBEDDINGS_TENSORRT").is_ok() {
        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
        let dll_src = std::env::var("SINORAG_TENSORRT_EP_DLL").unwrap_or_default();
        let dst = std::path::Path::new(&out_dir).join("ORTTensorRTEp.dll");

        if !dll_src.is_empty() {
            std::fs::copy(&dll_src, &dst).unwrap_or_else(|e| {
                panic!(
                    "Failed to copy TensorRT EP DLL from {:?} to {:?}: {}.\n\
                     Set SINORAG_TENSORRT_EP_DLL to the path of ORTTensorRTEp.dll.",
                    dll_src, dst, e
                )
            });
            println!("cargo:rerun-if-changed={}", dll_src);
        } else if !dst.exists() {
            // Create a zero-byte placeholder so include_bytes! compiles.
            // The real DLL is resolved at runtime via resolve_tensorrt_plugin_dll().
            std::fs::write(&dst, b"").expect("failed to write placeholder DLL");
        }

        println!("cargo:rerun-if-env-changed=SINORAG_TENSORRT_EP_DLL");
    }
}
