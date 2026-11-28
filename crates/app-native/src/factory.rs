//! Factory + file dialogs wiring the native providers into `app_core::EditorApp`.

use crate::audio::NativeAudio;
use crate::video::{AudioGate, NativeVideo};
use app_core::media::{AudioProvider, FileDialogs, MediaFactory, PickKind, VideoProvider};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Creates [`NativeVideo`] / [`NativeAudio`] providers.
#[derive(Default)]
pub struct NativeFactory {
    last_gate: Option<(PathBuf, Arc<AudioGate>)>,
}

impl NativeFactory {
    /// New factory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl MediaFactory for NativeFactory {
    fn open_video(
        &mut self,
        path: &Path,
        ctx: &egui::Context,
        ass_text: String,
    ) -> Result<Box<dyn VideoProvider>, String> {
        let gate = Arc::new(AudioGate::default());
        let video = NativeVideo::open(path, ctx, ass_text, Arc::clone(&gate))?;
        self.last_gate = Some((path.to_path_buf(), gate));
        Ok(Box::new(video))
    }

    fn open_audio(
        &mut self,
        path: &Path,
        ctx: &egui::Context,
    ) -> Result<Box<dyn AudioProvider>, String> {
        let gate = self
            .last_gate
            .as_ref()
            .filter(|(p, _)| p == path)
            .map(|(_, g)| Arc::clone(g));
        Ok(Box::new(NativeAudio::open(path, ctx, gate)?))
    }
}

/// Native Windows file dialogs via `rfd`.
#[derive(Debug, Default)]
pub struct NativeFileDialogs;

impl FileDialogs for NativeFileDialogs {
    fn pick_file(&mut self, kind: PickKind) -> Option<PathBuf> {
        let d = rfd::FileDialog::new();
        let d = match kind {
            PickKind::Subtitles => d
                .set_title("Open subtitles")
                .add_filter("Subtitles", &["ass", "ssa", "srt"]),
            PickKind::Video => d.set_title("Open video").add_filter(
                "Video",
                &[
                    "mkv", "mp4", "m4v", "avi", "mov", "webm", "ts", "m2ts", "wmv",
                ],
            ),
            PickKind::Audio => d.set_title("Open audio").add_filter(
                "Audio / video",
                &[
                    "wav", "flac", "mp3", "aac", "m4a", "ogg", "opus", "mka", "mkv", "mp4",
                ],
            ),
            PickKind::Mkv => d
                .set_title("Open subtitles from video")
                .add_filter("Matroska", &["mkv", "mks"]),
        };
        d.pick_file()
    }

    fn pick_save(&mut self, suggested_name: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Save subtitles")
            .set_file_name(suggested_name)
            .add_filter("Advanced SubStation Alpha", &["ass"])
            .save_file()
    }
}
