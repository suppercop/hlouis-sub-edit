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
