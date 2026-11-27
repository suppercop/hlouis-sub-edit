//! Matroska track metadata, `SeekHead`, `Info`, `Tracks`, and `Cues` parsing with no track-count limit.

use crate::ebml::{id, EbmlReader};
use crate::MkvError;
use std::collections::HashMap;
use std::io::{Read, Seek};

/// High-level Matroska track type (`TrackType` element `0x83`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackType {
    Video,
    Audio,
    Subtitle,
    Other(u64),
}

impl TrackType {
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        match raw {
            1 => Self::Video,
            2 => Self::Audio,
            17 => Self::Subtitle,
            other => Self::Other(other),
        }
    }
}

/// Classification of subtitle codecs found in Matroska tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubtitleCodec {
    /// `S_TEXT/ASS`
    Ass,
    /// `S_TEXT/SSA`
    Ssa,
    /// `S_TEXT/UTF8` (`SubRip` / SRT)
    Srt,
    /// `S_TEXT/WEBVTT`
    WebVtt,
    /// Image-based subtitle formats (`S_HDMV/PGS`, `S_VOBSUB`, `S_DVBSUB`, `S_HDMV/TEXTST`).
    UnsupportedImage(String),
    /// Non-subtitle or unrecognized codec ID.
    Other(String),
}

impl SubtitleCodec {
    /// Classifies a Matroska `CodecID` string.
    #[must_use]
    pub fn from_codec_id(codec_id: &str) -> Self {
        match codec_id {
            "S_TEXT/ASS" => Self::Ass,
            "S_TEXT/SSA" => Self::Ssa,
            "S_TEXT/UTF8" => Self::Srt,
            "S_TEXT/WEBVTT" => Self::WebVtt,
            "S_HDMV/PGS" | "S_VOBSUB" | "S_DVBSUB" | "S_HDMV/TEXTST" => {
                Self::UnsupportedImage(codec_id.to_owned())
            }
            other if other.starts_with("S_VOBSUB") || other.starts_with("S_HDMV/") => {
                Self::UnsupportedImage(other.to_owned())
            }
            other => Self::Other(other.to_owned()),
        }
    }

    /// Returns `true` if this codec is a supported text subtitle format.
    #[must_use]
    pub const fn is_supported_text(&self) -> bool {
        matches!(self, Self::Ass | Self::Ssa | Self::Srt | Self::WebVtt)
    }
}

/// Matroska `ContentCompression` algorithm (`ContentCompAlgo` `0x4254`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressionAlgo {
    /// Algorithm 0: zlib deflate.
    Zlib,
    /// Algorithm 3: header stripping (prepend `settings` bytes to every frame).
    HeaderStripping(Vec<u8>),
    /// Unsupported compression algorithm ID.
    Unsupported(u64),
}

/// Parsed `ContentEncoding` transform entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentEncodingInfo {
    pub order: u64,
    pub scope: u64,
    pub algo: CompressionAlgo,
}

/// Complete metadata for a single Matroska track (supports arbitrary track counts > 32).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInfo {
    pub number: u64,
    pub track_type: TrackType,
    pub codec_id: String,
    pub subtitle_codec: SubtitleCodec,
    pub language: Option<String>,
    pub language_bcp47: Option<String>,
    pub name: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub codec_private: Option<Vec<u8>>,
    pub encodings: Vec<ContentEncodingInfo>,
}

impl TrackInfo {
    /// Returns the preferred language tag (`LanguageBCP47` if present, otherwise ISO 639-2 `Language`).
    #[must_use]
    pub fn display_language(&self) -> Option<&str> {
        self.language_bcp47.as_deref().or(self.language.as_deref())
    }
}

/// Parsed Matroska container metadata (`Info`, `Tracks`, `Cues`, and Segment layout).
#[derive(Debug, Clone)]
pub struct MkvMetadata {
    pub segment_data_start: u64,
    pub segment_end: u64,
    pub timestamp_scale_ns: u64,
    pub tracks: Vec<TrackInfo>,
    /// Cluster byte offsets (absolute file offsets) indexed by track number from `Cues`.
    pub cue_clusters_by_track: HashMap<u64, Vec<u64>>,
    /// First cluster offset encountered during initial scan, if any.
    pub first_cluster_pos: Option<u64>,
}

/// Lists all tracks in a Matroska stream with no track-count limit.
pub fn list_tracks<R: Read + Seek>(reader: R) -> Result<Vec<TrackInfo>, MkvError> {
    let mut ebml = EbmlReader::new(reader)?;
    let meta = read_metadata(&mut ebml, false)?;
    Ok(meta.tracks)
}

/// Reads `Info`, `Tracks`, and optionally `Cues` using `SeekHead` when available, falling back
/// to scanning top-level Segment elements up to the first `Cluster`.
#[allow(clippy::too_many_lines)]
pub(crate) fn read_metadata<R: Read + Seek>(
    reader: &mut EbmlReader<R>,
    load_cues: bool,
) -> Result<MkvMetadata, MkvError> {
    reader.seek_to(0)?;
    let file_len = reader.len();

    // 1. Verify EBML header.
    let ebml_hdr = reader
        .read_header(file_len)?
        .ok_or_else(|| MkvError::InvalidFormat("empty file".to_owned()))?;
    if ebml_hdr.id != id::EBML {
        return Err(MkvError::InvalidFormat(format!(
            "expected EBML header 0x1A45DFA3, got 0x{:X}",
            ebml_hdr.id
        )));
    }
    if let Some(size) = ebml_hdr.size {
        reader.skip(size)?;
    }

    // 2. Locate Segment element.
    let mut segment_hdr = None;
    while let Some(hdr) = reader.read_header(file_len)? {
        if hdr.id == id::SEGMENT {
            segment_hdr = Some(hdr);
            break;
        }
        if let Some(size) = hdr.size {
            reader.skip(size)?;
        } else {
            break;
        }
    }
    let seg = segment_hdr
        .ok_or_else(|| MkvError::InvalidFormat("missing Matroska Segment element".to_owned()))?;
    let segment_data_start = seg.data_pos;
    let segment_end = seg.end_pos().map_or(file_len, |e| e.min(file_len));

    let mut timestamp_scale_ns = 1_000_000u64;
    let mut tracks: Vec<TrackInfo> = Vec::new();
    let mut cue_clusters_by_track: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut first_cluster_pos: Option<u64> = None;

    let mut seek_targets: HashMap<u32, u64> = HashMap::new();
    let mut saw_info = false;
    let mut saw_tracks = false;
    let mut saw_cues = false;

    // 3. Scan top-level Segment children until first Cluster (collecting SeekHead, Info, Tracks).
    while let Some(hdr) = reader.read_header(segment_end)? {
        match hdr.id {
            id::VOID | id::CRC32 => {
                if let Some(size) = hdr.size {
                    reader.skip(size)?;
                }
            }
            id::SEEK_HEAD => {
                let end = hdr.end_pos().map_or(segment_end, |e| e.min(segment_end));
                parse_seek_head(reader, end, segment_data_start, &mut seek_targets)?;
                reader.seek_to(end)?;
            }
            id::INFO => {
                let end = hdr.end_pos().map_or(segment_end, |e| e.min(segment_end));
                if let Some(scale) = parse_info(reader, end)? {
                    timestamp_scale_ns = scale;
                }
                saw_info = true;
                reader.seek_to(end)?;
            }
            id::TRACKS => {
                let end = hdr.end_pos().map_or(segment_end, |e| e.min(segment_end));
                parse_tracks(reader, end, &mut tracks)?;
                saw_tracks = true;
                reader.seek_to(end)?;
            }
            id::CUES => {
                let end = hdr.end_pos().map_or(segment_end, |e| e.min(segment_end));
                if load_cues {
                    parse_cues(reader, end, segment_data_start, &mut cue_clusters_by_track)?;
                    saw_cues = true;
                }
                reader.seek_to(end)?;
            }
            id::CLUSTER => {
                first_cluster_pos = Some(hdr.header_pos);
                break;
            }
            _ => {
                if let Some(size) = hdr.size {
                    reader.skip(size)?;
                } else {
                    break;
                }
            }
        }
    }

    // 4. Follow SeekHead pointers for any metadata located after Clusters (e.g. Cues or late Tracks).
    if !saw_info {
        if let Some(&pos) = seek_targets.get(&id::INFO) {
            if pos < segment_end {
                reader.seek_to(pos)?;
                if let Some(hdr) = reader.read_header(segment_end)? {
                    if hdr.id == id::INFO {
                        let end = hdr.end_pos().map_or(segment_end, |e| e.min(segment_end));
                        if let Some(scale) = parse_info(reader, end)? {
                            timestamp_scale_ns = scale;
                        }
                    }
                }
            }
        }
    }
    if !saw_tracks {
        if let Some(&pos) = seek_targets.get(&id::TRACKS) {
            if pos < segment_end {
                reader.seek_to(pos)?;
                if let Some(hdr) = reader.read_header(segment_end)? {
                    if hdr.id == id::TRACKS {
                        let end = hdr.end_pos().map_or(segment_end, |e| e.min(segment_end));
                        parse_tracks(reader, end, &mut tracks)?;
                    }
                }
            }
        }
    }
    if load_cues && !saw_cues {
        if let Some(&pos) = seek_targets.get(&id::CUES) {
            if pos < segment_end {
                reader.seek_to(pos)?;
                if let Some(hdr) = reader.read_header(segment_end)? {
                    if hdr.id == id::CUES {
                        let end = hdr.end_pos().map_or(segment_end, |e| e.min(segment_end));
                        parse_cues(reader, end, segment_data_start, &mut cue_clusters_by_track)?;
                    }
                }
            }
        }
    }

    Ok(MkvMetadata {
        segment_data_start,
        segment_end,
        timestamp_scale_ns,
        tracks,
        cue_clusters_by_track,
        first_cluster_pos,
    })
}

fn parse_seek_head<R: Read + Seek>(
    reader: &mut EbmlReader<R>,
    end: u64,
    segment_data_start: u64,
    out: &mut HashMap<u32, u64>,
) -> Result<(), MkvError> {
    while let Some(hdr) = reader.read_header(end)? {
        let elem_end = hdr.end_pos().map_or(end, |e| e.min(end));
        if hdr.id == id::SEEK {
            let mut target_id: Option<u32> = None;
            let mut rel_pos: Option<u64> = None;
            while let Some(sub) = reader.read_header(elem_end)? {
                let sub_end = sub.end_pos().map_or(elem_end, |e| e.min(elem_end));
                match sub.id {
                    id::SEEK_ID => {
                        if let Some(sz) = sub.size {
                            if sz <= 4 {
                                let v = reader.read_uint(sz)?;
                                target_id = u32::try_from(v).ok();
                            }
                        }
                    }
                    id::SEEK_POSITION => {
                        if let Some(sz) = sub.size {
                            rel_pos = Some(reader.read_uint(sz)?);
                        }
                    }
                    _ => {}
                }
                reader.seek_to(sub_end)?;
            }
            if let (Some(tid), Some(rp)) = (target_id, rel_pos) {
                if let Some(abs) = segment_data_start.checked_add(rp) {
                    out.entry(tid).or_insert(abs);
                }
            }
        }
        reader.seek_to(elem_end)?;
    }
    Ok(())
}

fn parse_info<R: Read + Seek>(
    reader: &mut EbmlReader<R>,
    end: u64,
) -> Result<Option<u64>, MkvError> {
    let mut scale = None;
    while let Some(hdr) = reader.read_header(end)? {
        let elem_end = hdr.end_pos().map_or(end, |e| e.min(end));
        if hdr.id == id::TIMESTAMP_SCALE {
            if let Some(sz) = hdr.size {
                scale = Some(reader.read_uint(sz)?);
            }
        }
        reader.seek_to(elem_end)?;
    }
    Ok(scale)
}

fn parse_tracks<R: Read + Seek>(
    reader: &mut EbmlReader<R>,
    end: u64,
    tracks: &mut Vec<TrackInfo>,
) -> Result<(), MkvError> {
    while let Some(hdr) = reader.read_header(end)? {
        let elem_end = hdr.end_pos().map_or(end, |e| e.min(end));
        if hdr.id == id::TRACK_ENTRY {
            if let Some(track) = parse_track_entry(reader, elem_end)? {
                tracks.push(track);
            }
        }
        reader.seek_to(elem_end)?;
    }
    Ok(())
}

fn parse_track_entry<R: Read + Seek>(
    reader: &mut EbmlReader<R>,
    end: u64,
) -> Result<Option<TrackInfo>, MkvError> {
    let mut number: Option<u64> = None;
    let mut track_type = TrackType::Other(0);
    let mut codec_id = String::new();
    let mut language: Option<String> = None;
    let mut language_bcp47: Option<String> = None;
    let mut name: Option<String> = None;
    let mut is_default = true; // Matroska spec default for FlagDefault is 1.
    let mut is_forced = false;
    let mut codec_private: Option<Vec<u8>> = None;
    let mut encodings: Vec<ContentEncodingInfo> = Vec::new();

    while let Some(hdr) = reader.read_header(end)? {
        let elem_end = hdr.end_pos().map_or(end, |e| e.min(end));
        match hdr.id {
            id::TRACK_NUMBER => {
                if let Some(sz) = hdr.size {
                    number = Some(reader.read_uint(sz)?);
                }
            }
            id::TRACK_TYPE => {
                if let Some(sz) = hdr.size {
                    track_type = TrackType::from_raw(reader.read_uint(sz)?);
                }
            }
            id::FLAG_DEFAULT => {
                if let Some(sz) = hdr.size {
                    is_default = reader.read_uint(sz)? != 0;
                }
            }
            id::FLAG_FORCED => {
                if let Some(sz) = hdr.size {
                    is_forced = reader.read_uint(sz)? != 0;
                }
            }
            id::NAME => {
                if let Some(sz) = hdr.size {
                    name = Some(reader.read_string(sz)?);
                }
            }
            id::LANGUAGE => {
                if let Some(sz) = hdr.size {
                    language = Some(reader.read_string(sz)?);
                }
            }
            id::LANGUAGE_BCP47 => {
                if let Some(sz) = hdr.size {
                    language_bcp47 = Some(reader.read_string(sz)?);
                }
            }
            id::CODEC_ID => {
                if let Some(sz) = hdr.size {
                    codec_id = reader.read_string(sz)?;
                }
            }
            id::CODEC_PRIVATE => {
                if let Some(sz) = hdr.size {
                    codec_private = Some(reader.read_bytes(sz)?);
                }
            }
            id::CONTENT_ENCODINGS => {
                parse_content_encodings(reader, elem_end, &mut encodings)?;
            }
            _ => {}
        }
        reader.seek_to(elem_end)?;
    }

    // Sort by ContentEncodingOrder so decoding reverses them in descending order.
    encodings.sort_by_key(|e| e.order);

    let Some(track_num) = number else {
        return Ok(None);
    };
    let subtitle_codec = SubtitleCodec::from_codec_id(&codec_id);

    Ok(Some(TrackInfo {
        number: track_num,
        track_type,
        codec_id,
        subtitle_codec,
        language,
        language_bcp47,
        name,
        is_default,
        is_forced,
        codec_private,
        encodings,
    }))
}

fn parse_content_encodings<R: Read + Seek>(
    reader: &mut EbmlReader<R>,
    end: u64,
    out: &mut Vec<ContentEncodingInfo>,
) -> Result<(), MkvError> {
    while let Some(hdr) = reader.read_header(end)? {
        let elem_end = hdr.end_pos().map_or(end, |e| e.min(end));
        if hdr.id == id::CONTENT_ENCODING {
            let mut order = 0u64;
            let mut scope = 1u64; // Default scope = 1 (all frame contents).
            let mut enc_type = 0u64; // Default type = 0 (compression).
            let mut algo: Option<CompressionAlgo> = None;

            while let Some(sub) = reader.read_header(elem_end)? {
                let sub_end = sub.end_pos().map_or(elem_end, |e| e.min(elem_end));
                match sub.id {
                    id::CONTENT_ENCODING_ORDER => {
                        if let Some(sz) = sub.size {
                            order = reader.read_uint(sz)?;
                        }
                    }
                    id::CONTENT_ENCODING_SCOPE => {
                        if let Some(sz) = sub.size {
                            scope = reader.read_uint(sz)?;
                        }
                    }
                    id::CONTENT_ENCODING_TYPE => {
                        if let Some(sz) = sub.size {
                            enc_type = reader.read_uint(sz)?;
                        }
                    }
                    id::CONTENT_COMPRESSION => {
                        algo = Some(parse_content_compression(reader, sub_end)?);
                    }
                    _ => {}
                }
                reader.seek_to(sub_end)?;
            }

            if enc_type == 0 {
                out.push(ContentEncodingInfo {
                    order,
                    scope,
                    algo: algo.unwrap_or(CompressionAlgo::Zlib),
                });
            }
        }
        reader.seek_to(elem_end)?;
    }
    Ok(())
}

fn parse_content_compression<R: Read + Seek>(
    reader: &mut EbmlReader<R>,
    end: u64,
) -> Result<CompressionAlgo, MkvError> {
    let mut algo_id = 0u64; // Matroska default ContentCompAlgo = 0 (zlib).
    let mut settings = Vec::new();
    while let Some(hdr) = reader.read_header(end)? {
        let elem_end = hdr.end_pos().map_or(end, |e| e.min(end));
        match hdr.id {
            id::CONTENT_COMP_ALGO => {
                if let Some(sz) = hdr.size {
                    algo_id = reader.read_uint(sz)?;
                }
            }
            id::CONTENT_COMP_SETTINGS => {
                if let Some(sz) = hdr.size {
                    settings = reader.read_bytes(sz)?;
                }
            }
            _ => {}
        }
        reader.seek_to(elem_end)?;
    }
    Ok(match algo_id {
        0 => CompressionAlgo::Zlib,
        3 => CompressionAlgo::HeaderStripping(settings),
        other => CompressionAlgo::Unsupported(other),
    })
}

fn parse_cues<R: Read + Seek>(
    reader: &mut EbmlReader<R>,
    end: u64,
    segment_data_start: u64,
    out: &mut HashMap<u64, Vec<u64>>,
) -> Result<(), MkvError> {
    while let Some(hdr) = reader.read_header(end)? {
        let elem_end = hdr.end_pos().map_or(end, |e| e.min(end));
        if hdr.id == id::CUE_POINT {
            while let Some(sub) = reader.read_header(elem_end)? {
                let sub_end = sub.end_pos().map_or(elem_end, |e| e.min(elem_end));
                if sub.id == id::CUE_TRACK_POSITIONS {
                    let mut track: Option<u64> = None;
                    let mut cluster_rel: Option<u64> = None;
                    while let Some(pos_el) = reader.read_header(sub_end)? {
                        let pos_end = pos_el.end_pos().map_or(sub_end, |e| e.min(sub_end));
                        match pos_el.id {
                            id::CUE_TRACK => {
                                if let Some(sz) = pos_el.size {
                                    track = Some(reader.read_uint(sz)?);
                                }
                            }
                            id::CUE_CLUSTER_POSITION => {
                                if let Some(sz) = pos_el.size {
                                    cluster_rel = Some(reader.read_uint(sz)?);
                                }
                            }
                            _ => {}
                        }
                        reader.seek_to(pos_end)?;
                    }
                    if let (Some(t), Some(rel)) = (track, cluster_rel) {
                        if let Some(abs) = segment_data_start.checked_add(rel) {
                            let list = out.entry(t).or_default();
                            if list.last() != Some(&abs) {
                                list.push(abs);
                            }
                        }
                    }
                }
                reader.seek_to(sub_end)?;
            }
        }
        reader.seek_to(elem_end)?;
    }
    Ok(())
}
