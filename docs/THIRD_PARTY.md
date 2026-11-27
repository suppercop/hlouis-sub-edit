# Third-Party Native Dependencies

`hlouis-sub-edit` is licensed under the **BSD-3-Clause** license. All LGPL native dependencies are dynamically linked via Windows DLLs, and **no GPL code is linked** into any binary or bundled DLL.

All native libraries are fetched and built out-of-tree by [`scripts/fetch-native.ps1`](../scripts/fetch-native.ps1) with pinned versions/commits and SHA-256 verification.

## Native libraries (Windows x64)

| Library | Version / Pin | License | Linking | Bundled DLL(s) | Source |
|---|---|---|---|---|---|
| **libass** | `0.17.5` (vcpkg port `0.17.5#1`) | ISC | Dynamic (DLL) | `ass-9.dll` | vcpkg (`libass/libass`) |
| **FriBidi** | `1.0.18` | LGPL-2.1-or-later | Dynamic (DLL, dep of `libass`) | `fribidi-0.dll` | vcpkg (`fribidi/fribidi`) |
| **FreeType** | `2.15.0` | FreeType License (FTL) | Dynamic (DLL, dep of `libass`) | `freetype.dll` | vcpkg (`freetype`) |
| **HarfBuzz** | `15.1.0` (vcpkg port `15.1.0#1`) | MIT | Dynamic (DLL, dep of `libass`) | `harfbuzz.dll` | vcpkg (`harfbuzz/harfbuzz`) |
| **libpng** | `1.6.59` (vcpkg port `1.6.59#1`) | libpng License (zlib-style) | Dynamic (DLL, dep of `freetype`) | `libpng16.dll` | vcpkg (`pnggroup/libpng`) |
| **zlib** | `1.4` | zlib License | Dynamic (DLL, dep of `freetype`, `libpng`, `ffms2`) | `libz.dll` | vcpkg (`madler/zlib`) |
| **bzip2** | `1.0.8` (vcpkg port `1.0.8#6`) | bzip2-1.0.6 (BSD-style) | Dynamic (DLL, dep of `freetype`) | `bz2.dll` | vcpkg (`bzip2`) |
| **Brotli** | `1.2.1` | MIT | Dynamic (DLL, dep of `freetype`) | `brotlicommon.dll`, `brotlidec.dll` | vcpkg (`google/brotli`) |
| **FFmpeg** | `n9.0.2-982-g836df4e858` (`win64-lgpl-shared-9.0`) | LGPL-3.0 (no `--enable-gpl`, no `--enable-nonfree`) | Dynamic (DLL, dep of `ffms2`) | `avcodec-63.dll`, `avformat-63.dll`, `avutil-61.dll`, `swscale-10.dll`, `swresample-7.dll` | [`native-deps-v1`](https://github.com/suppercop/hlouis-sub-edit/releases/tag/native-deps-v1) prerelease mirror of `BtbN/FFmpeg-Builds` (`release/9.0`, SHA-256 verified) |
| **FFMS2** | `5.1.2` (commit `3af2ef2ae47bc30b64597c9e419e5b19c4bda7d8`) | MIT | Dynamic (DLL, compiled from source with MSVC) | `ffms2.dll` | `FFMS/ffms2` (SHA-256 verified) |

## Verification Notes

- `vcpkg` baseline commit: `9fa95bf687fa6ae7f2da0673a8067e5fc1515c27` (`vcpkg-configuration.json`).
- **FFmpeg version & mirror rationale:** Upstream FFmpeg tagged `n9.1`, while `BtbN/FFmpeg-Builds` publishes only `8.1` and `9.0` stable `win64-lgpl-shared` release builds (newest 9.x build tested: `n9.0.2-982-g836df4e858-win64-lgpl-shared-9.0.zip`), which compiles and passes all tests with pinned `FFMS2` (`3af2ef2ae4`). Because `BtbN/FFmpeg-Builds` prunes `autobuild-*` releases after ~10 days, the exact verified zip is hosted as an immutable asset on the [`native-deps-v1`](https://github.com/suppercop/hlouis-sub-edit/releases/tag/native-deps-v1) prerelease and checked against SHA-256 `F7BA27E7986A2D0835881E742A682DEC17896F6DC2CDCF9E596F043D2763D894`.
- Official prebuilt `ffms2-5.0-msvc.7z` releases from upstream FFMS2 statically link GPL FFmpeg and are **not** used. Instead, `fetch-native.ps1` builds `ffms2.dll` from upstream source against the LGPL-3.0 shared FFmpeg build above and `vcpkg` `zlib`.
- `libass` on Windows uses the system **DirectWrite** font provider (`ASS_FONTPROVIDER_AUTODETECT`), avoiding `fontconfig` and its transitive dependencies.
- Release artifacts bundle license notices in `licenses/` alongside `LICENSE` and this file, including the full **GPL-3.0** text (LGPL-3.0 is a set of additional permissions on top of GPL-3.0 and requires both texts), the **LGPL-3.0** text, and [`FFmpeg-SOURCE.txt`](../licenses/FFmpeg-SOURCE.txt) describing where to obtain the corresponding FFmpeg source. These files live in the repo's [`licenses/`](../licenses/) folder and are copied into the bundle by the CI "Stage release bundle" step.
- **Before any public release:** attach the exact FFmpeg source tarball for commit `836df4e858` (and the FriBidi `1.0.18` source) as assets on the `native-deps-v1` release, so the source is offered from the same place as the binaries.
