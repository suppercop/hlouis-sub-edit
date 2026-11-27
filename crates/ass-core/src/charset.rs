//! Charset detection and decoding (BOM, UTF-8, and `chardetng` + `encoding_rs` fallback).

use chardetng::EncodingDetector;
use encoding_rs::{Encoding, GB18030, GBK, UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1252, WINDOWS_1258};

/// Detected character encoding of a subtitle byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedEncoding {
    /// Canonical WHATWG encoding name (e.g. `"UTF-8"`, `"UTF-16LE"`, `"Shift_JIS"`, `"windows-1258"`).
    pub name: &'static str,
    /// Whether the input began with a Byte Order Mark (BOM).
    pub has_bom: bool,
    /// Whether decoding had to substitute malformed byte sequences with U+FFFD.
    pub had_errors: bool,
}

/// Decodes raw subtitle bytes into UTF-8 text and reports the detected encoding.
///
/// Detection order:
/// 1. Explicit BOM (`UTF-8`, `UTF-16LE`, `UTF-16BE`).
/// 2. Valid UTF-8 without BOM.
/// 3. Heuristic detection via `chardetng` (covering CP1252, Shift-JIS, GB18030, Windows-1258, etc.),
///    with a Vietnamese Windows-1258 disambiguation check when `chardetng` guesses Windows-1252.
#[must_use]
pub fn decode_bytes(bytes: &[u8]) -> (String, DetectedEncoding) {
    // 1. Check explicit BOMs first.
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let (cow, had_errors) = UTF_8.decode_without_bom_handling(rest);
        return (
            cow.into_owned(),
            DetectedEncoding {
                name: UTF_8.name(),
                has_bom: true,
                had_errors,
            },
        );
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let (cow, had_errors) = UTF_16LE.decode_without_bom_handling(rest);
        return (
            cow.into_owned(),
            DetectedEncoding {
                name: UTF_16LE.name(),
                has_bom: true,
                had_errors,
            },
        );
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (cow, had_errors) = UTF_16BE.decode_without_bom_handling(rest);
        return (
            cow.into_owned(),
            DetectedEncoding {
                name: UTF_16BE.name(),
                has_bom: true,
                had_errors,
            },
        );
    }

    // 2. Valid UTF-8 without BOM.
    if let Ok(valid_utf8) = std::str::from_utf8(bytes) {
        return (
            valid_utf8.to_owned(),
            DetectedEncoding {
                name: UTF_8.name(),
                has_bom: false,
                had_errors: false,
            },
        );
    }

    // 3. Heuristic detection via chardetng + encoding_rs.
    let mut detector = EncodingDetector::new();
    detector.feed(bytes, true);
    let mut enc: &'static Encoding = detector.guess(None, true);

    // `chardetng` does not distinguish Windows-1258 (Vietnamese combining tone marks 0xCC, 0xD2,
    // 0xDE, 0xEC, 0xF2 and ơ/ư/đ 0xD5/0xF5/0xDD/0xFD/0xD0/0xF0) from Windows-1252.
    if enc == WINDOWS_1252 && looks_like_windows_1258(bytes) {
        enc = WINDOWS_1258;
    } else if enc == GBK {
        // GB18030 is a strict superset of GBK; upgrading handles 4-byte Chinese extensions too.
        enc = GB18030;
    }

    let (cow, _, had_errors) = enc.decode(bytes);
    (
        cow.into_owned(),
        DetectedEncoding {
            name: enc.name(),
            has_bom: false,
            had_errors,
        },
    )
}

/// Heuristic check for Windows-1258 Vietnamese combining tone marks (`0xCC`, `0xD2`, `0xDE`, `0xEC`,
/// `0xF2`) immediately following ASCII/Vietnamese vowels (`a/e/i/o/u/y` or `ă/â/ê/ô/ơ/ư`).
fn looks_like_windows_1258(bytes: &[u8]) -> bool {
    let mut tone_hits = 0usize;
    let mut viet_vowel_hits = 0usize;
    for pair in bytes.windows(2) {
        let prev = pair[0];
        let curr = pair[1];
        let is_viet_base = matches!(
            prev,
            b'a' | b'e'
                | b'i'
                | b'o'
                | b'u'
                | b'y'
                | b'A'
                | b'E'
                | b'I'
                | b'O'
                | b'U'
                | b'Y'
                | 0xC3
                | 0xE3
                | 0xC2
                | 0xE2
                | 0xCA
                | 0xEA
                | 0xD4
                | 0xF4
                | 0xD5
                | 0xF5
                | 0xDD
                | 0xFD
        );
        // Combining grave (0xCC), hook above (0xD2), tilde (0xDE), acute (0xEC), dot below (0xF2).
        let is_combining_tone = matches!(curr, 0xCC | 0xD2 | 0xDE | 0xEC | 0xF2);
        if is_viet_base && is_combining_tone {
            tone_hits += 1;
        }
        // Windows-1258 specific letters: ă/Ă (0xE3/0xC3), đ/Đ (0xF0/0xD0), ơ/Ơ (0xF5/0xD5), ư/Ư (0xFD/0xDD).
        if matches!(curr, 0xC3 | 0xE3 | 0xD0 | 0xF0 | 0xD5 | 0xF5 | 0xDD | 0xFD) {
            viet_vowel_hits += 1;
        }
    }
    tone_hits >= 2 || (tone_hits >= 1 && viet_vowel_hits >= 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::SHIFT_JIS;

    #[test]
    fn detects_utf8_bom_and_plain_utf8() {
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice("Xin chào Tiếng Việt".as_bytes());
        let (text, enc) = decode_bytes(&with_bom);
        assert_eq!(text, "Xin chào Tiếng Việt");
        assert_eq!(enc.name, "UTF-8");
        assert!(enc.has_bom);
        assert!(!enc.had_errors);

        let (text2, enc2) = decode_bytes("Hello 日本語".as_bytes());
        assert_eq!(text2, "Hello 日本語");
        assert_eq!(enc2.name, "UTF-8");
        assert!(!enc2.has_bom);
    }

    #[test]
    fn detects_utf16_le_and_be_with_bom() {
        let sample = "[Script Info]\r\nTitle: Tiếng Việt 日本語";
        let mut le = vec![0xFF, 0xFE];
        for u in sample.encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        let (text_le, enc_le) = decode_bytes(&le);
        assert_eq!(text_le, sample);
        assert_eq!(enc_le.name, "UTF-16LE");
        assert!(enc_le.has_bom);

        let mut be = vec![0xFE, 0xFF];
        for u in sample.encode_utf16() {
            be.extend_from_slice(&u.to_be_bytes());
        }
        let (big_text, big_enc) = decode_bytes(&be);
        assert_eq!(big_text, sample);
        assert_eq!(big_enc.name, "UTF-16BE");
        assert!(big_enc.has_bom);
    }

    #[test]
    fn detects_shift_jis_gb18030_cp1252_and_windows_1258() {
        let jp = "日本語の字幕テストです。これはShift-JISで書かれた会話です。";
        let (sjis_bytes, _, _) = SHIFT_JIS.encode(jp);
        let (decoded_jp, enc_jp) = decode_bytes(&sjis_bytes);
        assert_eq!(enc_jp.name, "Shift_JIS");
        assert_eq!(decoded_jp, jp);

        let zh = "这是一段用于测试简体中文字符集自动检测的字幕文本内容。";
        let (gb_bytes, _, _) = GB18030.encode(zh);
        let (decoded_zh, enc_zh) = decode_bytes(&gb_bytes);
        assert_eq!(enc_zh.name, "gb18030");
        assert_eq!(decoded_zh, zh);

        let fr = "Café résumé naïve — élève à Paris, déjà vu.";
        let (cp_bytes, _, _) = WINDOWS_1252.encode(fr);
        let (decoded_fr, enc_fr) = decode_bytes(&cp_bytes);
        assert_eq!(enc_fr.name, "windows-1252");
        assert_eq!(decoded_fr, fr);

        // Windows-1258 uses NFD combining tone marks (e.g. "Tiê\u{0301}ng Viê\u{0323}t").
        let vi_nfd = "Xin cha\u{0300}o Ti\u{00EA}\u{0301}ng Vi\u{00EA}\u{0323}t \u{0111}\u{01B0}\u{01A1}\u{0323}c ma\u{0303} ho\u{0301}a";
        let (w1258_bytes, _, _) = WINDOWS_1258.encode(vi_nfd);
        let (decoded_vi, enc_vi) = decode_bytes(&w1258_bytes);
        assert_eq!(enc_vi.name, "windows-1258");
        assert_eq!(decoded_vi, vi_nfd);
    }
}
