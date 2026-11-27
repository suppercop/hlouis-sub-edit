//! Criterion benchmark on a generated skip-heavy 40-track MKV (37 SRT tracks + large video blocks)
//! to verify zero-allocation block skipping throughput against the 1.5 GB < 3 s budget.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use mkv_subs::ebml::id;
use mkv_subs::extract_tracks;
use std::io::Cursor;

fn encode_id(element_id: u32, out: &mut Vec<u8>) {
    if element_id <= 0xFF {
        out.push(u8::try_from(element_id).expect("element_id <= 0xFF"));
    } else if element_id <= 0xFFFF {
        out.extend_from_slice(
            &u16::try_from(element_id)
                .expect("element_id <= 0xFFFF")
                .to_be_bytes(),
        );
    } else if element_id <= 0x00FF_FFFF {
        let b = element_id.to_be_bytes();
        out.extend_from_slice(&b[1..]);
    } else {
        out.extend_from_slice(&element_id.to_be_bytes());
    }
}

fn encode_vint(val: u64, out: &mut Vec<u8>) {
    if val < 0x7F {
        out.push(u8::try_from(val).expect("val < 0x7F") | 0x80);
    } else if val < 0x3FFF {
        let v = u16::try_from(val).expect("val < 0x3FFF") | 0x4000;
        out.extend_from_slice(&v.to_be_bytes());
    } else if val < 0x001F_FFFF {
        let v = u32::try_from(val).expect("val < 0x1F_FFFF") | 0x0020_0000;
        let b = v.to_be_bytes();
        out.extend_from_slice(&b[1..]);
    } else {
        let v = u32::try_from(val).expect("val fits 4-byte VINT") | 0x1000_0000;
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

fn elem_uint(element_id: u32, val: u64) -> Vec<u8> {
    let bytes = val.to_be_bytes();
    let first_nz = bytes.iter().position(|&b| b != 0).unwrap_or(7);
    elem(element_id, &bytes[first_nz..])
}

fn elem_str(element_id: u32, s: &str) -> Vec<u8> {
    elem(element_id, s.as_bytes())
}

fn build_block_payload(track_num: u64, rel_ts: i16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + data.len());
    encode_vint(track_num, &mut out);
    out.extend_from_slice(&rel_ts.to_be_bytes());
    out.push(0);
    out.extend_from_slice(data);
    out
}

/// Generates a ~16 MiB skip-heavy 40-track MKV (1 video, 2 audio, 37 SRT tracks) with
/// 99% of bytes in video/audio blocks that must be seek-skipped without allocation.
fn generate_skip_heavy_mkv() -> Vec<u8> {
    let mut tracks_body = Vec::new();
    for (num, ttype, codec) in [
        (1u64, 1u64, "V_MPEG4/ISO/AVC"),
        (2, 2, "A_OPUS"),
        (3, 2, "A_AAC"),
    ] {
        let mut te = elem_uint(id::TRACK_NUMBER, num);
        te.extend(elem_uint(id::TRACK_TYPE, ttype));
        te.extend(elem_str(id::CODEC_ID, codec));
        tracks_body.extend(elem(id::TRACK_ENTRY, &te));
    }
    for num in 4..=40u64 {
        let mut te = elem_uint(id::TRACK_NUMBER, num);
        te.extend(elem_uint(id::TRACK_TYPE, 17));
        te.extend(elem_str(id::CODEC_ID, "S_TEXT/UTF8"));
        tracks_body.extend(elem(id::TRACK_ENTRY, &te));
    }

    let mut seg_body = Vec::with_capacity(17 * 1024 * 1024);
    seg_body.extend(elem(id::INFO, &elem_uint(id::TIMESTAMP_SCALE, 1_000_000)));
    seg_body.extend(elem(id::TRACKS, &tracks_body));

    let video_frame = vec![0x55u8; 64 * 1024]; // 64 KiB video frame
    let audio_frame = vec![0x33u8; 2 * 1024]; // 2 KiB audio frame

    // 16 clusters × ~1 MiB each = ~16 MiB total
    for c_idx in 0..16u64 {
        let mut c_body = elem_uint(id::CLUSTER_TIMESTAMP, c_idx * 5000);
        for f in 0..15i16 {
            c_body.extend(elem(
                id::SIMPLE_BLOCK,
                &build_block_payload(1, f * 40, &video_frame),
            ));
            c_body.extend(elem(
                id::SIMPLE_BLOCK,
                &build_block_payload(2, f * 40, &audio_frame),
            ));
            c_body.extend(elem(
                id::SIMPLE_BLOCK,
                &build_block_payload(3, f * 40, &audio_frame),
            ));
        }
        // 1 subtitle block per cluster for each of the 37 SRT tracks
        for num in 4..=40u64 {
            let line = format!("Cluster {c_idx} subtitle line for track {num}");
            let mut bg = elem(id::BLOCK, &build_block_payload(num, 100, line.as_bytes()));
            bg.extend(elem_uint(id::BLOCK_DURATION, 2000));
            c_body.extend(elem(id::BLOCK_GROUP, &bg));
        }
        seg_body.extend(elem(id::CLUSTER, &c_body));
    }

    let mut file = Vec::with_capacity(seg_body.len() + 64);
    file.extend(elem(id::EBML, &elem_str(0x4282, "matroska")));
    file.extend(elem(id::SEGMENT, &seg_body));
    file
}

fn bench_mkv_subs(c: &mut Criterion) {
    let mkv = generate_skip_heavy_mkv();
    let all_37_subs: Vec<u64> = (4..=40).collect();

    let mut group = c.benchmark_group("mkv_skip_heavy_40_tracks");
    group.throughput(Throughput::Bytes(mkv.len() as u64));
    group.bench_function("extract_all_37_srt_tracks_16mb", |b| {
        b.iter(|| {
            let res = extract_tracks(
                Cursor::new(black_box(mkv.as_slice())),
                &all_37_subs,
                None,
                |_, _| {},
            );
            black_box(res).expect("extract")
        });
    });
    group.finish();
}

criterion_group!(benches, bench_mkv_subs);
criterion_main!(benches);
