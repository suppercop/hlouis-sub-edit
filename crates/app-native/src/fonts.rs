//! DirectWrite font availability and a synchronous libass [`SubtitleRenderer`].

use app_core::media::{FontProvider, SubtitleBBox, SubtitleRenderer};
use ass_core::{serialize_ass, ScriptFile};
use fonts::FontSpec;
use fonts_dwrite::DWriteFontProvider;
use render::AssSubRenderer;
use std::cell::RefCell;

/// [`FontProvider`] backed by the Windows DirectWrite system font collection.
pub struct NativeFontProvider {
    inner: DWriteFontProvider,
}

impl NativeFontProvider {
    /// Enumerates system + user fonts.
    pub fn new() -> Result<Self, String> {
        DWriteFontProvider::new()
            .map(|inner| Self { inner })
            .map_err(|e| format!("DirectWrite font enumeration failed: {e}"))
    }
}

impl FontProvider for NativeFontProvider {
    fn has_font(&self, family: &str) -> bool {
        fonts::FontProvider::has_font(&self.inner, &FontSpec::new(family, 400, false))
    }
    fn available_fonts(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Synchronous libass compositor (same engine the video worker uses), caching the parsed track
/// until the script text changes.
pub struct LibassRenderer {
    inner: RefCell<Option<(String, AssSubRenderer)>>,
}

impl Default for LibassRenderer {
    fn default() -> Self {
        Self {
            inner: RefCell::new(None),
        }
    }
}

impl SubtitleRenderer for LibassRenderer {
    fn render_over_frame(
        &self,
        script: &ScriptFile,
        time_ms: i64,
        width: u32,
        height: u32,
        frame_rgba: &mut [u8],
    ) -> Vec<SubtitleBBox> {
        let text = serialize_ass(script);
        let mut slot = self.inner.borrow_mut();
        let stale = slot.as_ref().is_none_or(|(t, _)| *t != text);
        if stale {
            let Ok(mut r) = AssSubRenderer::new(width, height) else {
                return Vec::new();
            };
            if r.load_ass_memory(&text).is_err() {
                return Vec::new();
            }
            *slot = Some((text, r));
        }
        if let Some((_, r)) = slot.as_mut() {
            let _ = r.render_and_blend_rgba(time_ms, frame_rgba, width, height);
        }
        Vec::new()
    }
}
