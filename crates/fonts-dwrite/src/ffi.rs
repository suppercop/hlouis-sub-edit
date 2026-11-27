//! The only `unsafe` in this crate: thin safe wrappers over the DirectWrite COM API.
//!
//! Every DirectWrite method in `windows` is an `unsafe fn` because it calls through a COM vtable.
//! The wrappers below only call them on live interface pointers owned by `windows` smart
//! pointers (released on drop) with correctly sized buffers, which is what each SAFETY comment
//! refers to.

use std::path::Path;
use windows::core::{Interface, Result, BOOL, HSTRING, PCWSTR};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory6, IDWriteFont, IDWriteFontCollection, IDWriteFontFace,
    IDWriteFontFace1, IDWriteFontFile, IDWriteFontSet, IDWriteFontSetBuilder2,
    IDWriteLocalFontFileLoader, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_FAMILY_MODEL_WEIGHT_STRETCH_STYLE, DWRITE_FONT_PROPERTY,
    DWRITE_FONT_PROPERTY_ID_FULL_NAME, DWRITE_FONT_PROPERTY_ID_POSTSCRIPT_NAME,
    DWRITE_FONT_PROPERTY_ID_WIN32_FAMILY_NAME, DWRITE_FONT_STRETCH, DWRITE_FONT_STYLE,
    DWRITE_FONT_WEIGHT, DWRITE_UNICODE_RANGE,
};

/// Style of a DirectWrite font face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slant {
    Normal,
    Oblique,
    Italic,
}

impl Slant {
    const fn from_dwrite(s: DWRITE_FONT_STYLE) -> Self {
        match s.0 {
            1 => Self::Oblique,
            2 => Self::Italic,
            _ => Self::Normal,
        }
    }

    const fn to_dwrite(self) -> DWRITE_FONT_STYLE {
        DWRITE_FONT_STYLE(match self {
            Self::Normal => 0,
            Self::Oblique => 1,
            Self::Italic => 2,
        })
    }
}

/// Weight / stretch / style of one font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FaceAttrs {
    pub weight: u16,
    pub stretch: u8,
    pub slant: Slant,
}

/// A DirectWrite font set and the WWS font collection built from it.
pub struct Collection {
    factory: IDWriteFactory6,
    set: IDWriteFontSet,
    fonts: IDWriteFontCollection,
}

/// A resolved font.
pub struct Font {
    font: IDWriteFont,
}

impl Collection {
    fn factory() -> Result<IDWriteFactory6> {
        // SAFETY: plain factory creation; the returned interface is owned by the smart pointer.
        unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
    }

    /// The system font set / collection (served by the Windows font cache, so cheap; on
    /// Windows 10 1809+ it also contains per-user installed fonts).
    pub fn system() -> Result<Self> {
        let factory = Self::factory()?;
        // SAFETY: live interface; results are owned smart pointers.
        let (set, fonts) = unsafe {
            (
                factory.GetSystemFontSet(false)?.cast::<IDWriteFontSet>()?,
                factory
                    .GetSystemFontCollection(false, DWRITE_FONT_FAMILY_MODEL_WEIGHT_STRETCH_STYLE)?
                    .cast::<IDWriteFontCollection>()?,
            )
        };
        Ok(Self {
            factory,
            set,
            fonts,
        })
    }

    /// A collection of the font files at `files` (unreadable / non-font files are skipped).
    /// Returns the collection and the number of files that were accepted.
    pub fn from_files(files: &[&Path]) -> Result<(Self, usize)> {
        let factory = Self::factory()?;
        // SAFETY: `factory` is a live interface.
        let builder: IDWriteFontSetBuilder2 = unsafe { factory.CreateFontSetBuilder()? };
        let mut accepted = 0usize;
        for path in files {
            let wide = HSTRING::from(path.as_os_str());
            // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
            if unsafe { builder.AddFontFile(&wide) }.is_ok() {
                accepted += 1;
            }
        }
        // SAFETY: live interface; the result is an owned smart pointer.
        let set = unsafe { builder.CreateFontSet()? };
        Ok((Self::from_set(factory, set)?, accepted))
    }
    fn from_set(factory: IDWriteFactory6, set: IDWriteFontSet) -> Result<Self> {
        // SAFETY: live interfaces; the result is an owned smart pointer.
        let fonts = unsafe {
            factory
                .CreateFontCollectionFromFontSet(
                    &set,
                    DWRITE_FONT_FAMILY_MODEL_WEIGHT_STRETCH_STYLE,
                )?
                .cast::<IDWriteFontCollection>()?
        };
        Ok(Self {
            factory,
            set,
            fonts,
        })
    }

    fn family_count(&self) -> u32 {
        // SAFETY: live interface.
        unsafe { self.fonts.GetFontFamilyCount() }
    }

    /// Index of the WWS family called `name` (DirectWrite compares case-insensitively across
    /// all localized family names).
    pub fn find_family(&self, name: &str) -> Option<u32> {
        let wide = HSTRING::from(name);
        let mut index = 0u32;
        let mut exists = BOOL(0);
        // SAFETY: `wide` outlives the call; out-pointers reference live locals.
        unsafe {
            self.fonts
                .FindFamilyName(&wide, &raw mut index, &raw mut exists)
                .ok()?;
        }
        exists.as_bool().then_some(index)
    }

    /// The fonts whose GDI family name, full name or PostScript name (tried in that order, any
    /// locale) is `name`, as their own collection. `None` if no font has such a name.
    pub fn by_other_name(&self, name: &str) -> Option<Self> {
        let wide = HSTRING::from(name);
        let empty = HSTRING::new();
        for id in [
            DWRITE_FONT_PROPERTY_ID_WIN32_FAMILY_NAME,
            DWRITE_FONT_PROPERTY_ID_FULL_NAME,
            DWRITE_FONT_PROPERTY_ID_POSTSCRIPT_NAME,
        ] {
            let prop = DWRITE_FONT_PROPERTY {
                propertyId: id,
                propertyValue: PCWSTR(wide.as_ptr()),
                localeName: PCWSTR(empty.as_ptr()),
            };
            // SAFETY: `prop` points at `wide` / `empty`, which outlive the call; live interface.
            let Ok(subset) = (unsafe { self.set.GetMatchingFonts2(&[prop]) }) else {
                continue;
            };
            // SAFETY: live interface.
            if unsafe { subset.GetFontCount() } == 0 {
                continue;
            }
            if let Ok(sub) = Self::from_set(self.factory.clone(), subset) {
                if sub.family_count() > 0 {
                    return Some(sub);
                }
            }
        }
        None
    }

    /// `IDWriteFontFamily::GetFirstMatchingFont` for normal stretch.
    pub fn first_matching(&self, family: u32, weight: u16, slant: Slant) -> Option<Font> {
        // SAFETY: live interfaces; `family` is below the family count.
        unsafe {
            let fam = self.fonts.GetFontFamily(family).ok()?;
            let font = fam
                .GetFirstMatchingFont(
                    DWRITE_FONT_WEIGHT(i32::from(weight)),
                    DWRITE_FONT_STRETCH(5),
                    slant.to_dwrite(),
                )
                .ok()?;
            Some(Font { font })
        }
    }
}

/// # Safety
/// `font` must be a live interface.
unsafe fn attrs_of(font: &IDWriteFont) -> FaceAttrs {
    // SAFETY: caller guarantees `font` is live.
    unsafe {
        FaceAttrs {
            weight: u16::try_from(font.GetWeight().0).unwrap_or(400),
            stretch: u8::try_from(font.GetStretch().0).unwrap_or(5),
            slant: Slant::from_dwrite(font.GetStyle()),
        }
    }
}
impl Font {
    pub fn attrs(&self) -> FaceAttrs {
        // SAFETY: `self.font` is live for the lifetime of `self`.
        unsafe { attrs_of(&self.font) }
    }

    /// `IDWriteFont::HasCharacter`.
    pub fn has_character(&self, ch: char) -> bool {
        // SAFETY: `self.font` is live.
        unsafe { self.font.HasCharacter(u32::from(ch)) }.is_ok_and(BOOL::as_bool)
    }

    fn face(&self) -> Option<IDWriteFontFace> {
        // SAFETY: `self.font` is live.
        unsafe { self.font.CreateFontFace() }.ok()
    }

    /// Identity of the underlying file (`path`, face index) when the font is a local file.
    pub fn file_key(&self) -> Option<(String, u32)> {
        let face = self.face()?;
        // SAFETY: live interfaces. `GetFiles` is called first with a null array to get the
        // count, then with an array of exactly that many slots. The reference key returned by
        // `GetReferenceKey` stays valid while `file` is alive, which spans its use below, and
        // the path buffer is sized to the reported length + NUL.
        unsafe {
            let mut count = 0u32;
            face.GetFiles(&raw mut count, None).ok()?;
            if count == 0 {
                return None;
            }
            let mut files: Vec<Option<IDWriteFontFile>> = vec![None; count as usize];
            face.GetFiles(&raw mut count, Some(files.as_mut_ptr()))
                .ok()?;
            let file = files.into_iter().next().flatten()?;
            let mut key: *mut core::ffi::c_void = core::ptr::null_mut();
            let mut key_size = 0u32;
            file.GetReferenceKey(&raw mut key, &raw mut key_size).ok()?;
            let loader = file
                .GetLoader()
                .ok()?
                .cast::<IDWriteLocalFontFileLoader>()
                .ok()?;
            let len = loader.GetFilePathLengthFromKey(key, key_size).ok()? as usize;
            let mut buf = vec![0u16; len + 1];
            loader.GetFilePathFromKey(key, key_size, &mut buf).ok()?;
            Some((String::from_utf16_lossy(&buf[..len]), face.GetIndex()))
        }
    }

    /// Unicode ranges mapped by the font's `cmap` (sorted, inclusive).
    pub fn unicode_ranges(&self) -> Option<Vec<(u32, u32)>> {
        let face = self.face()?.cast::<IDWriteFontFace1>().ok()?;
        // SAFETY: live interface; first call only queries the count (it reports
        // E_NOT_SUFFICIENT_BUFFER, which is expected), the second fills a buffer of that size.
        unsafe {
            let mut count = 0u32;
            let _ = face.GetUnicodeRanges(None, &raw mut count);
            let mut ranges = vec![DWRITE_UNICODE_RANGE::default(); count as usize];
            face.GetUnicodeRanges(Some(&mut ranges), &raw mut count)
                .ok()?;
            ranges.truncate(count as usize);
            let mut out: Vec<(u32, u32)> = ranges.iter().map(|r| (r.first, r.last)).collect();
            out.sort_unstable();
            Some(out)
        }
    }
}
