//! Charset detection and decoding (BOM, UTF-8, and `chardetng` + `encoding_rs` fallback).

use chardetng::EncodingDetector;
use encoding_rs::{
    Encoding, BIG5, GB18030, GBK, ISO_8859_2, UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1250,
    WINDOWS_1252, WINDOWS_1254, WINDOWS_1257, WINDOWS_1258,
};

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
///    with a Vietnamese Windows-1258 disambiguation check for short 1-3 word lines.
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

    // `chardetng` does not distinguish Windows-1258 from single-byte Latin encodings
    // (especially on short 1-3 word Vietnamese lines like "Việt" or "được không").
    let is_single_byte_latin = enc == WINDOWS_1252
        || enc == WINDOWS_1250
        || enc == ISO_8859_2
        || enc == WINDOWS_1254
        || enc == WINDOWS_1257;
    if is_single_byte_latin && looks_like_windows_1258(bytes) {
        enc = WINDOWS_1258;
    } else if enc == GBK {
        // GB18030 is a strict superset of GBK; upgrading handles 4-byte Chinese extensions too.
        enc = GB18030;
    } else if enc == BIG5 && looks_like_spanish_inverted_punct(bytes) {
        // A single `¡` (0xA1) or `¿` (0xBF) followed by ASCII letters is Spanish CP1252, not Big5.
        enc = WINDOWS_1252;
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

/// Discriminates Windows-1258 (Vietnamese) from Windows-1252 (French/Spanish/German/etc.),
/// including short 1-3 word strings such as `"Việt"`, `"được không"`, and `"Tiếng Việt"`.
fn looks_like_windows_1258(bytes: &[u8]) -> bool {
    let mut tone_after_vowel = 0usize;
    let mut viet_unique_letter = 0usize;
    let mut cp1252_western_marker = 0usize;

    for (idx, &b) in bytes.iter().enumerate() {
        // Characters common in CP1252 French/Spanish/German that do not occur in Vietnamese text:
        // ¡ ¿ ß ä ö ü Ä Ö Ü ñ Ñ ç Ç « »
        if matches!(
            b,
            0xA1 | 0xAB
                | 0xBB
                | 0xBF
                | 0xC4
                | 0xC7
                | 0xD1
                | 0xD6
                | 0xDC
                | 0xDF
                | 0xE4
                | 0xE7
                | 0xF1
                | 0xF6
                | 0xFC
        ) {
            cp1252_western_marker += 1;
        }

        // Windows-1258 unique Vietnamese letters that do not appear in FR/ES/DE CP1252:
        // Ă/ă (0xC3/0xE3), Đ/đ (0xD0/0xF0), Ơ/ơ (0xD5/0xF5), Ư/ư (0xDD/0xFD), ₫ (0xFE).
        if matches!(
            b,
            0xC3 | 0xE3 | 0xD0 | 0xF0 | 0xD5 | 0xF5 | 0xDD | 0xFD | 0xFE
        ) {
            let prev_is_letter = idx > 0 && is_latin_or_viet_byte(bytes[idx - 1]);
            let next_is_letter = idx + 1 < bytes.len() && is_latin_or_viet_byte(bytes[idx + 1]);
            if prev_is_letter || next_is_letter || b == 0xFE {
                viet_unique_letter += 1;
            }
        }

        // Combining tone marks in Windows-1258:
        // 0xCC (grave), 0xD2 (hook above), 0xDE (tilde), 0xEC (acute), 0xF2 (dot below).
        // In Vietnamese they always follow a vowel immediately.
        if idx > 0 && matches!(b, 0xCC | 0xD2 | 0xDE | 0xEC | 0xF2) && is_viet_vowel(bytes[idx - 1])
        {
            tone_after_vowel += 1;
        }
    }

    if tone_after_vowel >= 1 && cp1252_western_marker == 0 {
        return true;
    }
    if tone_after_vowel >= 2 {
        return true;
    }
    viet_unique_letter >= 1 && cp1252_western_marker == 0
}

fn looks_like_spanish_inverted_punct(bytes: &[u8]) -> bool {
    let high_count = bytes.iter().filter(|&&b| b >= 0x80).count();
    let inverted_punct = bytes
        .windows(2)
        .any(|w| matches!(w[0], 0xA1 | 0xBF) && w[1].is_ascii_alphabetic());
    inverted_punct && high_count <= 2
}

const fn is_viet_vowel(b: u8) -> bool {
    matches!(
        b,
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
    )
}

const fn is_latin_or_viet_byte(b: u8) -> bool {
    b.is_ascii_alphabetic()
        || matches!(
            b,
            0xC2 | 0xC3
                | 0xCA
                | 0xCC
                | 0xD0
                | 0xD2
                | 0xD4
                | 0xD5
                | 0xDD
                | 0xDE
                | 0xE2
                | 0xE3
                | 0xEA
                | 0xEC
                | 0xF0
                | 0xF2
                | 0xF4
                | 0xF5
                | 0xFD
        )
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
    fn detects_shift_jis_and_gb18030() {
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
    }

    #[test]
    fn table_driven_short_windows_1258_vs_cp1252() {
        // 22 short 1-3 word Vietnamese phrases in Windows-1258 (using combining tone marks as
        // required by Windows-1258 encoding).
        let vietnamese_samples = [
            "Vi\u{00EA}\u{0323}t",
            "\u{0111}\u{01B0}\u{01A1}\u{0323}c kh\u{00F4}ng",
            "Ti\u{00EA}\u{0301}ng Vi\u{00EA}\u{0323}t",
            "Xin cha\u{0300}o",
            "Ca\u{0309}m \u{01A1}n",
            "Ta\u{0323}m bi\u{00EA}\u{0323}t",
            "\u{0103}n c\u{01A1}m",
            "\u{0111}i ngu\u{0309}",
            "kh\u{00F4}ng sao \u{0111}\u{00E2}u",
            "t\u{00F4}i bi\u{00EA}\u{0301}t",
            "c\u{00F3} th\u{00EA}\u{0309}",
            "m\u{00F4}\u{0323}t chu\u{0301}t",
            "nhanh l\u{00EA}n na\u{0300}o",
            "\u{0111}\u{01B0}\u{0300}ng lo",
            "he\u{0323}n g\u{0103}\u{0323}p la\u{0323}i",
            "ng\u{01B0}\u{01A1}\u{0300}i ba\u{0323}n",
            "c\u{00E2}u chuy\u{00EA}\u{0323}n",
            "b\u{0103}\u{0301}t \u{0111}\u{00E2}\u{0300}u",
            "k\u{00EA}\u{0301}t thu\u{0301}c",
            "phu\u{0323} \u{0111}\u{00EA}\u{0300}",
            "m\u{01A1} \u{01B0}\u{01A1}\u{0301}c",
            "500\u{20AB}",
        ];

        for sample in vietnamese_samples {
            let (bytes, _, unmappable) = WINDOWS_1258.encode(sample);
            assert!(
                !unmappable,
                "test sample `{sample}` must be representable in Windows-1258"
            );
            let (decoded, enc) = decode_bytes(&bytes);
            assert_eq!(
                enc.name, "windows-1258",
                "failed to detect Windows-1258 for `{sample}` (got `{}`)",
                enc.name
            );
            assert_eq!(decoded, sample);
        }

        // 22 short 1-3 word French / Spanish / German phrases in Windows-1252.
        let cp1252_samples = [
            "Café crème",
            "Déjà vu",
            "Naïve élève",
            "Garçon français",
            "Très bien",
            "Où es-tu ?",
            "À bientôt",
            "Ça va bien",
            "Noël joyeux",
            "Cœur brisé",
            "¡Hola amigo!",
            "¿Cómo estás?",
            "Mañana mismo",
            "Niño pequeño",
            "Español clásico",
            "Corazón roto",
            "Grüße aus Köln",
            "Straße überqueren",
            "Schöne Träume",
            "München fährt",
            "Äpfel und Öl",
            "Überraschung!",
        ];

        for sample in cp1252_samples {
            let (bytes, _, unmappable) = WINDOWS_1252.encode(sample);
            assert!(
                !unmappable,
                "test sample `{sample}` must be representable in Windows-1252"
            );
            let (decoded, enc) = decode_bytes(&bytes);
            assert_eq!(
                enc.name, "windows-1252",
                "misclassified CP1252 sample `{sample}` as `{}`",
                enc.name
            );
            assert_eq!(decoded, sample);
        }
    }
}
