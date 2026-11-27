# BestSource handoff

State of the `bestsource` branch (rebased onto `main` after phase-1b) and where the remaining
owner requirements for the first-open experience should go.

## What exists

| Piece | Where | Notes |
|---|---|---|
| `bestsource-sys` | `crates/bestsource-sys/{build.rs,shim/bs_shim.{h,cpp},src/lib.rs}` | Builds pinned BestSource R23 + libp2p + xxHash with `cc` behind an `extern "C"` shim; links FFmpeg import libs only (LGPL DLLs stay dynamic). Sources are fetched and SHA-256 checked by `scripts/fetch-native.ps1` into `.native/src/`; `build.rs` never touches the network. |
| Exact provider | `crates/media/src/bestsource.rs` (`BestSourceProvider`, `BestSourceAudio`, `BestSourceOptions`, `IndexCache`) + `crates/media/src/ffi/bestsource.rs` (`RawBs*` wrappers, the only `unsafe` in `media` besides `ffi/win.rs`) | Behind the common `VideoProvider` / `AudioProvider` traits in `crates/media/src/provider.rs`. |
| Index-free quick decoder | shim `bs_quick_*` (uses BestSource's `LWVideoDecoder`, no index), Rust `crates/media/src/quick.rs` (`QuickVideo`, `ProviderKind::Quick`) | CFR frame numbers from timestamps, no keyframe list, `is_frame_exact() == false`. Pixels match the exact decode on the CFR test clip. |
| Hybrid open | `crates/media/src/provider.rs` (`HybridVideo`, `HybridOptions`, `FastOpen`, `IndexProgress`, `background_decode_threads`) | Quick (or FFMS2) serves frames at once; BestSource indexes on a below-normal-priority thread with `cores - 2` decoder threads; `poll_upgrade()` swaps providers; drop cancels. Progress + ETA via `index_progress()`. With a cached index the exact provider opens directly. |
| Persistent index cache | `crates/media/src/index_store.rs` (`IndexStore`) | `%LOCALAPPDATA%\hlouis-sub-edit\bsindex`, key = canonical path + size + mtime + FNV-1a of three 64 KiB samples (start/middle/end), 2 GiB cap, LRU by mtime (`touch` on use, `enforce_limit` after each index). |
| Win32 helpers | `crates/media/src/ffi/win.rs` | Thread priority, process CPU time (bench only). |
| App wiring | `crates/app/src/video_player.rs` (`handle_open_video`, `poll_exact_index`, `PlayerEvent::Exact*`), `crates/app/src/main.rs` (`ExactIndexUi`, `render_exact_index_status`) | Worker polls every 250 ms while indexing, shows a progress bar with ETA, re-emits the current frame after the switch, shows a message if the exact index fails (stays on quick path). |
| i18n | `crates/i18n/locales/{en,vi}/main.ftl` keys `status-exact-indexing*`, `status-exact-index-failed` | Both languages present. |
| Tests | `crates/media/tests/bestsource.rs` | Clips are generated with `ffmpeg.exe` (`.native/tools` or `HLOUIS_FFMPEG`); no media committed. Covers metadata, VFR timecodes, cache reuse, cancel, random seek vs sequential, FFMS2 vs BestSource agreement, hybrid switch (quick + FFMS2 fast paths), store reopen, quick-vs-exact pixels. |
| Bench / measurement | `crates/media/benches/*` (provider comparison), `crates/media/examples/first_open.rs` | `first_open` prints quick first frame, FFMS2 open, full index time and CPU %, cached exact open. |

## Measurements so far (release, this laptop: 16 logical cores, Iris Xe + MX550)

Owner's file: 1080p H.264 MKV, 24 min, 35 609 frames.

| Metric | Result | Budget / target |
|---|---|---|
| Cold open -> first frame (quick path) | 65 ms | < 500 ms |
| Approximate seek to middle (quick path) | 103 ms | — |
| Quick path sequential decode | 642 fps | — |
| FFMS2 open (index) + first frame | 1173 ms | — |
| BestSource full index, 14 threads, below normal | 40.8 s (CPU 50 % of 16 cores) | background |
| Cached reopen -> first exact frame | 66 ms | < 300 ms |
| Peak private memory during indexing | not measured yet | < 600 MB total |

Decode-only comparison with `ffmpeg.exe -t 300 -f null` (7 192 frames):

| Mode | Time |
|---|---|
| software, 14 threads | 8.6 s (~840 fps, same rate as BestSource indexing: indexing is decode-bound) |
| software, 1 thread | 38.8 s |
| D3D11VA with readback to system memory | 36.1 s |
| D3D11VA with readback, `-threads 4` | 19.7 s |
| D3D11VA, frames kept on GPU (no readback) | 7.6 s |
| Vulkan | not available (Intel driver lacks `VK_KHR_video_decode_queue`) |

## Remaining requirements: where to implement

1. **Instant first frame** — done by `FastOpen::Quick` (65 ms). Remaining: verify with the app
   end to end (cold process start + open) and add it to a phase report.
2. **Faster indexing / D3D11VA** — threads are already `background_decode_threads()` (cores - 2).
   D3D11VA: BestSource's `LWVideoDecoder::OpenFile` (`.native/src/bestsource/src/videosource.cpp`)
   only knows Vulkan (`GPU` flag) and, with it, GPU-side hashes that make the index usable only by
   a GPU decoder. A D3D11VA index pass must therefore (a) decode with `AV_PIX_FMT_D3D11`,
   (b) read back NV12 and hash it **exactly as `HashDecodedFrame` hashes the software
   `yuv420p` frame** (deinterleave UV, same plane/row order, same visible size) and record the
   software pixel format, then (c) compare the hashes of the first N frames against a software
   decode and fall back if they differ. Per the table above, readback makes D3D11VA *slower* than
   threaded software on this machine, so it is only worth it with pipelined/async readback or a
   GPU compute hash; measure before shipping. Implement as a build-time text patch of the pinned
   source (see pitfalls) plus a `BestSourceOptions::hw_index` flag.
3. **Video first, audio later** — in `HybridVideo::open`'s worker closure (`provider.rs`), after
   `BestSourceProvider::open_with_progress` succeeds, open `BestSourceAudio` with
   `store.cache_for(path)` so its index lands in the store; report it as a second progress phase.
   The app has no audio consumer yet, so expose the result via a new `UpgradeStatus`/accessor.
4. **Progressive exact seeking** — BestSource builds its index inside the `BestVideoSource`
   constructor (`IndexTrack`, `videosource.cpp` ~line 1520) and exposes nothing until it returns.
   Plan: patch `IndexTrack` to call a `thread_local` hook after each
   `TrackIndex.Frames.push_back(...)` with `(PTS, KeyFrame)`; the shim appends them to a
   mutex-guarded vector shared with `bs_quick`. Then `bs_quick_get_frame_rgba` for `n < indexed`
   seeks to the last indexed keyframe PTS <= target and decodes until `pts == Frames[n].PTS`
   (exact), and for `n >= indexed` extrapolates from the last indexed PTS (approximate). Because
   both use the same frame numbering, `poll_upgrade` can swap without a jump. Rust side:
   `QuickVideo` needs a shared `Arc` of the partial index and `is_frame_exact_at(n)`.
5. **Persistent LRU cache** — done (`IndexStore`). Remaining: user-configurable cap in settings.
6. **Idle pre-index of sibling files (setting, default off)** — new module in `crates/media`
   (e.g. `preindex.rs`) driven from `app` when the player is idle: list same-folder video files,
   skip `store.has_index`, index at `ThreadPriority::Idle` with few threads, cancel on any open.
   Needs a preferences entry + en/vi strings.
7. **Progress + ETA text** — done for the video pass; add a phase label once audio indexing (3)
   and pre-indexing (6) exist.

## Pitfalls

- `.native/src/bestsource` is the pinned R23 tarball; do not edit it in place. Patches belong in
  `crates/bestsource-sys` (e.g. `build.rs` copies the file to `OUT_DIR`, applies a string
  replacement and `assert!`s the anchor was found, so a source bump fails loudly).
- `Progress(Track, INT64_MAX, INT64_MAX)` is BestSource's "done" call; `HybridVideo` filters it.
- `LWVideoDecoder`'s own frame-count estimate ignores the time base; `bs_quick_open` recomputes it
  from the container header (Matroska has no per-stream duration).
- MSVC mangles access specifiers into member function names, so `#define private public` tricks
  in the shim do not link; use the patch approach.
- GPU (Vulkan) indexes must be read by a GPU decoder (hash domain), never mix them with CPU
  seeking.
- FFmpeg's frame/slice threads do not inherit the indexing thread's priority; that is why the
  thread count is capped at cores - 2.
- Shared checkout rule: never run git checkout/pull in the main `hlouis-sub-edit` folder while
  other agents run.

## Running

```powershell
$env:PATH = "$PWD\.native\bin;$env:PATH"      # FFmpeg/FFMS2/libass DLLs
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                         # add -j 4 on low RAM
cargo run --release -p media --example first_open -- "PATH\TO\episode.mkv" [--skip-auto]
cargo bench -p media
```

`HLOUIS_NATIVE_DIR` overrides the `.native` location; `HLOUIS_FFMPEG` points the tests at an
`ffmpeg.exe` for clip generation.
