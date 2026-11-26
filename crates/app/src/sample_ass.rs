//! Built-in ASS sample subtitle script for Phase 0 video rendering tests.

/// Generates an ASS subtitle script covering 0..180 seconds with English, Vietnamese,
/// Japanese, override tags, and karaoke animations so playback benchmarks always have
/// active subtitles to render via `libass`.
#[must_use]
pub fn build_sample_ass(width: u32, height: u32) -> String {
    use std::fmt::Write as _;

    let w = width.max(640);
    let h = height.max(360);
    let mut script = format!(
        "\
[Script Info]
Title: hlouis-sub-edit Phase 0 Sample
ScriptType: v4.00+
PlayResX: {w}
PlayResY: {h}
ScaledBorderAndShadow: yes

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Segoe UI,54,&H00FFFFFF,&H0000FFFF,&H00181820,&H90000000,-1,0,0,0,100,100,0,0,1,3,1.5,2,40,40,36,1
Style: TopBanner,Segoe UI,36,&H00FFE890,&H000000FF,&H00101420,&H80000000,-1,0,0,0,100,100,0,0,1,2,1,8,40,40,22,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.00,0:03:00.00,TopBanner,,0,0,0,,{{\\fad(250,250)}}hlouis-sub-edit Phase 0 — libass + FFMS2 + wgpu
"
    );

    let phrases = [
        r"{\k35}Phụ đề {\kf50}Tiếng Việt {\k40}với {\kf60}đầy đủ dấu: ă â đ ê ô ơ ư ắ ệ ộ!",
        r"{\fad(150,150)}日本語字幕レンダリングテスト — Hiragana・Katakana・漢字",
        r"{\an2\c&H88EEFF&\t(0,800,\c&HFFFFFF&)}Frame-accurate seeking & smooth 23.976 fps playback",
        r"{\k30}Ka{\kf45}ra{\k30}o{\kf45}ke {\k35}test {\c&H90FF90&}overlay line",
    ];

    for step in 0..60 {
        let start_s = step * 3;
        let end_s = start_s + 3;
        let sh = start_s / 3600;
        let sm = (start_s / 60) % 60;
        let ss = start_s % 60;
        let eh = end_s / 3600;
        let em = (end_s / 60) % 60;
        let es = end_s % 60;
        let phrase = phrases[step % phrases.len()];
        let _ = writeln!(
            script,
            "Dialogue: 0,{sh}:{sm:02}:{ss:02}.00,{eh}:{em:02}:{es:02}.00,Default,,0,0,0,,{phrase}"
        );
    }

    script
}
