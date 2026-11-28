//! Native Windows providers behind the `app-core` media traits: `BestSource`/`FFMS2` video with
//! an instant first frame and background index, libass subtitle compositing, WASAPI audio output
//! with waveform/spectrum from the real decoded audio, DirectWrite fonts and `rfd` file dialogs.

mod audio;
mod factory;
mod fonts;
mod video;
pub mod video_player;

pub use audio::NativeAudio;
pub use factory::{NativeFactory, NativeFileDialogs};
pub use fonts::{LibassRenderer, NativeFontProvider};
pub use video::{AudioGate, GateState, NativeVideo};

use app_core::EditorApp;
use ass_core::ScriptFile;
use std::time::Instant;

/// Monotonic wall clock for the playback controller.
pub struct InstantClock(pub Instant);

impl Default for InstantClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl playback::Clock for InstantClock {
    fn now_ns(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

/// Creates an [`EditorApp`] for `script` with every native provider plugged in. A provider that
/// cannot start (for example no DirectWrite) keeps the pure-Rust fallback and the reason is
/// returned so the caller can show it.
#[must_use]
pub fn native_editor(script: ScriptFile) -> (EditorApp, Vec<String>) {
    let mut app = EditorApp::new(script);
    let mut problems = Vec::new();
    app.factory = Some(Box::new(NativeFactory::new()));
    app.file_dialogs = Some(Box::new(NativeFileDialogs));
    app.renderer = Box::new(LibassRenderer::default());
    match NativeFontProvider::new() {
        Ok(f) => app.fonts = Box::new(f),
        Err(e) => problems.push(e),
    }
    // No synthetic media: the demo video/audio of `EditorApp::new` must not look like real media.
    app.shell.detach_video();
    app.shell.detach_audio();
    app.video = Box::new(app_core::media::DummyVideoProvider::new(2, 2, 1, 1));
    app.audio = None;
    (app, problems)
}
