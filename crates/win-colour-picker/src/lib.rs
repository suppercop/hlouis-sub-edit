//! Screen colour picker for Windows: the colour of the physical pixel under the cursor anywhere
//! on any monitor, plus magnifier data (e.g. a 9×9 block) around it.
//!
//! All coordinates are **physical pixels** in virtual-screen space: calls temporarily make the
//! calling thread per-monitor-DPI-aware (v2), so results are correct at any scaling factor and on
//! mixed-DPI setups even if the process itself is not DPI aware. No UI here; the colour dialog
//! draws the magnifier from [`Magnifier`] and polls [`pick_under_cursor`] while picking.

#![deny(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

#[allow(unsafe_code)]
mod ffi;

use thiserror::Error;

/// Errors from screen sampling.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PickError {
    /// The cursor position could not be read (e.g. no interactive desktop).
    #[error("cannot read cursor position: {0}")]
    Cursor(String),
    /// Screen capture failed (e.g. secure desktop, no interactive session).
    #[error("screen capture failed: {0}")]
    Capture(&'static str),
}

/// 8-bit RGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Point in physical virtual-screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Rectangle in physical virtual-screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    /// `true` if `p` lies inside the rectangle.
    #[must_use]
    pub const fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.width && p.y < self.y + self.height
    }
}

/// Square block of pixels centred on [`Magnifier::center`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Magnifier {
    /// Centre pixel (physical virtual-screen coordinates).
    pub center: Point,
    /// Pixels from the centre to each edge (`4` → 9×9).
    pub radius: u32,
    /// Effective DPI of the monitor under the centre (96 = 100 %), if known.
    pub dpi: Option<u32>,
    /// Row-major pixels, top row first; `None` where the block extends past every monitor.
    pub pixels: Vec<Option<Rgb>>,
}

impl Magnifier {
    /// Side length in pixels (`2 * radius + 1`).
    #[must_use]
    pub const fn side(&self) -> u32 {
        2 * self.radius + 1
    }

    /// Pixel at offset (`dx`, `dy`) from the centre, each in `-radius..=radius`.
    #[must_use]
    pub fn at(&self, dx: i32, dy: i32) -> Option<Rgb> {
        let r = i32::try_from(self.radius).ok()?;
        if dx.abs() > r || dy.abs() > r {
            return None;
        }
        let side = 2 * r + 1;
        let idx = usize::try_from((dy + r) * side + (dx + r)).ok()?;
        self.pixels.get(idx).copied().flatten()
    }

    /// The centre pixel's colour.
    #[must_use]
    pub fn center_colour(&self) -> Option<Rgb> {
        self.at(0, 0)
    }
}

/// Cursor position in physical pixels.
///
/// # Errors
/// [`PickError::Cursor`] if Windows refuses (no interactive desktop).
pub fn cursor_position() -> Result<Point, PickError> {
    let _dpi = ffi::PhysicalPixels::enter();
    ffi::cursor_pos().map_err(|e| PickError::Cursor(e.message()))
}

/// Bounds of the virtual screen (all monitors) in physical pixels.
#[must_use]
pub fn virtual_screen() -> Rect {
    let _dpi = ffi::PhysicalPixels::enter();
    ffi::virtual_screen()
}

/// Magnifier block of `(2 * radius + 1)²` physical pixels centred on `center`.
///
/// # Errors
/// [`PickError::Capture`] if the screen cannot be read.
pub fn magnifier_at(center: Point, radius: u32) -> Result<Magnifier, PickError> {
    let _dpi = ffi::PhysicalPixels::enter();
    let screen = ffi::virtual_screen();
    let r = i32::try_from(radius.min(1024)).map_err(|_| PickError::Capture("radius"))?;
    let side = 2 * r + 1;
    let rect = Rect {
        x: center.x - r,
        y: center.y - r,
        width: side,
        height: side,
    };
    let n = usize::try_from(side * side).map_err(|_| PickError::Capture("size"))?;
    let mut bgra = vec![0u8; n * 4];
    ffi::capture_bgra(rect, &mut bgra).map_err(PickError::Capture)?;
    Ok(Magnifier {
        center,
        radius: radius.min(1024),
        dpi: ffi::dpi_at(center),
        pixels: decode(&bgra, rect, screen),
    })
}

/// Magnifier block centred on the cursor (`radius = 4` gives the 9×9 view).
///
/// # Errors
/// See [`cursor_position`] and [`magnifier_at`].
pub fn magnifier_under_cursor(radius: u32) -> Result<Magnifier, PickError> {
    let _dpi = ffi::PhysicalPixels::enter();
    let p = ffi::cursor_pos().map_err(|e| PickError::Cursor(e.message()))?;
    magnifier_at(p, radius)
}

/// Colour of the physical pixel at `p` (`None` if `p` is outside every monitor).
///
/// # Errors
/// [`PickError::Capture`] if the screen cannot be read.
pub fn pixel_at(p: Point) -> Result<Option<Rgb>, PickError> {
    Ok(magnifier_at(p, 0)?.center_colour())
}

/// The cursor position and the colour of the pixel under it.
///
/// # Errors
/// See [`cursor_position`] and [`magnifier_at`].
pub fn pick_under_cursor() -> Result<(Point, Option<Rgb>), PickError> {
    let m = magnifier_under_cursor(0)?;
    Ok((m.center, m.center_colour()))
}

/// Converts top-down BGRA rows of `rect` to RGB, masking pixels outside `screen`.
fn decode(bgra: &[u8], rect: Rect, screen: Rect) -> Vec<Option<Rgb>> {
    let w = usize::try_from(rect.width).unwrap_or(0).max(1);
    bgra.as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .map(|(i, px)| {
            let (col, row) = (i % w, i / w);
            let p = Point {
                x: rect.x + i32::try_from(col).unwrap_or(0),
                y: rect.y + i32::try_from(row).unwrap_or(0),
            };
            screen.contains(p).then_some(Rgb {
                r: px[2],
                g: px[1],
                b: px[0],
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_swaps_bgr_and_masks_offscreen() {
        let rect = Rect {
            x: -1,
            y: 0,
            width: 2,
            height: 1,
        };
        let screen = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let px = decode(&[1, 2, 3, 0, 10, 20, 30, 0], rect, screen);
        assert_eq!(
            px,
            vec![
                None,
                Some(Rgb {
                    r: 30,
                    g: 20,
                    b: 10
                })
            ]
        );
    }

    #[test]
    fn magnifier_indexing() {
        let pixels = (0..81u8).map(|i| Some(Rgb { r: i, g: 0, b: 0 })).collect();
        let m = Magnifier {
            center: Point::default(),
            radius: 4,
            dpi: None,
            pixels,
        };
        assert_eq!(m.side(), 9);
        assert_eq!(m.center_colour().unwrap().r, 40);
        assert_eq!(m.at(-4, -4).unwrap().r, 0);
        assert_eq!(m.at(4, 4).unwrap().r, 80);
        assert_eq!(m.at(1, 0).unwrap().r, 41);
        assert_eq!(m.at(0, 1).unwrap().r, 49);
        assert_eq!(m.at(5, 0), None);
    }
}
