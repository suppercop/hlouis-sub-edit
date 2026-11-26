use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=HLOUIS_NATIVE_DIR");

    let native_dir = if let Ok(dir) = env::var("HLOUIS_NATIVE_DIR") {
        PathBuf::from(dir)
    } else {
        let manifest_dir =
            PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
        manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .expect("crate lives in workspace/crates/ffms2-sys")
            .join(".native")
    };

    let lib_dir = native_dir.join("lib");
    let lib_file = lib_dir.join("ffms2.lib");
    println!("cargo:rerun-if-changed={}", lib_file.display());
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=ffms2");
}
