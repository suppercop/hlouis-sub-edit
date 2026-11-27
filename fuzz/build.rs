//! Generates initial seed corpora from existing plain-text fixtures and synthetic samples
//! into `$OUT_DIR/seeds/<target>/` (and optionally `fuzz/corpus/<target>/` when
//! `HLOUIS_WRITE_CORPUS=1`) so zero binary seed files need to be committed to git.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn write_seed(root: &Path, target: &str, name: &str, bytes: &[u8]) {
    let dir = root.join(target);
    if fs::create_dir_all(&dir).is_ok() {
        let _ = fs::write(dir.join(name), bytes);
    }
}

fn minimal_ebml_mkv_seed() -> Vec<u8> {
    // Minimal EBML header (0x1A45DFA3) + empty Segment (0x18538067) with Void element (0xEC).
    vec![
        0x1A, 0x45, 0xDF, 0xA3, 0x84, 0x42, 0x86, 0x81, 0x01, // EBML header
        0x18, 0x53, 0x80, 0x67, 0x84, 0xEC, 0x82, 0x00, 0x00, // Segment -> Void
    ]
}

fn populate_seeds(root: &Path) {
    let ass_sample = b"[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:03.50,Default,,0,0,0,,{\\pos(960,540)\\k50}Xin chao\n";
    let srt_sample = b"1\n00:00:01,000 --> 00:00:03,500\nHello <i>world</i>\n\n";

    write_seed(root, "fuzz_ass_core", "sample.ass", ass_sample);
    write_seed(root, "fuzz_ass_core", "sample.srt", srt_sample);

    write_seed(
        root,
        "fuzz_mkv_subs",
        "minimal.mkv",
        &minimal_ebml_mkv_seed(),
    );

    write_seed(
        root,
        "fuzz_keyframes",
        "kf.txt",
        b"# keyframe format v1\nfps 23.976\n0\n24\n120\n",
    );
    write_seed(
        root,
        "fuzz_keyframes",
        "tc_v2.txt",
        b"# timecode format v2\n0.000\n41.708\n83.417\n",
    );

    write_seed(
        root,
        "fuzz_aegisub_config",
        "config.json",
        br#"{"Audio":{"Lead":{"IN":200,"OUT":300}},"Subtitle":{"Grid":{"Hide Overrides":"\\u2605"}}}"#,
    );
    write_seed(
        root,
        "fuzz_aegisub_config",
        "hotkey.json",
        br#"{"Default":{"Ctrl-Z":["edit/undo"],"Ctrl-Y":["edit/redo"]}}"#,
    );

    write_seed(
        root,
        "fuzz_settings",
        "settings.json",
        br#"{"schema_version":1,"appearance":{"theme":"Dark","ui_scale":1.0}}"#,
    );

    write_seed(
        root,
        "fuzz_project",
        "garbage.ass",
        b"[Aegisub Project Garbage]\nAudio File: ?video\nVideo File: ep01.mkv\nVideo Position: 42\n",
    );

    write_seed(
        root,
        "fuzz_styles_catalog",
        "default.sty",
        b"[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n",
    );

    write_seed(
        root,
        "fuzz_karaskel",
        "kara.ass",
        b"[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nComment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,{\\pos($x,$y)}$syl\nDialogue: 0,0:00:01.00,0:00:04.00,Default,,0,0,0,karaoke,{\\k50}ka{\\k50}ra\n",
    );

    write_seed(
        root,
        "fuzz_syntax_highlight",
        "tags.txt",
        br"{\pos(960,540)\t(0,500,\fscx120)\clip(m 0 0 l 100 0 100 100)}Text\NLine2",
    );

    write_seed(
        root,
        "fuzz_visual_tools",
        "clip.txt",
        b"m 0 0 l 100 0 b 120 10 130 90 100 100 s 50 120 0 100 c",
    );

    write_seed(
        root,
        "fuzz_template_doctor",
        "tpl.ass",
        b"[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nComment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,code once,x = 1 + 2\nComment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,!retime(\"syl\",0,0)!{\\pos($x,$y)}\n",
    );
}

fn main() {
    println!("cargo:rerun-if-env-changed=HLOUIS_WRITE_CORPUS");
    if let Ok(out_dir) = env::var("OUT_DIR") {
        populate_seeds(&PathBuf::from(out_dir).join("seeds"));
    }
    if env::var("HLOUIS_WRITE_CORPUS").ok().as_deref() == Some("1") {
        if let Ok(manifest) = env::var("CARGO_MANIFEST_DIR") {
            populate_seeds(&PathBuf::from(manifest).join("corpus"));
        }
    }
}
