//! Compiles `BestSource` (MIT) + `libp2p` (WTFPL) + the `extern "C"` shim with `cc` and links
//! the result statically. `FFmpeg` stays an LGPL DLL: only its import libraries are linked.
//!
//! The pinned sources are fetched and SHA-256 verified by `scripts/fetch-native.ps1` into
//! `.native/src/{bestsource,libp2p,xxhash}`; nothing here touches the network.

use std::env;
use std::path::{Path, PathBuf};

fn native_dir() -> PathBuf {
    if let Ok(dir) = env::var("HLOUIS_NATIVE_DIR") {
        return PathBuf::from(dir);
    }
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("crate lives in workspace/crates/bestsource-sys")
        .join(".native")
}

fn require(path: &Path) {
    assert!(
        path.exists(),
        "{} is missing; run scripts/fetch-native.ps1 first",
        path.display()
    );
}

fn main() {
    println!("cargo:rerun-if-env-changed=HLOUIS_NATIVE_DIR");
    println!("cargo:rerun-if-changed=shim/bs_shim.cpp");
    println!("cargo:rerun-if-changed=shim/bs_shim.h");

    let native = native_dir();
    let src = native.join("src");
    let bs = src.join("bestsource").join("src");
    let p2p = src.join("libp2p");
    let xxh = src.join("xxhash");
    let include = native.join("include");
    let lib = native.join("lib");
    for p in [
        &bs.join("videosource.cpp"),
        &p2p.join("p2p_api.cpp"),
        &xxh.join("xxhash.h"),
        &include.join("libavcodec").join("avcodec.h"),
        &lib.join("avcodec.lib"),
    ] {
        require(p);
    }
    for stamp in ["bestsource.stamp", "libp2p.stamp", "xxhash.stamp"] {
        println!("cargo:rerun-if-changed={}", src.join(stamp).display());
    }

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .flag_if_supported("/W0")
        .flag("/EHsc")
        .flag("/utf-8")
        .flag("/permissive-")
        .define("_CRT_SECURE_NO_WARNINGS", None)
        .define("NOMINMAX", None)
        .define("P2P_SIMD", None)
        .define("XXH_INLINE_ALL", None)
        .define("__STDC_CONSTANT_MACROS", None)
        .include(&bs)
        .include(&p2p)
        .include(&xxh)
        .include(&include)
        .include("shim");
    for f in [
        "audiosource.cpp",
        "bsshared.cpp",
        "gpuhash.cpp",
        "tracklist.cpp",
        "videosource.cpp",
    ] {
        build.file(bs.join(f));
    }
    for f in [
        "p2p_api.cpp",
        "v210.cpp",
        "simd/cpuinfo_x86.cpp",
        "simd/p2p_simd.cpp",
        "simd/p2p_sse41.cpp",
    ] {
        build.file(p2p.join(f));
    }
    build.file("shim/bs_shim.cpp");
    build.compile("bestsource_shim");

    println!("cargo:rustc-link-search=native={}", lib.display());
    for l in ["avformat", "avcodec", "avutil", "swscale"] {
        println!("cargo:rustc-link-lib=dylib={l}");
    }
    for l in ["shell32", "ole32", "uuid"] {
        println!("cargo:rustc-link-lib=dylib={l}");
    }
}
