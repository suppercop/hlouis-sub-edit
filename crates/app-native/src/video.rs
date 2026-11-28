//! [`NativeVideo`]: asynchronous `VideoProvider` over the background video worker
//! (`BestSource` with instant fast-open, FFMS2 fallback, libass compositing).

use crate::video_player::{PlayerCmd, PlayerEvent, ReadyFrame, VideoPlayerHandle};
use app_core::media::{MediaEvent, VideoMeta, VideoProvider};
use app_core::transport::timecodes_for;
use keyframes::Timecodes;
use media::{IndexPhase, VideoInfo};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

/// Progress of the shared exact audio index so the audio decoder knows when it may open it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateState {
    Pending,
    Ready,
    Failed(String),
}

/// Hand-off between the video worker (which builds the audio index after the video index) and
/// the audio decoder of the same file.
#[derive(Debug, Default)]
pub struct AudioGate(Mutex<Option<GateState>>);

impl AudioGate {
    fn set(&self, s: GateState) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(s);
    }
    /// Current state (`Pending` until the worker reports).
    #[must_use]
    pub fn state(&self) -> GateState {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or(GateState::Pending)
    }
}

/// Frame source + libass compositor running on a background worker thread.
pub struct NativeVideo {
    handle: VideoPlayerHandle,
    gate: Arc<AudioGate>,
    width: u32,
    height: u32,
    frame_count: u32,
    timecodes: Timecodes,
    keyframes: Vec<u32>,
    latest: Option<ReadyFrame>,
    revision: u64,
    last_pct: i64,
    last_ass_error: Option<String>,
}

fn meta_from(info: &VideoInfo) -> Option<VideoMeta> {
    let tc = timecodes_for(
        &info.frame_pts_ms,
        u64::try_from(info.fps_num).unwrap_or(24_000),
        u64::try_from(info.fps_den).unwrap_or(1_001),
    )?;
    let keyframes: Vec<u32> = info
        .keyframes
        .iter()
        .filter_map(|&k| u32::try_from(k).ok())
        .collect();
    let frame_count = u32::try_from(info.num_frames).ok()?;
    Some(VideoMeta {
        width: info.width,
        height: info.height,
        frame_count,
        timecodes: tc,
        keyframes,
        sar: info.sar(),
    })
}

impl NativeVideo {
    /// Starts opening `path`; returns immediately.
    pub fn open(
        path: &Path,
        ctx: &egui::Context,
        ass_text: String,
        gate: Arc<AudioGate>,
    ) -> Result<Self, String> {
        let handle = VideoPlayerHandle::spawn(ass_text, false, 2_048, ctx.clone())
            .map_err(|e| e.to_string())?;
        handle.open_video(path.to_path_buf())?;
        let timecodes =
            Timecodes::from_cfr(24_000, 1_001).map_err(|e| format!("timecodes: {e}"))?;
        Ok(Self {
            handle,
            gate,
            width: 0,
            height: 0,
            frame_count: 0,
            timecodes,
            keyframes: Vec::new(),
            latest: None,
            revision: 0,
            last_pct: -100,
            last_ass_error: None,
        })
    }

    fn adopt(&mut self, info: &VideoInfo) -> Option<VideoMeta> {
        let meta = meta_from(info)?;
        self.width = meta.width;
        self.height = meta.height;
        self.frame_count = meta.frame_count;
        self.timecodes = meta.timecodes.clone();
        self.keyframes.clone_from(&meta.keyframes);
        Some(meta)
    }
}

impl VideoProvider for NativeVideo {
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn frame_count(&self) -> u32 {
        self.frame_count
    }
    fn timecodes(&self) -> &Timecodes {
        &self.timecodes
    }
    fn keyframes(&self) -> &[u32] {
        &self.keyframes
    }
    fn frame_rgba(&self, _frame: u32) -> Option<&[u8]> {
        self.latest.as_ref().map(|f| f.rgba.as_slice())
    }
    fn revision(&self, _frame: u32) -> u64 {
        self.revision
    }
    fn request_frame(&mut self, frame: u32) {
        let _ = self.handle.send(PlayerCmd::SeekFrame(frame as usize));
    }
    fn set_subtitles(&mut self, ass_text: String) {
        let _ = self.handle.send(PlayerCmd::LoadAssMemory(ass_text));
    }
    fn poll(&mut self, out: &mut Vec<MediaEvent>) {
        for evt in self.handle.drain_events() {
            match evt {
                PlayerEvent::IndexingProgress { current, total } => {
                    if total > 0 {
                        let pct = current * 100 / total;
                        if pct - self.last_pct >= 5 {
                            self.last_pct = pct;
                            out.push(MediaEvent::Status(format!("Indexing video {pct}%")));
                        }
                    }
                }
                PlayerEvent::IndexingCancelled => {
                    out.push(MediaEvent::Status("Video open cancelled".to_owned()));
                }
                PlayerEvent::VideoReady { info, file_name } => {
                    if let Some(meta) = self.adopt(&info) {
                        out.push(MediaEvent::Status(format!("Opened {file_name}")));
                        out.push(MediaEvent::VideoReady(meta));
                    } else {
                        out.push(MediaEvent::Error(format!(
                            "{file_name}: the video reports invalid timing information"
                        )));
                    }
                }
                PlayerEvent::ExactReady { info } => {
                    if let Some(meta) = self.adopt(&info) {
                        out.push(MediaEvent::VideoRefined(meta));
                    }
                }
                PlayerEvent::ExactIndexing {
                    phase,
                    fraction,
                    eta_secs,
                } => {
                    #[allow(clippy::cast_possible_truncation)]
                    let pct = (fraction * 100.0) as i64;
                    if pct - self.last_pct >= 5 || pct < self.last_pct {
                        self.last_pct = pct;
                        let what = match phase {
                            IndexPhase::Video => "video",
                            IndexPhase::Audio => "audio",
                        };
                        let eta = eta_secs.map_or(String::new(), |s| format!(", ETA {s} s"));
                        out.push(MediaEvent::Status(format!(
                            "Frame-exact {what} index {pct}%{eta}"
                        )));
                    }
                }
                PlayerEvent::ExactAudioReady => self.gate.set(GateState::Ready),
                PlayerEvent::ExactAudioFailed(msg) => {
                    self.gate.set(GateState::Failed(msg.clone()));
                    out.push(MediaEvent::Error(format!("Audio index failed: {msg}")));
                }
                PlayerEvent::ExactIndexFailed(msg) => {
                    self.gate.set(GateState::Failed(msg.clone()));
                    out.push(MediaEvent::Error(format!(
                        "Frame-exact index failed ({msg}); seeking stays approximate"
                    )));
                }
                PlayerEvent::ProviderNotice(msg) => out.push(MediaEvent::Status(msg)),
                PlayerEvent::PreindexStatus(_) | PlayerEvent::PlaybackEnded => {}
                PlayerEvent::Frame(frame) => {
                    self.handle.release_frame_slots(1);
                    if let Some(old) = self.latest.replace(frame) {
                        self.handle.recycle_buffer(old.rgba);
                    }
                    self.revision += 1;
                }
                PlayerEvent::AssError(e) => {
                    if self.last_ass_error.as_deref() != Some(e.as_str()) {
                        out.push(MediaEvent::Error(format!("Subtitle rendering: {e}")));
                        self.last_ass_error = Some(e);
                    }
                }
                PlayerEvent::Error(e) => out.push(MediaEvent::Error(e)),
            }
        }
    }
}
