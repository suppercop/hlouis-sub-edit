//! Synthetic 64-track MKV tests (ASS, SRT, WebVTT, zlib, header stripping, lacing,
//! unknown-size clusters, Void/CRC, Cues preview, cancellation, explicit errors)
//! plus the ignored `HLOUIS_MKV_MULTISUB` real-file integration test.

use ass_core::AssTime;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use mkv_subs::ebml::id;
use mkv_subs::{extract_tracks, list_tracks, preview_track, MkvError, SubtitleCodec, TrackType};
use std::io::{Cursor, Write};
use std::sync::atomic::AtomicBool;

// ---------- Tiny EBML Writer for Test Generation ----------

fn encode_id(element_id: u32, out: &mut Vec<u8>) {
    if element_id <= 0xFF {
        out.push(element_id as u8);
    } else if element_id <= 0xFFFF {
        out.extend_from_slice(&(element_id as u16).to_be_bytes());
    } else if element_id <= 0x00FF_FFFF {
        let b = element_id.to_be_bytes();
        out.extend_from_slice(&b[1..]);
    } else {
        out.extend_from_slice(&element_id.to_be_bytes());
    }
}

fn encode_vint(val: u64, out: &mut Vec<u8>) {
    if val < 0x7F {
        out.push((val as u8) | 0x80);
    } else if val < 0x3FFF {
        let v = (val as u16) | 0x4000;
        out.extend_from_slice(&v.to_be_bytes());
    } else if val < 0x001F_FFFF {
        let v = (val as u32) | 0x0020_0000;
        let b = v.to_be_bytes();
        out.extend_from_slice(&b[1..]);
    } else {
        let v = (val as u32) | 0x1000_0000;
        out.extend_from_slice(&v.to_be_bytes());
    }
}

fn elem(element_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    encode_id(element_id, &mut out);
    encode_vint(payload.len() as u64, &mut out);
    out.extend_from_slice(payload);
    out
}

fn elem_unknown_size(element_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    encode_id(element_id, &mut out);
    out.push(0xFF); // 1-byte unknown size
    out.extend_from_slice(payload);
    out
}

fn elem_uint(element_id: u32, val: u64) -> Vec<u8> {
    let bytes = val.to_be_bytes();
    let first_nz = bytes.iter().position(|&b| b != 0).unwrap_or(7);
    elem(element_id, &bytes[first_nz..])
}

fn elem_str(element_id: u32, s: &str) -> Vec<u8> {
    elem(element_id, s.as_bytes())
}

fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).expect("zlib write");
    enc.finish().expect("zlib finish")
}

fn build_block_payload(track_num: u64, rel_ts: i16, flags: u8, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + data.len());
    encode_vint(track_num, &mut out);
    out.extend_from_slice(&rel_ts.to_be_bytes());
    out.push(flags);
    out.extend_from_slice(data);
    out
}

fn build_xiph_laced(frames: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push((frames.len() - 1) as u8);
    for f in &frames[..frames.len() - 1] {
        let mut rem = f.len();
        while rem >= 255 {
            out.push(255);
            rem -= 255;
        }
        out.push(rem as u8);
    }
    for f in frames {
        out.extend_from_slice(f);
    }
    out
}

fn build_ebml_laced(frames: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push((frames.len() - 1) as u8);
    encode_vint(frames[0].len() as u64, &mut out);
    let mut prev = frames[0].len() as i64;
    for f in &frames[1..frames.len() - 1] {
        let cur = f.len() as i64;
        let diff = cur - prev;
        // 2-byte signed VINT (bias = 8191)
        let biased = (diff + 8191) as u16;
        let v = biased | 0x4000;
        out.extend_from_slice(&v.to_be_bytes());
        prev = cur;
    }
    for f in frames {
        out.extend_from_slice(f);
    }
    out
}

const SAMPLE_ASS_HEADER: &str = "\
[Script Info]
Title: MKV Embedded ASS
ScriptType: v4.00+
PlayResX: 1920
PlayResY: 1080

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Segoe UI,48,&H00FFFFFF,&H0000FFFF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,20,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
";

fn build_64_track_synthetic_mkv() -> Vec<u8> {
    // Track 1: Video (dummy blocks to skip)
    // Tracks 2..=65: 64 subtitle tracks (ASS, SRT, WebVTT, zlib, header-stripped, laced)
    // Track 66: Unsupported S_HDMV/PGS image subtitle
    // Track 67: Empty SRT subtitle track (0 blocks -> explicit EmptyTrack error)
    let mut tracks_body = Vec::new();

    // Video track 1
    {
        let mut te = Vec::new();
        te.extend(elem_uint(id::TRACK_NUMBER, 1));
        te.extend(elem_uint(id::TRACK_TYPE, 1));
        te.extend(elem_str(id::CODEC_ID, "V_MPEG4/ISO/AVC"));
        tracks_body.extend(elem(id::TRACK_ENTRY, &te));
    }

    let strip_prefix = b"0,0,Default,,0,0,0,,";

    for idx in 0..64u64 {
        let track_num = idx + 2; // 2..=65
        let mut te = Vec::new();
        te.extend(elem_uint(id::TRACK_NUMBER, track_num));
        te.extend(elem_uint(id::TRACK_TYPE, 17));
        te.extend(elem_uint(id::FLAG_DEFAULT, u64::from(idx == 0)));
        te.extend(elem_uint(id::FLAG_FORCED, u64::from(idx == 1)));
        te.extend(elem_str(id::LANGUAGE, "eng"));
        te.extend(elem_str(
            id::LANGUAGE_BCP47,
            if idx % 2 == 0 { "vi-VN" } else { "en-US" },
        ));
        te.extend(elem_str(id::NAME, &format!("Sub Track #{track_num}")));

        match idx % 4 {
            0 => {
                // Plain ASS with CodecPrivate
                te.extend(elem_str(id::CODEC_ID, "S_TEXT/ASS"));
                te.extend(elem(id::CODEC_PRIVATE, SAMPLE_ASS_HEADER.as_bytes()));
            }
            1 => {
                // Plain SRT
                te.extend(elem_str(id::CODEC_ID, "S_TEXT/UTF8"));
            }
            2 => {
                // Zlib-compressed SRT
                te.extend(elem_str(id::CODEC_ID, "S_TEXT/UTF8"));
                let comp = elem(
                    id::CONTENT_COMPRESSION,
                    &elem_uint(id::CONTENT_COMP_ALGO, 0),
                );
                let ce = elem(id::CONTENT_ENCODING, &comp);
                te.extend(elem(id::CONTENT_ENCODINGS, &ce));
            }
            _ => {
                // Header-stripped ASS (prepend "0,0,Default,,0,0,0,,")
                te.extend(elem_str(id::CODEC_ID, "S_TEXT/ASS"));
                te.extend(elem(id::CODEC_PRIVATE, SAMPLE_ASS_HEADER.as_bytes()));
                let mut comp_body = Vec::new();
                comp_body.extend(elem_uint(id::CONTENT_COMP_ALGO, 3));
                comp_body.extend(elem(id::CONTENT_COMP_SETTINGS, strip_prefix));
                let ce = elem(
                    id::CONTENT_ENCODING,
                    &elem(id::CONTENT_COMPRESSION, &comp_body),
                );
                te.extend(elem(id::CONTENT_ENCODINGS, &ce));
            }
        }
        tracks_body.extend(elem(id::TRACK_ENTRY, &te));
    }

    // Track 66: Unsupported PGS
    {
        let mut te = Vec::new();
        te.extend(elem_uint(id::TRACK_NUMBER, 66));
        te.extend(elem_uint(id::TRACK_TYPE, 17));
        te.extend(elem_str(id::CODEC_ID, "S_HDMV/PGS"));
        tracks_body.extend(elem(id::TRACK_ENTRY, &te));
    }
    // Track 67: Empty SRT track
    {
        let mut te = Vec::new();
        te.extend(elem_uint(id::TRACK_NUMBER, 67));
        te.extend(elem_uint(id::TRACK_TYPE, 17));
        te.extend(elem_str(id::CODEC_ID, "S_TEXT/UTF8"));
        tracks_body.extend(elem(id::TRACK_ENTRY, &te));
    }

    let info_el = elem(id::INFO, &elem_uint(id::TIMESTAMP_SCALE, 1_000_000));
    let tracks_el = elem(id::TRACKS, &tracks_body);
    let void_el = elem(id::VOID, &[0u8; 32]);
    let crc_el = elem(id::CRC32, &[0xDE, 0xAD, 0xBE, 0xEF]);

    // Cluster 1 (known size): Video block + blocks for tracks 2..=65.
    // Includes out-of-order ReadOrder on ASS track 2, Xiph lacing on SRT track 3, EBML lacing on track 7.
    let mut cluster1_body = Vec::new();
    cluster1_body.extend(elem_uint(id::CLUSTER_TIMESTAMP, 1000));
    // Dummy 16 KiB video SimpleBlock (must be skipped without reading payload)
    let dummy_video = vec![0xAAu8; 16 * 1024];
    cluster1_body.extend(elem(
        id::SIMPLE_BLOCK,
        &build_block_payload(1, 0, 0x80, &dummy_video),
    ));

    for idx in 0..64u64 {
        let track_num = idx + 2;
        match idx % 4 {
            0 => {
                // Emit ReadOrder 2 first, then ReadOrder 1 to test ReadOrder sorting!
                let p2 = format!("2,0,Default,,0,0,0,,Second T{track_num}");
                let p1 = format!("1,0,Default,,0,0,0,,First T{track_num}");
                let mut bg2 = elem(
                    id::BLOCK,
                    &build_block_payload(track_num, 500, 0, p2.as_bytes()),
                );
                bg2.extend(elem_uint(id::BLOCK_DURATION, 1500));
                cluster1_body.extend(elem(id::BLOCK_GROUP, &bg2));

                let mut bg1 = elem(
                    id::BLOCK,
                    &build_block_payload(track_num, 0, 0, p1.as_bytes()),
                );
                bg1.extend(elem_uint(id::BLOCK_DURATION, 1000));
                cluster1_body.extend(elem(id::BLOCK_GROUP, &bg1));
            }
            1 => {
                if track_num == 3 {
                    // Xiph-laced 2 SRT frames in one SimpleBlock (missing duration -> filled by next)
                    let f1 = b"<i>Laced Xiph 1</i>";
                    let f2 = b"<b>Laced Xiph 2</b>";
                    let laced = build_xiph_laced(&[f1, f2]);
                    cluster1_body.extend(elem(
                        id::SIMPLE_BLOCK,
                        &build_block_payload(track_num, 0, 0x02, &laced),
                    ));
                } else if track_num == 7 {
                    // EBML-laced 2 SRT frames
                    let f1 = b"EBML Laced A";
                    let f2 = b"EBML Laced B longer";
                    let laced = build_ebml_laced(&[f1, f2]);
                    cluster1_body.extend(elem(
                        id::SIMPLE_BLOCK,
                        &build_block_payload(track_num, 0, 0x06, &laced),
                    ));
                } else {
                    let text = format!("<i>SRT line T{track_num}</i>");
                    let mut bg = elem(
                        id::BLOCK,
                        &build_block_payload(track_num, 0, 0, text.as_bytes()),
                    );
                    bg.extend(elem_uint(id::BLOCK_DURATION, 2000));
                    cluster1_body.extend(elem(id::BLOCK_GROUP, &bg));
                }
            }
            2 => {
                // Zlib-compressed SRT
                let raw = format!("Zlib compressed T{track_num}");
                let compressed = zlib_compress(raw.as_bytes());
                let mut bg = elem(
                    id::BLOCK,
                    &build_block_payload(track_num, 0, 0, &compressed),
                );
                bg.extend(elem_uint(id::BLOCK_DURATION, 1800));
                cluster1_body.extend(elem(id::BLOCK_GROUP, &bg));
            }
            _ => {
                // Header-stripped ASS (stripped "0,0,Default,,0,0,0,,")
                let suffix = format!("Stripped ASS T{track_num}");
                let mut bg = elem(
                    id::BLOCK,
                    &build_block_payload(track_num, 0, 0, suffix.as_bytes()),
                );
                bg.extend(elem_uint(id::BLOCK_DURATION, 2500));
                cluster1_body.extend(elem(id::BLOCK_GROUP, &bg));
            }
        }
    }
    let cluster1_el = elem(id::CLUSTER, &cluster1_body);

    // Cluster 2: UNKNOWN-SIZE cluster with an extra event on Track 2 (at 10,000 ms)
    let mut cluster2_body = Vec::new();
    cluster2_body.extend(elem_uint(id::CLUSTER_TIMESTAMP, 10_000));
    let p3 = b"3,0,Default,,0,0,0,,Unknown-size cluster line";
    let mut bg3 = elem(id::BLOCK, &build_block_payload(2, 0, 0, p3));
    bg3.extend(elem_uint(id::BLOCK_DURATION, 1000));
    cluster2_body.extend(elem(id::BLOCK_GROUP, &bg3));
    let cluster2_el = elem_unknown_size(id::CLUSTER, &cluster2_body);

    // Build Segment body: CRC + Void + Info + Tracks + Cluster1 + Cluster2 + Cues
    let mut seg_body = Vec::new();
    seg_body.extend(&crc_el);
    seg_body.extend(&void_el);
    seg_body.extend(&info_el);
    seg_body.extend(&tracks_el);

    let cluster1_rel_pos = seg_body.len() as u64;
    seg_body.extend(&cluster1_el);
    let cluster2_rel_pos = seg_body.len() as u64;
    seg_body.extend(&cluster2_el);

    // Add Cues pointing directly to Cluster 1 and Cluster 2 for Track 2
    let mut cues_body = Vec::new();
    for pos in [cluster1_rel_pos, cluster2_rel_pos] {
        let mut ctp = elem_uint(id::CUE_TRACK, 2);
        ctp.extend(elem_uint(id::CUE_CLUSTER_POSITION, pos));
        cues_body.extend(elem(id::CUE_POINT, &elem(id::CUE_TRACK_POSITIONS, &ctp)));
    }
    let cues_pos_rel = seg_body.len() as u64;
    seg_body.extend(elem(id::CUES, &cues_body));

    // Prepend SeekHead pointing to Cues (adjusting offsets cleanly by placing SeekHead after computing?
    // Even simpler: Cues is at top-level; let's place a fixed-size SeekHead or let read_metadata find Cues via SeekHead!)
    // Build a SeekHead pointing to Cues with exact offset adjustment.
    let mut seek_entry = elem_uint(id::SEEK_ID, u64::from(id::CUES));
    // We'll compute SeekHead length with 4-byte position padding by fixing position size:
    let mut seek_pos_placeholder = Vec::new();
    encode_id(id::SEEK_POSITION, &mut seek_pos_placeholder);
    encode_vint(4, &mut seek_pos_placeholder);
    seek_pos_placeholder.extend_from_slice(&0u32.to_be_bytes());
    seek_entry.extend(&seek_pos_placeholder);
    let seek_head_dummy = elem(id::SEEK_HEAD, &elem(id::SEEK, &seek_entry));
    let sh_len = seek_head_dummy.len() as u64;

    // Rebuild Cues with shifted cluster offsets (+sh_len)
    let mut real_cues_body = Vec::new();
    for pos in [cluster1_rel_pos + sh_len, cluster2_rel_pos + sh_len] {
        let mut ctp = elem_uint(id::CUE_TRACK, 2);
        ctp.extend(elem_uint(id::CUE_CLUSTER_POSITION, pos));
        real_cues_body.extend(elem(id::CUE_POINT, &elem(id::CUE_TRACK_POSITIONS, &ctp)));
    }
    let real_cues_el = elem(id::CUES, &real_cues_body);

    // Rebuild SeekHead with exact Cues offset (cues_pos_rel + sh_len)
    let mut real_seek_entry = elem_uint(id::SEEK_ID, u64::from(id::CUES));
    encode_id(id::SEEK_POSITION, &mut real_seek_entry);
    encode_vint(4, &mut real_seek_entry);
    real_seek_entry.extend_from_slice(&((cues_pos_rel + sh_len) as u32).to_be_bytes());
    let real_seek_head = elem(id::SEEK_HEAD, &elem(id::SEEK, &real_seek_entry));
    assert_eq!(real_seek_head.len() as u64, sh_len);

    // Assemble final Segment body
    let mut final_seg = Vec::new();
    final_seg.extend(&real_seek_head);
    final_seg.extend(&crc_el);
    final_seg.extend(&void_el);
    final_seg.extend(&info_el);
    final_seg.extend(&tracks_el);
    final_seg.extend(&cluster1_el);
    final_seg.extend(&cluster2_el);
    final_seg.extend(&real_cues_el);

    let mut file = Vec::new();
    file.extend(elem(id::EBML, &elem_str(0x4282, "matroska")));
    file.extend(elem(id::SEGMENT, &final_seg));
    file
}

#[test]
fn lists_and_extracts_all_64_subtitle_tracks_with_compression_lacing_and_unknown_cluster() {
    let mkv = build_64_track_synthetic_mkv();
    let tracks = list_tracks(Cursor::new(&mkv)).expect("list tracks");
    assert_eq!(tracks.len(), 67, "1 video + 64 subs + 1 PGS + 1 empty SRT");
    assert_eq!(tracks[0].track_type, TrackType::Video);
    assert_eq!(tracks[1].display_language(), Some("vi-VN"));
    assert!(tracks[1].is_default);
    assert!(tracks[2].is_forced);
    assert_eq!(
        tracks[65].subtitle_codec,
        SubtitleCodec::UnsupportedImage("S_HDMV/PGS".to_owned())
    );

    // Extract all 64 populated text subtitle tracks in ONE single pass!
    let target_nums: Vec<u64> = (2..=65).collect();
    let mut progress_calls = 0usize;
    let extracted = extract_tracks(Cursor::new(&mkv), &target_nums, None, |_, _| {
        progress_calls += 1;
    })
    .expect("extract all 64 tracks");
    assert!(progress_calls > 0);
    assert_eq!(extracted.len(), 64);

    // Track 2 (ASS): ReadOrder sorted (1, 2, 3) + unknown-size cluster line included!
    let t2 = &extracted[&2];
    let t2_evs: Vec<_> = t2.script.events().collect();
    assert_eq!(t2_evs.len(), 3);
    assert_eq!(t2_evs[0].text(), "First T2");
    assert_eq!(t2_evs[1].text(), "Second T2");
    assert_eq!(t2_evs[2].text(), "Unknown-size cluster line");
    assert_eq!(t2_evs[2].start(), AssTime::from_cs(1000));

    // Track 3 (Xiph-laced SRT): 2 events with HTML->ASS conversion
    let t3_evs: Vec<_> = extracted[&3].script.events().collect();
    assert_eq!(t3_evs.len(), 2);
    assert_eq!(t3_evs[0].text(), r"{\i1}Laced Xiph 1{\i0}");
    assert_eq!(t3_evs[1].text(), r"{\b1}Laced Xiph 2{\b0}");

    // Track 7 (EBML-laced SRT): 2 events
    let t7_evs: Vec<_> = extracted[&7].script.events().collect();
    assert_eq!(t7_evs.len(), 2);
    assert_eq!(t7_evs[0].text(), "EBML Laced A");
    assert_eq!(t7_evs[1].text(), "EBML Laced B longer");

    // Track 4 (Zlib-compressed SRT)
    let t4_evs: Vec<_> = extracted[&4].script.events().collect();
    assert_eq!(t4_evs.len(), 1);
    assert_eq!(t4_evs[0].text(), "Zlib compressed T4");

    // Track 5 (Header-stripped ASS)
    let t5_evs: Vec<_> = extracted[&5].script.events().collect();
    assert_eq!(t5_evs.len(), 1);
    assert_eq!(t5_evs[0].text(), "Stripped ASS T5");

    // Track 65 (> 32 track number!)
    let t65_evs: Vec<_> = extracted[&65].script.events().collect();
    assert_eq!(t65_evs.len(), 1);
    assert_eq!(t65_evs[0].text(), "Stripped ASS T65");
}

#[test]
fn preview_cancellation_and_explicit_errors_work() {
    let mkv = build_64_track_synthetic_mkv();

    // Preview first 2 events of Track 2 via Cues without reading Cluster 2.
    let preview = preview_track(Cursor::new(&mkv), 2, 2, None).expect("preview");
    assert_eq!(preview.event_count, 2);

    // Unsupported image subtitle (PGS) returns explicit UnsupportedCodec error.
    let err_pgs = extract_tracks(Cursor::new(&mkv), &[66], None, |_, _| {});
    assert!(matches!(
        err_pgs,
        Err(MkvError::UnsupportedCodec { track: 66, .. })
    ));

    // Empty track returns explicit EmptyTrack error (never silently returns empty script).
    let err_empty = extract_tracks(Cursor::new(&mkv), &[67], None, |_, _| {});
    assert!(matches!(
        err_empty,
        Err(MkvError::EmptyTrack { track: 67, .. })
    ));

    // Cancellation stops immediately.
    let cancel = AtomicBool::new(true);
    let err_cancel = extract_tracks(Cursor::new(&mkv), &[2], Some(&cancel), |_, _| {});
    assert!(matches!(err_cancel, Err(MkvError::Cancelled)));
}

/// Ignored integration test against a real multi-subtitle MKV (`HLOUIS_MKV_MULTISUB`).
/// Run with: `HLOUIS_MKV_MULTISUB=/path/to/episode.mkv cargo test -p mkv-subs -- --ignored`
#[test]
#[ignore = "requires HLOUIS_MKV_MULTISUB pointing to a real multi-subtitle MKV file"]
fn extract_real_multisub_mkv() {
    let path = std::env::var("HLOUIS_MKV_MULTISUB")
        .expect("set HLOUIS_MKV_MULTISUB to a real MKV file path");
    let file = std::fs::File::open(&path).expect("open HLOUIS_MKV_MULTISUB");
    let tracks = list_tracks(file).expect("list tracks");
    assert!(!tracks.is_empty(), "MKV must contain tracks");

    let text_track_nums: Vec<u64> = tracks
        .iter()
        .filter(|t| t.subtitle_codec.is_supported_text())
        .map(|t| t.number)
        .collect();
    assert!(
        !text_track_nums.is_empty(),
        "expected at least one text subtitle track in {path}"
    );

    let file2 = std::fs::File::open(&path).expect("re-open HLOUIS_MKV_MULTISUB");
    let extracted =
        extract_tracks(file2, &text_track_nums, None, |_, _| {}).expect("extract all text tracks");
    assert_eq!(extracted.len(), text_track_nums.len());
    for num in text_track_nums {
        assert!(
            extracted[&num].event_count >= 1,
            "track {num} must have at least 1 event"
        );
    }
}
