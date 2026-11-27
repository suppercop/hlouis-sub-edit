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
    println!(
        "cargo:rerun-if-changed={}",
        bs.join("videosource.cpp").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        bs.join("audiosource.cpp").display()
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set"));
    let patched_video = patch_videosource(&bs.join("videosource.cpp"), &out_dir);
    let patched_audio = patch_audiosource(&bs.join("audiosource.cpp"), &out_dir);

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
    for f in ["bsshared.cpp", "gpuhash.cpp", "tracklist.cpp"] {
        build.file(bs.join(f));
    }
    build.file(&patched_audio);
    build.file(&patched_video);
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

fn patch_once(src: &mut String, anchor: &str, replacement: &str, label: &str) {
    assert!(
        src.contains(anchor),
        "bestsource patch anchor missing for {label}: {anchor:?}"
    );
    *src = src.replacen(anchor, replacement, 1);
}

fn patch_videosource(src_path: &Path, out_dir: &Path) -> PathBuf {
    let mut text = std::fs::read_to_string(src_path)
        .expect("read videosource.cpp")
        .replace("\r\n", "\n");

    patch_once(
        &mut text,
        "#include \"version.h\"",
        "#include \"version.h\"\n\nnamespace bs_shim_hooks {\nvoid OnFrameIndexed(int64_t pts, bool keyframe) noexcept;\nbool ShouldAbortOnCacheMiss() noexcept;\n}",
        "videosource hooks decl",
    );
    patch_once(
        &mut text,
        "        const bool IndexFromCache = (CacheMode != bcmDisable) && ReadVideoTrackIndex(IsAbsolutePathCacheMode(CacheMode), CachePath);\n        if (!IndexFromCache) {",
        "        const bool IndexFromCache = (CacheMode != bcmDisable) && ReadVideoTrackIndex(IsAbsolutePathCacheMode(CacheMode), CachePath);\n        if (!IndexFromCache) {\n            if (bs_shim_hooks::ShouldAbortOnCacheMiss())\n                throw BestSourceException(\"cached video index unavailable or invalid\");",
        "videosource cache-only guard",
    );
    patch_once(
        &mut text,
        "        TrackIndex.Frames.push_back({ F->pts, F->repeat_pict, !!(F->flags & AV_FRAME_FLAG_KEY), !!(F->flags & AV_FRAME_FLAG_TOP_FIELD_FIRST), GetEffectivePixelFormat(F), VisibleWidth(F), VisibleHeight(F), HashDecodedFrame(Decoder.get(), F) });",
        "        TrackIndex.Frames.push_back({ F->pts, F->repeat_pict, !!(F->flags & AV_FRAME_FLAG_KEY), !!(F->flags & AV_FRAME_FLAG_TOP_FIELD_FIRST), GetEffectivePixelFormat(F), VisibleWidth(F), VisibleHeight(F), HashDecodedFrame(Decoder.get(), F) });\n        bs_shim_hooks::OnFrameIndexed(F->pts, !!(F->flags & AV_FRAME_FLAG_KEY));",
        "videosource progressive hook",
    );
    patch_once(
        &mut text,
        "        int DictSize = ReadInt(F);\n        if (DictSize > 0xFF)\n            return false;",
        "        int DictSize = ReadInt(F);\n        if (DictSize < 0 || DictSize > 0xFF)\n            return false;",
        "videosource dict size check",
    );
    patch_once(
        &mut text,
        "    for (const auto &FI : Index.Frames)\n        if (!av_pix_fmt_desc_get(static_cast<AVPixelFormat>(FI.Format)) || FI.Width <= 0 || FI.Height <= 0)\n            return false;",
        "    for (const auto &FI : Index.Frames)\n        if (!av_pix_fmt_desc_get(static_cast<AVPixelFormat>(FI.Format)) || FI.Width <= 0 || FI.Height <= 0 || FI.RepeatPict < 0)\n            return false;",
        "videosource repeat_pict check",
    );

    let out = out_dir.join("videosource.cpp");
    std::fs::write(&out, text).expect("write patched videosource.cpp");
    out
}

fn patch_audiosource(src_path: &Path, out_dir: &Path) -> PathBuf {
    let mut text = std::fs::read_to_string(src_path)
        .expect("read audiosource.cpp")
        .replace("\r\n", "\n");

    patch_once(
        &mut text,
        "#include \"version.h\"",
        "#include \"version.h\"\n\nnamespace bs_shim_hooks {\nbool ShouldAbortOnCacheMiss() noexcept;\n}",
        "audiosource hooks decl",
    );
    patch_once(
        &mut text,
        "    if (CacheMode == bcmDisable || !ReadAudioTrackIndex(IsAbsolutePathCacheMode(CacheMode), CachePath)) {",
        "    if (CacheMode == bcmDisable || !ReadAudioTrackIndex(IsAbsolutePathCacheMode(CacheMode), CachePath)) {\n        if (bs_shim_hooks::ShouldAbortOnCacheMiss())\n            throw BestSourceException(\"cached audio index unavailable or invalid\");",
        "audiosource cache-only guard",
    );
    patch_once(
        &mut text,
        "        int DictSize = ReadInt(F);\n        if (DictSize > 0xFF)\n            return false;",
        "        int DictSize = ReadInt(F);\n        if (DictSize < 0 || DictSize > 0xFF)\n            return false;",
        "audiosource dict size check",
    );

    let out = out_dir.join("audiosource.cpp");
    std::fs::write(&out, text).expect("write patched audiosource.cpp");
    out
}
