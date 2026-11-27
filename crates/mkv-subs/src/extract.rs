//! Single-pass Matroska subtitle extractor, lacing/compression decoder, ASS/SRT/WebVTT converter,
//! and Cues-accelerated preview API.

use crate::ebml::{id, is_segment_top_level_id, EbmlReader};
use crate::tracks::{
    read_metadata, CompressionAlgo, ContentEncodingInfo, MkvMetadata, SubtitleCodec, TrackInfo,
};
use crate::MkvError;
use ass_core::{parse_ass_bytes, parse_srt, AssEvent, AssTime, EventEntry, EventKind, ScriptFile};
use flate2::read::ZlibDecoder;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicBool, Ordering};

/// Default fallback duration (2 seconds = 200 cs) for the final subtitle block when `BlockDuration` is absent.
const FALLBACK_LAST_DURATION_CS: u32 = 200;

/// Extracted subtitle track converted to an `ass_core::ScriptFile`.
#[derive(Debug, Clone)]
pub struct ExtractedTrack {
    pub track: TrackInfo,
    pub script: ScriptFile,
    pub event_count: usize,
    /// Non-fatal problems found while converting the track (malformed blocks, invalid UTF-8,
    /// unparseable header fields...). The UI must show these; they are never silently dropped.
    pub warnings: Vec<String>,
}

#[derive(Debug)]
struct RawSubPacket {
    start_ms: u64,
    duration_ms: Option<u64>,
    payload: Vec<u8>,
}

/// Extracts one or more selected subtitle tracks in a single streaming pass.
///
/// Non-selected blocks (video, audio, other subtitles) are skipped by `Seek` after reading only
/// their track-number VINT, with zero payload allocation.
pub fn extract_tracks<R: Read + Seek>(
    reader: R,
    track_numbers: &[u64],
    cancel: Option<&AtomicBool>,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<HashMap<u64, ExtractedTrack>, MkvError> {
    let mut ebml = EbmlReader::new(reader)?;
    let meta = read_metadata(&mut ebml, false)?;
    let selected = validate_selected_tracks(&meta, track_numbers)?;
    let selected_set: HashSet<u64> = selected.keys().copied().collect();

    let mut packets: HashMap<u64, Vec<RawSubPacket>> =
        selected_set.iter().map(|&t| (t, Vec::new())).collect();

    let scan_start = meta.first_cluster_pos.unwrap_or(meta.segment_data_start);
    ebml.seek_to(scan_start)?;
    let total_len = ebml.len();

    while let Some(hdr) = ebml.read_header(meta.segment_end)? {
        if is_cancelled(cancel) {
            return Err(MkvError::Cancelled);
        }
        on_progress(ebml.pos(), total_len);

        if hdr.id == id::CLUSTER {
            scan_cluster(
                &mut ebml,
                hdr.end_pos().map(|e| e.min(meta.segment_end)),
                meta.segment_end,
                meta.timestamp_scale_ns,
                &selected_set,
                &selected,
                &mut packets,
                None,
                cancel,
            )?;
        } else if let Some(size) = hdr.size {
            ebml.skip(size)?;
        } else {
            break;
        }
    }

    on_progress(total_len, total_len);
    build_extracted_scripts(&selected, packets)
}

/// Extracts at most the first `max_events` events of a single subtitle track.
///
/// Uses `Cues` to jump directly to cued clusters when the track is indexed in `Cues`, and stops
/// early as soon as `max_events` packets have been collected without scanning the entire file.
pub fn preview_track<R: Read + Seek>(
    reader: R,
    track_number: u64,
    max_events: usize,
    cancel: Option<&AtomicBool>,
) -> Result<ExtractedTrack, MkvError> {
    let mut ebml = EbmlReader::new(reader)?;
    let meta = read_metadata(&mut ebml, true)?;
    let selected = validate_selected_tracks(&meta, &[track_number])?;
    let selected_set = HashSet::from([track_number]);
    let mut packets: HashMap<u64, Vec<RawSubPacket>> = HashMap::from([(track_number, Vec::new())]);
    let limit = max_events.max(1);

    if let Some(cued_clusters) = meta.cue_clusters_by_track.get(&track_number) {
        for &cluster_pos in cued_clusters {
            if is_cancelled(cancel) {
                return Err(MkvError::Cancelled);
            }
            if packets.get(&track_number).is_some_and(|v| v.len() >= limit) {
                break;
            }
            if cluster_pos >= meta.segment_end {
                continue;
            }
            ebml.seek_to(cluster_pos)?;
            if let Some(hdr) = ebml.read_header(meta.segment_end)? {
                if hdr.id == id::CLUSTER {
                    scan_cluster(
                        &mut ebml,
                        hdr.end_pos().map(|e| e.min(meta.segment_end)),
                        meta.segment_end,
                        meta.timestamp_scale_ns,
                        &selected_set,
                        &selected,
                        &mut packets,
                        Some(limit),
                        cancel,
                    )?;
                }
            }
        }
    } else {
        let scan_start = meta.first_cluster_pos.unwrap_or(meta.segment_data_start);
        ebml.seek_to(scan_start)?;
        while let Some(hdr) = ebml.read_header(meta.segment_end)? {
            if is_cancelled(cancel) {
                return Err(MkvError::Cancelled);
            }
            if packets.get(&track_number).is_some_and(|v| v.len() >= limit) {
                break;
            }
            if hdr.id == id::CLUSTER {
                scan_cluster(
                    &mut ebml,
                    hdr.end_pos().map(|e| e.min(meta.segment_end)),
                    meta.segment_end,
                    meta.timestamp_scale_ns,
                    &selected_set,
                    &selected,
                    &mut packets,
                    Some(limit),
                    cancel,
                )?;
            } else if let Some(size) = hdr.size {
                ebml.skip(size)?;
            } else {
                break;
            }
        }
    }

    if let Some(list) = packets.get_mut(&track_number) {
        list.truncate(limit);
    }

    let mut map = build_extracted_scripts(&selected, packets)?;
    map.remove(&track_number)
        .ok_or(MkvError::TrackNotFound(track_number))
}

fn validate_selected_tracks(
    meta: &MkvMetadata,
    track_numbers: &[u64],
) -> Result<HashMap<u64, TrackInfo>, MkvError> {
    let mut by_num: HashMap<u64, &TrackInfo> = HashMap::with_capacity(meta.tracks.len());
    for t in &meta.tracks {
        by_num.insert(t.number, t);
    }

    let mut selected = HashMap::with_capacity(track_numbers.len());
    for &num in track_numbers {
        let info = by_num.get(&num).ok_or(MkvError::TrackNotFound(num))?;
        if !info.subtitle_codec.is_supported_text() {
            return Err(MkvError::UnsupportedCodec {
                track: num,
                codec: info.codec_id.clone(),
            });
        }
        selected.insert(num, (*info).clone());
    }
    Ok(selected)
}

#[allow(clippy::too_many_arguments)]
fn scan_cluster<R: Read + Seek>(
    reader: &mut EbmlReader<R>,
    cluster_end_opt: Option<u64>,
    segment_end: u64,
    timestamp_scale_ns: u64,
    selected_set: &HashSet<u64>,
    selected_info: &HashMap<u64, TrackInfo>,
    packets: &mut HashMap<u64, Vec<RawSubPacket>>,
    early_stop_limit: Option<usize>,
    cancel: Option<&AtomicBool>,
) -> Result<(), MkvError> {
    let limit = cluster_end_opt.unwrap_or(segment_end);
    let mut cluster_ts: u64 = 0;

    while let Some(hdr) = reader.read_header(limit)? {
        if is_cancelled(cancel) {
            return Err(MkvError::Cancelled);
        }

        // Unknown-size cluster ends as soon as we encounter another top-level Segment element.
        if cluster_end_opt.is_none() && is_segment_top_level_id(hdr.id) {
            reader.seek_to(hdr.header_pos)?;
            break;
        }

        match hdr.id {
            id::CLUSTER_TIMESTAMP => {
                if let Some(sz) = hdr.size {
                    cluster_ts = reader.read_uint(sz)?;
                }
            }
            id::SIMPLE_BLOCK => {
                let Some(sz) = hdr.size else {
                    break;
                };
                read_or_skip_block(
                    reader,
                    sz,
                    cluster_ts,
                    None,
                    timestamp_scale_ns,
                    selected_set,
                    selected_info,
                    packets,
                )?;
            }
            id::BLOCK_GROUP => {
                let bg_end = hdr.end_pos().map_or(limit, |e| e.min(limit));
                parse_block_group(
                    reader,
                    bg_end,
                    cluster_ts,
                    timestamp_scale_ns,
                    selected_set,
                    selected_info,
                    packets,
                )?;
                reader.seek_to(bg_end)?;
            }
            _ => {
                if let Some(sz) = hdr.size {
                    reader.skip(sz)?;
                } else {
                    break;
                }
            }
        }

        if let Some(max_ev) = early_stop_limit {
            if selected_set
                .iter()
                .all(|t| packets.get(t).is_some_and(|v| v.len() >= max_ev))
            {
                break;
            }
        }
    }

    Ok(())
}

fn parse_block_group<R: Read + Seek>(
    reader: &mut EbmlReader<R>,
    bg_end: u64,
    cluster_ts: u64,
    timestamp_scale_ns: u64,
    selected_set: &HashSet<u64>,
    selected_info: &HashMap<u64, TrackInfo>,
    packets: &mut HashMap<u64, Vec<RawSubPacket>>,
) -> Result<(), MkvError> {
    let mut block_span: Option<(u64, u64)> = None; // (data_pos, size)
    let mut block_duration_ticks: Option<u64> = None;

    while let Some(hdr) = reader.read_header(bg_end)? {
        let elem_end = hdr.end_pos().map_or(bg_end, |e| e.min(bg_end));
        match hdr.id {
            id::BLOCK => {
                if let Some(sz) = hdr.size {
                    // Peek track number immediately to skip unselected BlockGroups without storing.
                    let (track_num, consumed) = reader.read_vint_uint()?;
                    if selected_set.contains(&track_num) {
                        block_span = Some((hdr.data_pos, sz));
                    }
                    if let Some(rem) = sz.checked_sub(consumed) {
                        reader.skip(rem)?;
                    }
                }
            }
            id::BLOCK_DURATION => {
                if let Some(sz) = hdr.size {
                    block_duration_ticks = Some(reader.read_uint(sz)?);
                }
            }
            _ => {}
        }
        reader.seek_to(elem_end)?;
    }

    if let Some((data_pos, sz)) = block_span {
        let duration_ms = block_duration_ticks.map(|ticks| ticks_to_ms(ticks, timestamp_scale_ns));
        reader.seek_to(data_pos)?;
        read_or_skip_block(
            reader,
            sz,
            cluster_ts,
            duration_ms,
            timestamp_scale_ns,
            selected_set,
            selected_info,
            packets,
        )?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn read_or_skip_block<R: Read + Seek>(
    reader: &mut EbmlReader<R>,
    block_size: u64,
    cluster_ts: u64,
    duration_ms: Option<u64>,
    timestamp_scale_ns: u64,
    selected_set: &HashSet<u64>,
    selected_info: &HashMap<u64, TrackInfo>,
    packets: &mut HashMap<u64, Vec<RawSubPacket>>,
) -> Result<(), MkvError> {
    if block_size < 4 {
        reader.skip(block_size)?;
        return Ok(());
    }
    let (track_num, vint_len) = reader.read_vint_uint()?;
    let Some(remaining) = block_size.checked_sub(vint_len) else {
        return Err(MkvError::MalformedEbml(
            "block shorter than track VINT".to_owned(),
        ));
    };

    // Fast path: unselected video/audio/subtitle track -> seek past payload with ZERO allocation.
    if !selected_set.contains(&track_num) {
        reader.skip(remaining)?;
        return Ok(());
    }

    if remaining < 3 {
        reader.skip(remaining)?;
        return Ok(());
    }

    let mut hdr_bytes = [0u8; 3];
    reader.read_exact(&mut hdr_bytes)?;
    let rel_ts = i16::from_be_bytes([hdr_bytes[0], hdr_bytes[1]]);
    let flags = hdr_bytes[2];
    let lacing = (flags >> 1) & 0x03;

    let payload_len = remaining - 3;
    let raw_payload = reader.read_bytes(payload_len)?;

    let abs_ticks = if rel_ts >= 0 {
        cluster_ts.saturating_add(u64::from(rel_ts.cast_unsigned()))
    } else {
        cluster_ts.saturating_sub(u64::from(rel_ts.unsigned_abs()))
    };
    let start_ms = ticks_to_ms(abs_ticks, timestamp_scale_ns);

    let frames = unlace_frames(&raw_payload, lacing)?;
    let encodings = selected_info
        .get(&track_num)
        .map_or(&[][..], |t| t.encodings.as_slice());

    if let Some(list) = packets.get_mut(&track_num) {
        for frame in frames {
            let decoded = apply_content_encodings(track_num, frame, encodings)?;
            list.push(RawSubPacket {
                start_ms,
                duration_ms,
                payload: decoded,
            });
        }
    }

    Ok(())
}

fn unlace_frames(data: &[u8], lacing: u8) -> Result<Vec<&[u8]>, MkvError> {
    if lacing == 0 {
        return Ok(vec![data]);
    }
    if data.is_empty() {
        return Err(MkvError::MalformedEbml(
            "laced block has empty payload".to_owned(),
        ));
    }
    let frame_count = usize::from(data[0]) + 1;
    let mut pos = 1usize;

    match lacing {
        // 1: Xiph lacing
        1 => {
            let mut sizes = Vec::with_capacity(frame_count);
            for _ in 0..(frame_count - 1) {
                let mut sz = 0usize;
                loop {
                    let &b = data.get(pos).ok_or_else(|| {
                        MkvError::MalformedEbml("truncated Xiph lace size".to_owned())
                    })?;
                    pos += 1;
                    sz = sz.saturating_add(usize::from(b));
                    if b != 255 {
                        break;
                    }
                }
                sizes.push(sz);
            }
            slice_laced_frames(data, pos, &sizes)
        }
        // 2: Fixed-size lacing
        2 => {
            let rem = data.len() - pos;
            if !rem.is_multiple_of(frame_count) {
                return Err(MkvError::MalformedEbml(
                    "fixed-size laced block payload not divisible by frame count".to_owned(),
                ));
            }
            let each = rem / frame_count;
            let mut out = Vec::with_capacity(frame_count);
            for i in 0..frame_count {
                let s = pos + i * each;
                out.push(&data[s..s + each]);
            }
            Ok(out)
        }
        // 3: EBML lacing
        3 => {
            let mut sizes = Vec::with_capacity(frame_count);
            let (first_sz, used) = read_slice_vint_uint(&data[pos..])?;
            pos += used;
            let mut prev = i64::try_from(first_sz)
                .map_err(|_| MkvError::MalformedEbml("EBML lace size overflow".to_owned()))?;
            sizes.push(usize::try_from(first_sz).unwrap_or(0));

            for _ in 1..(frame_count - 1) {
                let (delta, used_d) = read_slice_vint_sint(&data[pos..])?;
                pos += used_d;
                prev = prev.checked_add(delta).ok_or_else(|| {
                    MkvError::MalformedEbml("EBML lace signed delta overflow".to_owned())
                })?;
                let sz = usize::try_from(prev).map_err(|_| {
                    MkvError::MalformedEbml("negative EBML lace frame size".to_owned())
                })?;
                sizes.push(sz);
            }
            slice_laced_frames(data, pos, &sizes)
        }
        _ => Ok(vec![data]),
    }
}

fn slice_laced_frames<'a>(
    data: &'a [u8],
    mut pos: usize,
    leading_sizes: &[usize],
) -> Result<Vec<&'a [u8]>, MkvError> {
    let mut out = Vec::with_capacity(leading_sizes.len() + 1);
    for &sz in leading_sizes {
        let end = pos
            .checked_add(sz)
            .ok_or_else(|| MkvError::MalformedEbml("lace size overflow".to_owned()))?;
        if end > data.len() {
            return Err(MkvError::MalformedEbml(
                "laced frame exceeds block boundary".to_owned(),
            ));
        }
        out.push(&data[pos..end]);
        pos = end;
    }
    if pos > data.len() {
        return Err(MkvError::MalformedEbml(
            "last laced frame exceeds block boundary".to_owned(),
        ));
    }
    out.push(&data[pos..]);
    Ok(out)
}

fn read_slice_vint_uint(buf: &[u8]) -> Result<(u64, usize), MkvError> {
    let &first = buf
        .first()
        .ok_or_else(|| MkvError::MalformedEbml("truncated EBML lace VINT".to_owned()))?;
    let lz = first.leading_zeros() as usize;
    if lz > 7 || 1 + lz > buf.len() {
        return Err(MkvError::MalformedEbml("invalid EBML lace VINT".to_owned()));
    }
    let mask = if lz == 7 { 0u8 } else { (1u8 << (7 - lz)) - 1 };
    let mut val = u64::from(first & mask);
    for &b in &buf[1..=lz] {
        val = (val << 8) | u64::from(b);
    }
    Ok((val, 1 + lz))
}

fn read_slice_vint_sint(buf: &[u8]) -> Result<(i64, usize), MkvError> {
    let (raw, len) = read_slice_vint_uint(buf)?;
    let bias = (1i64 << (7 * len - 1)) - 1;
    let signed = i64::try_from(raw)
        .map_err(|_| MkvError::MalformedEbml("EBML lace sint overflow".to_owned()))?
        - bias;
    Ok((signed, len))
}

fn apply_content_encodings(
    track: u64,
    frame: &[u8],
    encodings: &[ContentEncodingInfo],
) -> Result<Vec<u8>, MkvError> {
    if encodings.is_empty() {
        return Ok(frame.to_vec());
    }
    let mut cur = frame.to_vec();
    // Reverse ContentEncodingOrder when decoding.
    for enc in encodings.iter().rev() {
        if enc.scope & 1 == 0 {
            continue;
        }
        match &enc.algo {
            CompressionAlgo::Zlib => {
                let mut decoder = ZlibDecoder::new(cur.as_slice());
                let mut decompressed = Vec::new();
                decoder
                    .read_to_end(&mut decompressed)
                    .map_err(|source| MkvError::Decompress { track, source })?;
                cur = decompressed;
            }
            CompressionAlgo::HeaderStripping(prefix) => {
                let mut restored = Vec::with_capacity(prefix.len() + cur.len());
                restored.extend_from_slice(prefix);
                restored.extend_from_slice(&cur);
                cur = restored;
            }
            CompressionAlgo::Unsupported(algo) => {
                return Err(MkvError::InvalidFormat(format!(
                    "track {track} uses unsupported ContentCompAlgo {algo}"
                )));
            }
        }
    }
    Ok(cur)
}

fn ticks_to_ms(ticks: u64, timestamp_scale_ns: u64) -> u64 {
    let ns = u128::from(ticks).saturating_mul(u128::from(timestamp_scale_ns));
    let ms = ns.saturating_add(500_000) / 1_000_000;
    u64::try_from(ms).unwrap_or(u64::MAX)
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}

fn build_extracted_scripts(
    selected: &HashMap<u64, TrackInfo>,
    mut packets: HashMap<u64, Vec<RawSubPacket>>,
) -> Result<HashMap<u64, ExtractedTrack>, MkvError> {
    let mut out = HashMap::with_capacity(selected.len());

    for (&track_num, info) in selected {
        let mut raw_list = packets.remove(&track_num).unwrap_or_default();
        if raw_list.is_empty() {
            return Err(MkvError::EmptyTrack {
                track: track_num,
                reason: "no subtitle blocks were found for this track in any Cluster".to_owned(),
            });
        }

        // Fill missing durations from the next packet's start time (or fallback for the last packet).
        let len = raw_list.len();
        for i in 0..len {
            if raw_list[i].duration_ms.is_none() {
                if i + 1 < len && raw_list[i + 1].start_ms > raw_list[i].start_ms {
                    raw_list[i].duration_ms = Some(raw_list[i + 1].start_ms - raw_list[i].start_ms);
                } else {
                    raw_list[i].duration_ms = Some(u64::from(FALLBACK_LAST_DURATION_CS) * 10);
                }
            }
        }

        let mut warnings = Vec::new();
        let script = match &info.subtitle_codec {
            SubtitleCodec::Ass | SubtitleCodec::Ssa => {
                build_ass_script(info, &raw_list, &mut warnings)
            }
            SubtitleCodec::Srt | SubtitleCodec::WebVtt => {
                build_text_script(&raw_list, &mut warnings)
            }
            SubtitleCodec::UnsupportedImage(c) | SubtitleCodec::Other(c) => {
                return Err(MkvError::UnsupportedCodec {
                    track: track_num,
                    codec: c.clone(),
                });
            }
        };

        let event_count = script.events().count();
        if event_count == 0 {
            return Err(MkvError::EmptyTrack {
                track: track_num,
                reason: "blocks were present but contained no valid subtitle events".to_owned(),
            });
        }

        out.insert(
            track_num,
            ExtractedTrack {
                track: info.clone(),
                script,
                event_count,
                warnings,
            },
        );
    }

    Ok(out)
}

fn build_ass_script(
    info: &TrackInfo,
    packets: &[RawSubPacket],
    warnings: &mut Vec<String>,
) -> ScriptFile {
    let mut script = if let Some(ref priv_bytes) = info.codec_private {
        let (outcome, _) = parse_ass_bytes(priv_bytes);
        if !outcome.warnings.is_empty() {
            warnings.push(format!(
                "the track header (CodecPrivate) had {} problem(s); first: line {}: {}",
                outcome.warnings.len(),
                outcome.warnings[0].line,
                outcome.warnings[0].message
            ));
        }
        outcome.value
    } else {
        warnings.push(
            "the track has no CodecPrivate header; a default script header and style were used"
                .to_owned(),
        );
        ScriptFile::new_default()
    };

    // Matroska ASS/SSA block payload format:
    // ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text
    let mut indexed_events: Vec<(i64, usize, AssEvent)> = Vec::with_capacity(packets.len());
    let mut malformed = 0usize;
    let mut bad_numbers = 0usize;
    let mut bad_utf8 = 0usize;
    let mut first_bad: Option<usize> = None;
    for (seq, pkt) in packets.iter().enumerate() {
        let line = String::from_utf8_lossy(&pkt.payload);
        if matches!(line, std::borrow::Cow::Owned(_)) {
            bad_utf8 += 1;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let fields: Vec<&str> = trimmed.splitn(9, ',').collect();

        let start = AssTime::from_ms_rounded(pkt.start_ms);
        let end =
            AssTime::from_ms_rounded(pkt.start_ms.saturating_add(pkt.duration_ms.unwrap_or(0)));

        let (read_order, ev) = if fields.len() >= 9 {
            let mut numeric_ok = true;
            let mut num = |raw: &str| {
                raw.trim().parse::<i32>().unwrap_or_else(|_| {
                    numeric_ok = false;
                    0
                })
            };
            let layer = num(fields[1]);
            let margin_l = num(fields[4]);
            let margin_r = num(fields[5]);
            let margin_v = num(fields[6]);
            let ro = fields[0].trim().parse::<i64>().unwrap_or_else(|_| {
                numeric_ok = false;
                i64::try_from(seq).unwrap_or(0)
            });
            if !numeric_ok {
                bad_numbers += 1;
                first_bad.get_or_insert(seq);
            }
            let mut ev = AssEvent::dialogue(start, end, fields[8]);
            ev.set_kind(EventKind::Dialogue);
            ev.set_layer(layer);
            ev.set_style(fields[2]);
            ev.set_name(fields[3]);
            ev.set_margin_l(margin_l);
            ev.set_margin_r(margin_r);
            ev.set_margin_v(margin_v);
            ev.set_effect(fields[7]);
            (ro, ev)
        } else {
            malformed += 1;
            first_bad.get_or_insert(seq);
            (
                i64::try_from(seq).unwrap_or(0),
                AssEvent::dialogue(start, end, trimmed),
            )
        };
        indexed_events.push((read_order, seq, ev));
    }

    if malformed > 0 {
        warnings.push(format!(
            "{malformed} block(s) did not have the 9 ASS fields and were imported as plain text (first at block {})",
            first_bad.map_or(0, |i| i + 1)
        ));
    }
    if bad_numbers > 0 {
        warnings.push(format!(
            "{bad_numbers} block(s) had a non-numeric ReadOrder/Layer/Margin field; 0 (or the block order) was used"
        ));
    }
    if bad_utf8 > 0 {
        warnings.push(format!(
            "{bad_utf8} block(s) contained invalid UTF-8 that was replaced with U+FFFD"
        ));
    }

    indexed_events.sort_by_key(|&(ro, seq, _)| (ro, seq));
    let events_sec = script.ensure_events_section_mut();
    events_sec.entries = indexed_events
        .into_iter()
        .map(|(_, _, ev)| EventEntry::Event(ev))
        .collect();
    script
}

fn build_text_script(packets: &[RawSubPacket], warnings: &mut Vec<String>) -> ScriptFile {
    let mut script = ScriptFile::new_default();
    let mut entries = Vec::with_capacity(packets.len());
    let mut empty = 0usize;
    let mut bad_utf8 = 0usize;
    let mut unparsed = 0usize;

    for pkt in packets {
        let raw_text = String::from_utf8_lossy(&pkt.payload);
        if matches!(raw_text, std::borrow::Cow::Owned(_)) {
            bad_utf8 += 1;
        }
        let cleaned = raw_text.trim_end_matches(['\r', '\n', '\0']);
        if cleaned.is_empty() {
            empty += 1;
            continue;
        }
        let start = AssTime::from_ms_rounded(pkt.start_ms);
        let end =
            AssTime::from_ms_rounded(pkt.start_ms.saturating_add(pkt.duration_ms.unwrap_or(0)));

        // Reuse ass-core's SRT HTML->ASS converter via a single synthetic cue.
        let synthetic_srt = format!(
            "1\n{} --> {}\n{}\n",
            start.to_srt_string(),
            end.to_srt_string(),
            strip_webvtt_class_tags(cleaned)
        );
        let parsed = parse_srt(&synthetic_srt).value;
        let first_ev = parsed.events().next().cloned();
        if let Some(mut fixed) = first_ev {
            fixed.set_start(start);
            fixed.set_end(end);
            entries.push(EventEntry::Event(fixed));
        } else {
            unparsed += 1;
        }
    }
    if empty > 0 {
        warnings.push(format!("{empty} empty subtitle block(s) were skipped"));
    }
    if unparsed > 0 {
        warnings.push(format!(
            "{unparsed} subtitle block(s) could not be converted and were skipped"
        ));
    }
    if bad_utf8 > 0 {
        warnings.push(format!(
            "{bad_utf8} block(s) contained invalid UTF-8 that was replaced with U+FFFD"
        ));
    }

    script.ensure_events_section_mut().entries = entries;
    script
}

/// Strips `WebVTT` `<c.class>` / `</c>` / `<v Speaker>` tags while preserving `<i>`/`<b>`/`<u>`/`<font>`.
fn strip_webvtt_class_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt + 1..];
        let Some(gt) = after.find('>') else {
            out.push_str(&rest[lt..]);
            return out;
        };
        let tag = after[..gt].trim();
        let lower = tag.to_ascii_lowercase();
        let is_vtt_only = lower == "c"
            || lower == "/c"
            || lower.starts_with("c.")
            || lower.starts_with("v ")
            || lower == "/v"
            || lower.starts_with("lang ");
        if !is_vtt_only {
            out.push('<');
            out.push_str(&after[..=gt]);
        }
        rest = &after[gt + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(start_ms: u64, payload: &[u8]) -> RawSubPacket {
        RawSubPacket {
            start_ms,
            duration_ms: Some(1000),
            payload: payload.to_vec(),
        }
    }

    fn track(codec: SubtitleCodec) -> TrackInfo {
        TrackInfo {
            number: 1,
            track_type: crate::tracks::TrackType::Subtitle,
            codec_id: "S_TEXT/ASS".to_owned(),
            subtitle_codec: codec,
            language: None,
            language_bcp47: None,
            name: None,
            is_default: false,
            is_forced: false,
            codec_private: None,
            encodings: Vec::new(),
        }
    }

    #[test]
    fn ass_blocks_with_bad_fields_produce_warnings() {
        let packets = [
            pkt(0, b"0,0,Default,,0,0,0,,fine"),
            pkt(1000, b"x,y,Default,,0,0,0,,bad numbers"),
            pkt(2000, b"too,few"),
            pkt(3000, b"3,0,Default,,0,0,0,,bad \xFF utf8"),
        ];
        let mut warnings = Vec::new();
        let script = build_ass_script(&track(SubtitleCodec::Ass), &packets, &mut warnings);
        assert_eq!(script.events().count(), 4);
        let joined = warnings.join("\n");
        assert!(joined.contains("no CodecPrivate"), "{joined}");
        assert!(joined.contains("did not have the 9 ASS fields"), "{joined}");
        assert!(joined.contains("non-numeric"), "{joined}");
        assert!(joined.contains("invalid UTF-8"), "{joined}");
    }

    #[test]
    fn clean_ass_blocks_only_warn_about_missing_header() {
        let mut info = track(SubtitleCodec::Ass);
        info.codec_private = Some(
            b"[Script Info]\nScriptType: v4.00+\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
                .to_vec(),
        );
        let mut warnings = Vec::new();
        let _ = build_ass_script(&info, &[pkt(0, b"0,0,Default,,0,0,0,,ok")], &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn empty_text_blocks_are_counted_not_dropped_silently() {
        let packets = [pkt(0, b"hello"), pkt(1000, b""), pkt(2000, b"\n")];
        let mut warnings = Vec::new();
        let script = build_text_script(&packets, &mut warnings);
        assert_eq!(script.events().count(), 1);
        assert_eq!(warnings, vec!["2 empty subtitle block(s) were skipped"]);
    }
}
