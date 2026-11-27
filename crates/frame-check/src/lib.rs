//! Frame-accuracy self-check.
//!
//! For every dialogue event we compute the first and last frame it should be visible on using our
//! own timing code (`keyframes::Timecodes` with Aegisub Start/End semantics), then render the
//! boundary frames (first-1, first, last, last+1) with libass at the real frame timestamps and
//! check whether the event's bitmap is present. Any disagreement is reported, so the classic
//! "signs disappear ~40 ms early in players" problem can never happen silently.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{serialize_ass, AssEvent, EventEntry, EventKind, ScriptFile};
use keyframes::{TimeType, Timecodes};
use render::AssSubRenderer;
use std::fmt;
use std::fmt::Write as _;
use thiserror::Error;

/// Errors that stop the self-check (never a silent "0 events checked").
#[derive(Debug, Error)]
pub enum FrameCheckError {
    #[error("the video has no frames")]
    NoFrames,
    #[error("the script has no dialogue events to check")]
    NoEvents,
    #[error("invalid frame timestamps: {0}")]
    Timecodes(#[from] keyframes::KeyframeError),
    #[error("subtitle rendering failed: {0}")]
    Render(#[from] render::RenderError),
    #[error("video error: {0}")]
    Media(#[from] media::MediaError),
    #[error("matroska error: {0}")]
    Mkv(#[from] mkv_subs::MkvError),
    #[error("{0}")]
    NoSubtitleTrack(String),
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Which probe disagreed with our timing code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mismatch {
    /// Not visible on the first frame we expect it on (appears late).
    MissingOnFirst,
    /// Visible on the frame before the first expected one (appears early).
    PresentBeforeFirst,
    /// Not visible on the last frame we expect it on (disappears early).
    MissingOnLast,
    /// Still visible on the frame after the last expected one (disappears late).
    PresentAfterLast,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MissingOnFirst => "not visible on its first frame (appears late)",
            Self::PresentBeforeFirst => "visible one frame before its first frame (appears early)",
            Self::MissingOnLast => "not visible on its last frame (disappears early)",
            Self::PresentAfterLast => {
                "still visible one frame after its last frame (disappears late)"
            }
        })
    }
}

/// Outcome for a single event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// libass agrees with our timing at every probed boundary.
    Ok,
    /// libass disagrees; see the list.
    Mismatch(Vec<Mismatch>),
    /// The event spans no whole frame (shorter than a frame): no player will ever show it.
    ZeroFrames,
    /// The event starts after the last video frame.
    BeyondVideo,
    /// libass produced no bitmap anywhere in the event (blank text, fully transparent, ...), so
    /// its boundaries cannot be verified.
    NoBitmap,
}

/// Per-event result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventResult {
    /// 0-based index among the script's events (comments included).
    pub index: usize,
    pub start_cs: u32,
    pub end_cs: u32,
    /// First frame our timing code says the event is visible on (0-based).
    pub first_frame: u32,
    /// Last frame our timing code says the event is visible on (inclusive).
    pub last_frame: Option<u32>,
    pub verdict: Verdict,
    pub text_preview: String,
}

/// Full self-check report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrameCheckReport {
    pub results: Vec<EventResult>,
    pub frames: usize,
}

impl FrameCheckReport {
    #[must_use]
    pub fn count(&self, pred: impl Fn(&Verdict) -> bool) -> usize {
        self.results.iter().filter(|r| pred(&r.verdict)).count()
    }

    /// Events where libass disagrees with our timing.
    #[must_use]
    pub fn mismatches(&self) -> Vec<&EventResult> {
        self.results
            .iter()
            .filter(|r| matches!(r.verdict, Verdict::Mismatch(_)))
            .collect()
    }

    /// `true` when every verifiable event matched.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.mismatches().is_empty()
    }

    /// Multi-line human-readable summary (mismatches first, then counts).
    #[must_use]
    pub fn summary(&self, max_lines: usize) -> String {
        let mut out = String::new();
        let mism = self.mismatches();
        for r in mism.iter().take(max_lines) {
            if let Verdict::Mismatch(list) = &r.verdict {
                let reasons: Vec<String> = list.iter().map(ToString::to_string).collect();
                let _ = writeln!(
                    out,
                    "MISMATCH event #{} [{}..{} cs] frames {}..{}: {} | {}",
                    r.index + 1,
                    r.start_cs,
                    r.end_cs,
                    r.first_frame,
                    r.last_frame.map_or("-".to_owned(), |l| l.to_string()),
                    reasons.join("; "),
                    r.text_preview
                );
            }
        }
        if mism.len() > max_lines {
            let _ = writeln!(out, "... and {} more mismatches", mism.len() - max_lines);
        }
        let _ = writeln!(
            out,
            "checked {} events over {} frames: {} ok, {} MISMATCH, {} shorter than one frame, {} beyond video end, {} without bitmap (unverifiable)",
            self.results.len(),
            self.frames,
            self.count(|v| *v == Verdict::Ok),
            mism.len(),
            self.count(|v| *v == Verdict::ZeroFrames),
            self.count(|v| *v == Verdict::BeyondVideo),
            self.count(|v| *v == Verdict::NoBitmap),
        );
        out
    }
}

/// Builds the timing model from decoded frame timestamps. Uses the exact timestamp table (valid
/// for both CFR and VFR).
pub fn timecodes_from_pts(pts_ms: &[i64]) -> Result<Timecodes, FrameCheckError> {
    if pts_ms.is_empty() {
        return Err(FrameCheckError::NoFrames);
    }
    Ok(Timecodes::from_vfr_ms(pts_ms.to_vec())?)
}

/// Runs the self-check against the frame timestamps `pts_ms` of a `width`x`height` video.
pub fn check_script(
    script: &ScriptFile,
    pts_ms: &[i64],
    timecodes: &Timecodes,
    width: u32,
    height: u32,
) -> Result<FrameCheckReport, FrameCheckError> {
    if pts_ms.is_empty() {
        return Err(FrameCheckError::NoFrames);
    }
    let events: Vec<(usize, &AssEvent)> = script
        .events()
        .enumerate()
        .filter(|(_, e)| e.kind() == EventKind::Dialogue)
        .collect();
    if events.is_empty() {
        return Err(FrameCheckError::NoEvents);
    }

    let mut base = script.clone();
    let mut renderer = AssSubRenderer::new(width, height)?;
    let mut canvas = vec![0u8; width as usize * height as usize * 4];
    let nframes = pts_ms.len();
    let last_index = u32::try_from(nframes - 1).unwrap_or(u32::MAX);
    let mut results = Vec::with_capacity(events.len());

    for (index, ev) in events {
        let start_cs = ev.start().as_cs();
        let end_cs = ev.end().as_cs();
        let first = timecodes.time_cs_to_frame(start_cs, TimeType::Start);
        let mut result = EventResult {
            index,
            start_cs,
            end_cs,
            first_frame: first,
            last_frame: None,
            verdict: Verdict::Ok,
            text_preview: ev.text().chars().take(40).collect(),
        };
        if first as usize >= nframes {
            result.verdict = Verdict::BeyondVideo;
            results.push(result);
            continue;
        }
        // `End` gives the last frame strictly before the end time; an event that ends before the
        // next frame boundary after its start spans no frame at all.
        let last = timecodes.time_cs_to_frame(end_cs, TimeType::End);
        if end_cs <= start_cs || last < first {
            result.verdict = Verdict::ZeroFrames;
            results.push(result);
            continue;
        }
        let last = last.min(last_index);
        result.last_frame = Some(last);

        if let Some(sec) = base.events_section_mut() {
            sec.entries.clear();
            sec.entries.push(EventEntry::Event(ev.clone()));
        }
        renderer.load_ass_memory(&serialize_ass(&base))?;
        let mut present = |frame: u32| -> Result<bool, FrameCheckError> {
            let t = pts_ms[frame as usize];
            let stats = renderer.render_and_blend_rgba(t, &mut canvas, width, height)?;
            Ok(stats.bitmap_count > 0)
        };

        let on_first = present(first)?;
        let on_last = present(last)?;
        let on_mid = present(first + (last - first) / 2)?;
        if !on_first && !on_last && !on_mid {
            result.verdict = Verdict::NoBitmap;
            results.push(result);
            continue;
        }
        let mut bad = Vec::new();
        if !on_first {
            bad.push(Mismatch::MissingOnFirst);
        }
        if first > 0 && present(first - 1)? {
            bad.push(Mismatch::PresentBeforeFirst);
        }
        if !on_last {
            bad.push(Mismatch::MissingOnLast);
        }
        if last < last_index && present(last + 1)? {
            bad.push(Mismatch::PresentAfterLast);
        }
        if !bad.is_empty() {
            result.verdict = Verdict::Mismatch(bad);
        }
        results.push(result);
    }
    Ok(FrameCheckReport {
        results,
        frames: nframes,
    })
}

/// Opens `video` with FFMS2, parses `ass_path` and runs [`check_script`] using the video's real
/// frame timestamps and dimensions.
pub fn check_files(
    video: &std::path::Path,
    ass_path: &std::path::Path,
) -> Result<FrameCheckReport, FrameCheckError> {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let vid = media::FfmsVideo::open_with_progress(video, &cancel, |_, _| {})?;
    let info = vid.info();
    let bytes = std::fs::read(ass_path).map_err(|source| FrameCheckError::Io {
        path: ass_path.display().to_string(),
        source,
    })?;
    let (outcome, _) = ass_core::parse_ass_bytes(&bytes);
    let tc = timecodes_from_pts(&info.frame_pts_ms)?;
    check_script(
        &outcome.value,
        &info.frame_pts_ms,
        &tc,
        info.width,
        info.height,
    )
}

/// Like [`check_files`] but takes the script from a subtitle track of a Matroska file
/// (`track = None` picks the first text subtitle track).
pub fn check_embedded_track(
    video: &std::path::Path,
    mkv: &std::path::Path,
    track: Option<u64>,
) -> Result<FrameCheckReport, FrameCheckError> {
    let open = |path: &std::path::Path| {
        std::fs::File::open(path)
            .map(std::io::BufReader::new)
            .map_err(|source| FrameCheckError::Io {
                path: path.display().to_string(),
                source,
            })
    };
    let tracks = mkv_subs::list_tracks(open(mkv)?)?;
    let chosen = tracks
        .iter()
        .find(|t| {
            t.track_type == mkv_subs::TrackType::Subtitle
                && track.is_none_or(|n| t.number == n)
                && !matches!(
                    t.subtitle_codec,
                    mkv_subs::SubtitleCodec::UnsupportedImage(_)
                        | mkv_subs::SubtitleCodec::Other(_)
                )
        })
        .ok_or_else(|| {
            FrameCheckError::NoSubtitleTrack(format!(
                "no usable text subtitle track{} in {}",
                track.map_or(String::new(), |n| format!(" #{n}")),
                mkv.display()
            ))
        })?;
    let mut extracted = mkv_subs::extract_tracks(open(mkv)?, &[chosen.number], None, |_, _| {})?;
    let script = extracted
        .remove(&chosen.number)
        .ok_or_else(|| FrameCheckError::NoSubtitleTrack("extraction returned no track".to_owned()))?
        .script;

    let cancel = std::sync::atomic::AtomicBool::new(false);
    let vid = media::FfmsVideo::open_with_progress(video, &cancel, |_, _| {})?;
    let info = vid.info();
    let tc = timecodes_from_pts(&info.frame_pts_ms)?;
    check_script(&script, &info.frame_pts_ms, &tc, info.width, info.height)
}
