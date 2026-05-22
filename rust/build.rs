use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor = manifest_dir.join("vendor").join("libmpv");

    println!("cargo:rustc-link-search=native={}", vendor.display());
    println!("cargo:rustc-link-lib=dylib=mpv");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor/libmpv/libmpv.dll.a");
    println!("cargo:rerun-if-changed=vendor/libmpv/libmpv-2.dll");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let target_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has no ancestor at depth 3")
        .to_path_buf();
    let dll_src = vendor.join("libmpv-2.dll");
    let dll_dst = target_dir.join("libmpv-2.dll");
    if dll_src.exists() {
        if let Err(e) = std::fs::copy(&dll_src, &dll_dst) {
            eprintln!("warning: failed to copy libmpv-2.dll to target: {e}");
        }
    }

    if cfg!(target_os = "windows") {
        let rc = manifest_dir.join("app.rc");
        if rc.exists() {
            embed_resource::compile(&rc, embed_resource::NONE);
        }
    }
}
