//! DirectWrite implementation of [`fonts::FontProvider`] (Windows).
//!
//! Fonts come from the system font set (which includes per-user installed fonts on Windows 10
//! 1809+), every font file in `%LOCALAPPDATA%\Microsoft\Windows\Fonts` and any extra folders
//! (e.g. the script's attached-fonts folder). A family is looked up first as a DirectWrite
//! (weight/stretch/style) family; names DirectWrite does not group that way (GDI family names
//! such as `Segoe UI Semibold`, full names, PostScript names — what `VSFilter` / libass accept)
//! are looked up in the font set's name properties and turned into a small collection. Either
//! way the face is chosen by `IDWriteFontFamily::GetFirstMatchingFont` (normal stretch, the
//! requested weight, italic or normal style).
//!
//! Glyph coverage is read once per font file from its `cmap`
//! (`IDWriteFontFace1::GetUnicodeRanges`) and cached for the lifetime of the process as a BMP
//! bitmap plus ranges, so repeated checks are a bit test per glyph.
//!
//! The provider holds COM pointers and is therefore `!Send`: create it on the worker thread that
//! runs the font check (construction takes a few milliseconds; coverage data is shared
//! process-wide).

#![deny(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

#[allow(unsafe_code)]
mod ffi;

pub use ffi::{FaceAttrs, Slant};

use fonts::{FontProvider, FontSpec};
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use thiserror::Error;

/// Errors creating a [`DWriteFontProvider`].
#[derive(Debug, Error)]
pub enum FontsDwriteError {
    /// A DirectWrite call failed.
    #[error("DirectWrite error: {0}")]
    DirectWrite(#[from] windows::core::Error),
}

/// Where fonts are loaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontSources {
    /// Include the system font set.
    pub system: bool,
    /// Include every font file in `%LOCALAPPDATA%\Microsoft\Windows\Fonts`.
    pub user_fonts: bool,
    /// Extra folders whose font files (`.ttf`, `.otf`, `.ttc`, `.otc`) are loaded (not recursive).
    pub dirs: Vec<PathBuf>,
}

impl Default for FontSources {
    fn default() -> Self {
        Self {
            system: true,
            user_fonts: true,
            dirs: Vec::new(),
        }
    }
}

/// The face a [`FontSpec`] resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedFace {
    pub attrs: FaceAttrs,
    /// Font file path and face index, when the font is a local file.
    pub file: Option<(PathBuf, u32)>,
}

/// Glyph coverage of one font: a bitmap for the BMP plus sorted inclusive ranges above it.
#[derive(Debug)]
struct Coverage {
    bmp: Box<[u64; 1024]>,
    astral: Vec<(u32, u32)>,
}

impl Coverage {
    fn from_ranges(ranges: &[(u32, u32)]) -> Self {
        let mut bmp = Box::new([0u64; 1024]);
        let mut astral = Vec::new();
        for &(first, last) in ranges {
            for c in first..=last.min(0xFFFF) {
                bmp[(c >> 6) as usize] |= 1 << (c & 63);
            }
            if last > 0xFFFF {
                astral.push((first.max(0x1_0000), last));
            }
        }
        astral.sort_unstable();
        Self { bmp, astral }
    }

    fn contains(&self, ch: char) -> bool {
        let c = u32::from(ch);
        if c <= 0xFFFF {
            return self.bmp[(c >> 6) as usize] & (1 << (c & 63)) != 0;
        }
        let i = self.astral.partition_point(|&(first, _)| first <= c);
        i > 0 && self.astral[i - 1].1 >= c
    }
}

type CoverageKey = (String, u32);

/// Process-wide cmap coverage cache keyed by (lower-cased file path, face index).
fn coverage_cache() -> &'static Mutex<HashMap<CoverageKey, Arc<Coverage>>> {
    static CACHE: OnceLock<Mutex<HashMap<CoverageKey, Arc<Coverage>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn coverage_for(font: &ffi::Font, key: Option<&CoverageKey>) -> Option<Arc<Coverage>> {
    if let Some(key) = key {
        if let Ok(cache) = coverage_cache().lock() {
            if let Some(c) = cache.get(key) {
                return Some(Arc::clone(c));
            }
        }
    }
    let cov = Arc::new(Coverage::from_ranges(&font.unicode_ranges()?));
    if let Some(key) = key {
        if let Ok(mut cache) = coverage_cache().lock() {
            cache.insert(key.clone(), Arc::clone(&cov));
        }
    }
    Some(cov)
}

struct Resolved {
    font: ffi::Font,
    attrs: FaceAttrs,
    /// Original-case file path and face index.
    file: Option<(String, u32)>,
    /// `None` if the cmap could not be read; glyphs are then probed with `HasCharacter`.
    coverage: Option<Arc<Coverage>>,
}

impl Resolved {
    fn covers(&self, ch: char) -> bool {
        self.coverage
            .as_ref()
            .map_or_else(|| self.font.has_character(ch), |c| c.contains(ch))
    }
}

/// [`FontProvider`] backed by DirectWrite.
pub struct DWriteFontProvider {
    system: Option<ffi::Collection>,
    /// Font files from the user font folder and extra folders, loaded on the first lookup the
    /// system collection cannot answer (parsing ~100 files costs a few hundred ms, and on
    /// Windows 10 1809+ user-installed fonts are normally in the system set already).
    extra_paths: Vec<PathBuf>,
    extra: OnceCell<Option<(ffi::Collection, usize)>>,
    resolved: RefCell<HashMap<FontSpec, Option<Rc<Resolved>>>>,
}

impl std::fmt::Debug for DWriteFontProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DWriteFontProvider")
            .field("system", &self.system.is_some())
            .field("extra_paths", &self.extra_paths.len())
            .field("resolved", &self.resolved.borrow().len())
            .finish_non_exhaustive()
    }
}

/// `%LOCALAPPDATA%\Microsoft\Windows\Fonts`.
#[must_use]
pub fn user_fonts_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| {
        PathBuf::from(d)
            .join("Microsoft")
            .join("Windows")
            .join("Fonts")
    })
}

fn is_font_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["ttf", "otf", "ttc", "otc"]
            .iter()
            .any(|x| e.eq_ignore_ascii_case(x))
    })
}

fn font_files_in(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_font_file(p))
        .collect();
    files.sort();
    out.extend(files);
}

impl DWriteFontProvider {
    /// System + user-installed fonts.
    ///
    /// # Errors
    /// Fails if DirectWrite cannot create the font collection.
    pub fn new() -> Result<Self, FontsDwriteError> {
        Self::with_sources(&FontSources::default())
    }

    /// Fonts from `sources`.
    ///
    /// # Errors
    /// Fails if DirectWrite cannot create the font collection.
    pub fn with_sources(sources: &FontSources) -> Result<Self, FontsDwriteError> {
        let mut files = Vec::new();
        if sources.user_fonts {
            if let Some(dir) = user_fonts_dir() {
                font_files_in(&dir, &mut files);
            }
        }
        for dir in &sources.dirs {
            font_files_in(dir, &mut files);
        }
        let system = if sources.system {
            Some(ffi::Collection::system()?)
        } else {
            None
        };
        Ok(Self {
            system,
            extra_paths: files,
            extra: OnceCell::new(),
            resolved: RefCell::new(HashMap::new()),
        })
    }

    fn extra(&self) -> Option<&ffi::Collection> {
        self.extra
            .get_or_init(|| {
                if self.extra_paths.is_empty() {
                    return None;
                }
                let refs: Vec<&Path> = self.extra_paths.iter().map(PathBuf::as_path).collect();
                ffi::Collection::from_files(&refs).ok()
            })
            .as_ref()
            .map(|(c, _)| c)
    }

    /// Number of font files accepted from the user font folder and extra folders (loads them
    /// if that has not happened yet).
    #[must_use]
    pub fn extra_file_count(&self) -> usize {
        let _ = self.extra();
        self.extra
            .get()
            .and_then(Option::as_ref)
            .map_or(0, |(_, n)| *n)
    }

    /// The face `spec` resolves to, or `None` if no font has that family name.
    #[must_use]
    pub fn matched_face(&self, spec: &FontSpec) -> Option<MatchedFace> {
        let r = self.resolve(spec)?;
        Some(MatchedFace {
            attrs: r.attrs,
            file: r.file.as_ref().map(|(p, idx)| (PathBuf::from(p), *idx)),
        })
    }

    fn resolve(&self, spec: &FontSpec) -> Option<Rc<Resolved>> {
        if let Some(hit) = self.resolved.borrow().get(spec) {
            return hit.clone();
        }
        let resolved = self.resolve_uncached(spec).map(Rc::new);
        self.resolved
            .borrow_mut()
            .insert(spec.clone(), resolved.clone());
        resolved
    }

    fn resolve_uncached(&self, spec: &FontSpec) -> Option<Resolved> {
        let family = spec.family.trim();
        if family.is_empty() {
            return None;
        }
        let slant = if spec.italic {
            Slant::Italic
        } else {
            Slant::Normal
        };
        let font = self
            .system
            .as_ref()
            .and_then(|c| find_in(c, family, spec.weight, slant))
            .or_else(|| find_in(self.extra()?, family, spec.weight, slant))?;
        let attrs = font.attrs();
        let file = font.file_key();
        let key = file.as_ref().map(|(path, idx)| (path.to_lowercase(), *idx));
        let coverage = coverage_for(&font, key.as_ref());
        Some(Resolved {
            font,
            attrs,
            file,
            coverage,
        })
    }
}

/// Looks `family` up as a WWS family, then by GDI family / full / PostScript name, and picks
/// the face with `GetFirstMatchingFont`.
fn find_in(c: &ffi::Collection, family: &str, weight: u16, slant: Slant) -> Option<ffi::Font> {
    if let Some(fam) = c.find_family(family) {
        c.first_matching(fam, weight, slant)
    } else {
        c.by_other_name(family)?.first_matching(0, weight, slant)
    }
}

impl FontProvider for DWriteFontProvider {
    fn has_font(&self, spec: &FontSpec) -> bool {
        self.resolve(spec).is_some()
    }

    fn missing_glyphs(&self, spec: &FontSpec, glyphs: &BTreeSet<char>) -> Vec<char> {
        let Some(r) = self.resolve(spec) else {
            return glyphs.iter().copied().collect();
        };
        glyphs
            .iter()
            .copied()
            .filter(|&ch| !ch.is_whitespace() && !r.covers(ch))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_bitmap_and_astral_ranges() {
        let c = Coverage::from_ranges(&[
            (0x20, 0x7E),
            (0xA0, 0xFF),
            (0x3040, 0x309F),
            (0xFFF0, 0x1_0010),
            (0x1F600, 0x1F64F),
        ]);
        assert!(c.contains('A') && c.contains('~') && c.contains('ÿ') && c.contains('あ'));
        assert!(!c.contains('\u{7F}') && !c.contains('Ā') && !c.contains('漢'));
        assert!(c.contains('\u{FFFD}') && c.contains('\u{10005}') && c.contains('😀'));
        assert!(!c.contains('\u{10011}') && !c.contains('\u{1F650}'));
    }

    #[test]
    fn font_file_extensions() {
        assert!(is_font_file(Path::new("a.TTF")));
        assert!(is_font_file(Path::new("b.otc")));
        assert!(!is_font_file(Path::new("c.fon")));
        assert!(!is_font_file(Path::new("noext")));
    }
}
