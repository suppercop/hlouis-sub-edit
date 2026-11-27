//! Color-space math (sRGB <-> Linear RGB <-> XYZ <-> CIELAB / `OKLab`), WCAG 2.1 contrast,
//! Bradford chromatic adaptation (D65 -> warm CCT), and Viénot 1999 CVD simulation.

/// 8-bit sRGB RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba8 {
    /// Creates an opaque sRGB color.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Creates an sRGB color with explicit alpha.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Formats as `#RRGGBB` hex string.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

/// CIELAB (D65) coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lab {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

impl Lab {
    /// Chroma `C* = sqrt(a*^2 + b*^2)`.
    #[inline]
    #[must_use]
    pub fn chroma(self) -> f64 {
        self.a.hypot(self.b)
    }
}

/// `OKLab` perceptual coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OkLab {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

/// Color-vision deficiency simulation model (Viénot et al. 1999 linear sRGB matrices).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CvdKind {
    Deuteranopia,
    Protanopia,
}

#[inline]
#[must_use]
pub fn srgb_u8_to_linear(c: u8) -> f64 {
    let v = f64::from(c) / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn linear_to_srgb_u8(v: f64) -> u8 {
    let c = v.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Relative luminance per WCAG 2.1 (`0.0..=1.0`).
#[inline]
#[must_use]
pub fn relative_luminance(c: Rgba8) -> f64 {
    0.2126 * srgb_u8_to_linear(c.r)
        + 0.7152 * srgb_u8_to_linear(c.g)
        + 0.0722 * srgb_u8_to_linear(c.b)
}

/// WCAG 2.1 contrast ratio (`1.0..=21.0`).
#[inline]
#[must_use]
pub fn contrast(a: Rgba8, b: Rgba8) -> f64 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Linear sRGB to CIE XYZ (D65).
#[inline]
fn linear_rgb_to_xyz(r: f64, g: f64, b: f64) -> [f64; 3] {
    [
        0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b,
        0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b,
        0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b,
    ]
}

/// CIE XYZ (D65) to Linear sRGB.
#[inline]
fn xyz_to_linear_rgb(xyz: [f64; 3]) -> [f64; 3] {
    [
        3.240_454_2 * xyz[0] - 1.537_138_5 * xyz[1] - 0.498_531_4 * xyz[2],
        -0.969_266_0 * xyz[0] + 1.876_010_8 * xyz[1] + 0.041_556_0 * xyz[2],
        0.055_643_4 * xyz[0] - 0.204_025_9 * xyz[1] + 1.057_225_2 * xyz[2],
    ]
}

const D65_XYZ: [f64; 3] = [0.950_47, 1.000_00, 1.088_83];

/// Converts sRGB [`Rgba8`] to CIELAB (D65).
#[must_use]
pub fn rgba_to_lab(c: Rgba8) -> Lab {
    let xyz = linear_rgb_to_xyz(
        srgb_u8_to_linear(c.r),
        srgb_u8_to_linear(c.g),
        srgb_u8_to_linear(c.b),
    );
    let f = |t: f64| {
        if t > 0.008_856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let fx = f(xyz[0] / D65_XYZ[0]);
    let fy = f(xyz[1] / D65_XYZ[1]);
    let fz = f(xyz[2] / D65_XYZ[2]);
    Lab {
        l: 116.0 * fy - 16.0,
        a: 500.0 * (fx - fy),
        b: 200.0 * (fy - fz),
    }
}

/// Converts CIELAB (D65) to sRGB [`Rgba8`] with alpha `alpha`.
#[must_use]
pub fn lab_to_rgba(lab: Lab, alpha: u8) -> Rgba8 {
    let fy = (lab.l + 16.0) / 116.0;
    let fx = lab.a / 500.0 + fy;
    let fz = fy - lab.b / 200.0;
    let inv = |t: f64| {
        let t3 = t * t * t;
        if t3 > 0.008_856 {
            t3
        } else {
            (t - 16.0 / 116.0) / 7.787
        }
    };
    let xyz = [
        D65_XYZ[0] * inv(fx),
        D65_XYZ[1] * inv(fy),
        D65_XYZ[2] * inv(fz),
    ];
    let rgb = xyz_to_linear_rgb(xyz);
    Rgba8::rgba(
        linear_to_srgb_u8(rgb[0]),
        linear_to_srgb_u8(rgb[1]),
        linear_to_srgb_u8(rgb[2]),
        alpha,
    )
}

/// CIE76 `ΔE` distance in CIELAB.
#[inline]
#[must_use]
pub fn delta_e_lab(a: Rgba8, b: Rgba8) -> f64 {
    let la = rgba_to_lab(a);
    let lb = rgba_to_lab(b);
    let dl = la.l - lb.l;
    let da = la.a - lb.a;
    let db = la.b - lb.b;
    (dl * dl + da * da + db * db).sqrt()
}

/// Converts sRGB [`Rgba8`] to `OKLab` (Björn Ottosson 2020).
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn rgba_to_oklab(c: Rgba8) -> OkLab {
    let r = srgb_u8_to_linear(c.r);
    let g = srgb_u8_to_linear(c.g);
    let b = srgb_u8_to_linear(c.b);
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    OkLab {
        l: 0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        a: 1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        b: 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    }
}

/// Converts `OKLab` to sRGB [`Rgba8`] with alpha `alpha`.
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn oklab_to_rgba(ok: OkLab, alpha: u8) -> Rgba8 {
    let l_ = ok.l + 0.396_337_777_4 * ok.a + 0.215_803_757_3 * ok.b;
    let m_ = ok.l - 0.105_561_345_8 * ok.a - 0.063_854_172_8 * ok.b;
    let s_ = ok.l - 0.089_484_177_5 * ok.a - 1.291_485_548_0 * ok.b;
    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;
    let r = 4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s;
    let g = -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s;
    let b = -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s;
    Rgba8::rgba(
        linear_to_srgb_u8(r),
        linear_to_srgb_u8(g),
        linear_to_srgb_u8(b),
        alpha,
    )
}

/// Simulates deuteranopia or protanopia using Viénot 1999 linear-sRGB matrices.
#[must_use]
pub fn simulate_cvd(c: Rgba8, kind: CvdKind) -> Rgba8 {
    let r = srgb_u8_to_linear(c.r);
    let g = srgb_u8_to_linear(c.g);
    let b = srgb_u8_to_linear(c.b);
    let (nr, ng, nb) = match kind {
        CvdKind::Deuteranopia => (
            0.292_75 * r + 0.707_25 * g,
            0.292_75 * r + 0.707_25 * g,
            -0.022_34 * r + 0.022_34 * g + b,
        ),
        CvdKind::Protanopia => (
            0.112_38 * r + 0.887_62 * g,
            0.112_38 * r + 0.887_62 * g,
            0.004_01 * r - 0.004_01 * g + b,
        ),
    };
    Rgba8::rgba(
        linear_to_srgb_u8(nr),
        linear_to_srgb_u8(ng),
        linear_to_srgb_u8(nb),
        c.a,
    )
}

/// Computes CIE XYZ white point for a Planckian blackbody at temperature `kelvin` (`2000..=10000 K`).
fn cct_to_xyz(kelvin: f64) -> [f64; 3] {
    let t = kelvin.clamp(2000.0, 10_000.0);
    // Kang et al. (2002) Planckian locus approximation
    let x = if t <= 4000.0 {
        -0.266_123_9e9 / (t * t * t) - 0.234_358_9e6 / (t * t) + 0.877_695_6e3 / t + 0.179_910
    } else {
        -3.025_846_9e9 / (t * t * t) + 2.107_037_9e6 / (t * t) + 0.222_634_7e3 / t + 0.240_390
    };
    let y = if t <= 2222.0 {
        -1.106_381_4 * x * x * x - 1.348_110_20 * x * x + 2.185_558_32 * x - 0.202_196_83
    } else if t <= 4000.0 {
        -0.954_947_6 * x * x * x - 1.374_185_93 * x * x + 2.091_370_15 * x - 0.167_488_67
    } else {
        3.081_758_0 * x * x * x - 5.873_386_70 * x * x + 3.751_129_97 * x - 0.370_014_83
    };
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// Bradford cone-response matrix (`M_BFD`) and its inverse.
const BFD_M: [[f64; 3]; 3] = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];
const BFD_INV: [[f64; 3]; 3] = [
    [0.986_992_9, -0.147_054_3, 0.159_962_7],
    [0.432_305_3, 0.518_360_3, 0.049_291_2],
    [-0.008_528_7, 0.040_042_8, 0.968_486_7],
];

#[inline]
fn mul_mat3(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Applies Bradford chromatic adaptation in linear light from D65 (`6504 K`) to a warm white point
/// determined by `strength` (`0..=100`: `0 -> 6504 K`, `60 -> 4500 K`, `100 -> 3400 K`), plus
/// gentle linear-blue attenuation at high strengths to guarantee `<= 50%` linear blue at `100`.
#[must_use]
pub fn warm(color: Rgba8, strength: u8) -> Rgba8 {
    let s = f64::from(strength.min(100)) / 100.0;
    if s <= 0.0 {
        return color;
    }
    // Piecewise-smooth CCT curve: 0 -> 6504 K, 0.6 -> 4500 K, 1.0 -> 3400 K
    let cct = if s <= 0.6 {
        6504.0 - (6504.0 - 4500.0) * (s / 0.6)
    } else {
        4500.0 - (4500.0 - 3400.0) * ((s - 0.6) / 0.4)
    };
    let dst_white = cct_to_xyz(cct);
    let src_lms = mul_mat3(&BFD_M, D65_XYZ);
    let dst_lms = mul_mat3(&BFD_M, dst_white);

    let r = srgb_u8_to_linear(color.r);
    let g = srgb_u8_to_linear(color.g);
    let b = srgb_u8_to_linear(color.b);
    let xyz = linear_rgb_to_xyz(r, g, b);
    let lms = mul_mat3(&BFD_M, xyz);
    let adapted_lms = [
        lms[0] * (dst_lms[0] / src_lms[0]),
        lms[1] * (dst_lms[1] / src_lms[1]),
        lms[2] * (dst_lms[2] / src_lms[2]),
    ];
    let adapted_xyz = mul_mat3(&BFD_INV, adapted_lms);
    let mut rgb = xyz_to_linear_rgb(adapted_xyz);

    // Subtle gamut-preserving normalization + small luminance-range compression
    let max_c = rgb[0].max(rgb[1]).max(rgb[2]);
    if max_c > 1.0 {
        let inv = 1.0 / max_c;
        rgb[0] *= inv;
        rgb[1] *= inv;
        rgb[2] *= inv;
    }
    // Extra linear-blue attenuation ramping smoothly up to 25% at strength 100
    rgb[2] *= 1.0 - 0.25 * s * s;

    Rgba8::rgba(
        linear_to_srgb_u8(rgb[0]),
        linear_to_srgb_u8(rgb[1]),
        linear_to_srgb_u8(rgb[2]),
        color.a,
    )
}

/// Caps CIELAB chroma `C*` of `color` to `max_chroma` while preserving `L*` and hue.
#[must_use]
pub fn cap_chroma(color: Rgba8, max_chroma: f64) -> Rgba8 {
    let mut lab = rgba_to_lab(color);
    let c = lab.chroma();
    if c > max_chroma && c > 1e-6 {
        let scale = max_chroma / c;
        lab.a *= scale;
        lab.b *= scale;
        return lab_to_rgba(lab, color.a);
    }
    color
}

/// Nudges `fg`'s CIELAB `L*` (preserving hue/chroma) so `contrast(fg, bg)` lands inside
/// `[min_ratio, max_ratio]`.
#[must_use]
pub fn nudge_contrast(fg: Rgba8, bg: Rgba8, min_ratio: f64, max_ratio: f64) -> Rgba8 {
    let cur = contrast(fg, bg);
    if cur >= min_ratio && cur <= max_ratio {
        return fg;
    }
    let bg_lab = rgba_to_lab(bg);
    let fg_lab = rgba_to_lab(fg);
    let darker_fg = fg_lab.l < bg_lab.l || bg_lab.l >= 50.0;

    // Binary search L* toward the target contrast band.
    let (mut lo, mut hi) = if darker_fg {
        (4.0_f64, bg_lab.l.min(96.0))
    } else {
        (bg_lab.l.max(4.0), 96.0_f64)
    };

    let target = if cur < min_ratio {
        min_ratio + 0.35
    } else {
        max_ratio - 0.35
    };

    let mut best = fg;
    let mut best_err = f64::INFINITY;

    for _ in 0..14 {
        let mid = f64::midpoint(lo, hi);
        let cand = lab_to_rgba(
            Lab {
                l: mid,
                a: fg_lab.a,
                b: fg_lab.b,
            },
            fg.a,
        );
        let r = contrast(cand, bg);
        let err = (r - target).abs();
        if err < best_err {
            best_err = err;
            best = cand;
        }
        if darker_fg {
            // Lower L* => higher contrast
            if r < target {
                hi = mid;
            } else {
                lo = mid;
            }
        } else {
            // Higher L* => higher contrast
            if r < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
    }
    best
}
