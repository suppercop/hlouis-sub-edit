//! egui "Open subtitles from video (MKV)" dialog on top of [`mkv_import`] and [`mkv_subs`].
//!
//! Fixes Aegisub 3.5.1's 32-track Matroska limit by listing every subtitle track (64+ verified),
//! showing bitmap tracks (`PGS`/`VobSub`) as disabled with their reason, filtering by language or
//! title, previewing the first 5 lines, supporting multi-track separate or merged import with
//! progress + cancel, and never failing silently on 0-event tracks.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use egui::{ProgressBar, ScrollArea, Ui};
use i18n_catalog::Catalog;
use mkv_import::{ImportError, ImportOutput, MultiTrackMode, TrackPickerModel, TrackPickerRow};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Action emitted by [`show_mkv_import_dialog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MkvImportAction {
    None,
    ImportSelected {
        track_numbers: Vec<u64>,
        mode: MultiTrackMode,
    },
    RequestPreview(u64),
    CancelReading,
    Close,
}

/// Persistent UI state for the MKV subtitle track importer dialog.
#[derive(Debug, Clone)]
pub struct MkvImportDialogState {
    pub picker: TrackPickerModel,
    pub filter_text: String,
    pub multi_mode: MultiTrackMode,
    pub focused_track: Option<u64>,
    pub in_progress: bool,
    pub progress: f32,
    pub cancel_flag: Arc<AtomicBool>,
    pub status_warning: Option<String>,
}

impl Default for MkvImportDialogState {
    fn default() -> Self {
        Self {
            picker: TrackPickerModel::default(),
            filter_text: String::new(),
            multi_mode: MultiTrackMode::SeparateScripts,
            focused_track: None,
            in_progress: false,
            progress: 0.0,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            status_warning: None,
        }
    }
}

impl MkvImportDialogState {
    /// Initializes dialog state from a populated [`TrackPickerModel`].
    #[must_use]
    pub fn from_picker(picker: TrackPickerModel) -> Self {
        let focused_track = picker.rows().first().map(|r| r.track_number);
        Self {
            picker,
            focused_track,
            ..Self::default()
        }
    }

    /// Validates an [`ImportOutput`] and populates [`Self::status_warning`] if any imported track
    /// yielded 0 events (never fails silently).
    pub fn record_import_result(
        &mut self,
        result: Result<&ImportOutput, &ImportError>,
        catalog: &Catalog,
    ) {
        let zero_msg = catalog
            .get("mkv-import-zero-events")
            .unwrap_or("Selected track yielded 0 subtitle events");
        match result {
            Err(err) => {
                self.status_warning = Some(format!("{zero_msg}: {err}"));
            }
            Ok(output) => {
                let empty = match output {
                    ImportOutput::Separate(docs) => {
                        docs.iter().any(|(_, s)| s.events().count() == 0)
                    }
                    ImportOutput::Merged(s) => s.events().count() == 0,
                };
                self.status_warning = empty.then(|| zero_msg.to_owned());
            }
        }
    }
}

/// Returns `true` when `row` matches the case-insensitive `filter` (0 heap allocations when
/// `filter` is empty).
#[must_use]
pub fn row_matches_filter(row: &TrackPickerRow, filter_lower: &str) -> bool {
    if filter_lower.is_empty() {
        return true;
    }
    row.language.to_ascii_lowercase().contains(filter_lower)
        || row.name.to_ascii_lowercase().contains(filter_lower)
        || row.codec_id.to_ascii_lowercase().contains(filter_lower)
}

/// Renders the egui "Open subtitles from video (MKV)" dialog.
#[allow(clippy::too_many_lines)]
pub fn show_mkv_import_dialog(
    ui: &mut Ui,
    state: &mut MkvImportDialogState,
    catalog: &Catalog,
) -> MkvImportAction {
    let mut action = MkvImportAction::None;

    ui.heading(
        catalog
            .get("mkv-import-title")
            .unwrap_or("Open Subtitles from Video (MKV)"),
    );

    // Filter bar
    ui.horizontal(|ui| {
        ui.label(
            catalog
                .get("mkv-import-filter")
                .unwrap_or("Filter by language or title..."),
        );
        ui.text_edit_singleline(&mut state.filter_text);
        ui.label(format!("({} tracks)", state.picker.rows().len()));
    });

    // Progress + Cancel bar when reading
    if state.in_progress {
        ui.horizontal(|ui| {
            ui.add(ProgressBar::new(state.progress.clamp(0.0, 1.0)).show_percentage());
            if ui
                .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                .clicked()
            {
                state.cancel_flag.store(true, Ordering::Relaxed);
                action = MkvImportAction::CancelReading;
            }
        });
    }

    ui.separator();

    let filter_lower = state.filter_text.to_ascii_lowercase();
    let mut toggle_track: Option<u64> = None;
    let mut focus_track: Option<u64> = None;

    ui.horizontal(|ui| {
        // Left: complete track list (no 32-track cap)
        ui.vertical(|ui| {
            ui.set_min_width(420.0);
            ScrollArea::vertical()
                .id_salt("mkv_track_list")
                .max_height(260.0)
                .show(ui, |ui| {
                    for row in state.picker.rows() {
                        if !row_matches_filter(row, &filter_lower) {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            let mut checked = row.selected;
                            let cb = ui
                                .add_enabled(row.selectable, egui::Checkbox::new(&mut checked, ""));
                            if cb.changed() {
                                toggle_track = Some(row.track_number);
                            }
                            let flags = match (row.is_default, row.is_forced) {
                                (true, true) => " [Default, Forced]",
                                (true, false) => " [Default]",
                                (false, true) => " [Forced]",
                                (false, false) => "",
                            };
                            let count_str = row
                                .event_count
                                .map_or_else(String::new, |c| format!(" ({c} lines)"));
                            let label = format!(
                                "#{} {} | {} | {}{}{}",
                                row.track_number,
                                row.codec_id,
                                row.language,
                                row.name,
                                flags,
                                count_str
                            );
                            let is_focused = state.focused_track == Some(row.track_number);
                            if ui.selectable_label(is_focused, label).clicked() {
                                focus_track = Some(row.track_number);
                            }
                            if !row.selectable {
                                let reason =
                                    row.unselectable_reason.as_deref().unwrap_or_else(|| {
                                        catalog
                                            .get("mkv-import-bitmap-disabled")
                                            .unwrap_or("Bitmap track unsupported")
                                    });
                                ui.weak(format!("— {reason}"));
                            }
                        });
                    }
                });
        });

        ui.separator();

        // Right: 5-line preview for focused track
        ui.vertical(|ui| {
            ui.set_min_width(260.0);
            ui.strong("Preview (first 5 lines)");
            if let Some(tn) = state.focused_track {
                if let Some(row) = state.picker.rows().iter().find(|r| r.track_number == tn) {
                    if row.preview_lines.is_empty() {
                        ui.weak("(No preview loaded yet)");
                    } else {
                        for line in row.preview_lines.iter().take(5) {
                            ui.monospace(line);
                        }
                    }
                } else {
                    ui.weak("Select a track to preview.");
                }
            } else {
                ui.weak("Select a track to preview.");
            }
        });
    });

    if let Some(tn) = toggle_track {
        state.picker.toggle_select(tn);
    }
    if let Some(tn) = focus_track {
        state.focused_track = Some(tn);
        if action == MkvImportAction::None {
            action = MkvImportAction::RequestPreview(tn);
        }
    }

    ui.separator();

    // Multi-track mode selection
    ui.horizontal(|ui| {
        ui.radio_value(
            &mut state.multi_mode,
            MultiTrackMode::SeparateScripts,
            catalog
                .get("mkv-import-separate")
                .unwrap_or("Open each selected track as a separate document"),
        );
        ui.radio_value(
            &mut state.multi_mode,
            MultiTrackMode::MergeSeparateStyles,
            catalog
                .get("mkv-import-merge")
                .unwrap_or("Merge selected tracks into one script"),
        );
    });

    if let Some(ref warn) = state.status_warning {
        ui.colored_label(egui::Color32::from_rgb(230, 120, 40), warn);
    }

    ui.horizontal(|ui| {
        let selected = state.picker.selected_track_numbers();
        if ui
            .add_enabled(
                !selected.is_empty() && !state.in_progress,
                egui::Button::new(format!(
                    "{} ({})",
                    catalog.get("btn-ok").unwrap_or("OK"),
                    selected.len()
                )),
            )
            .clicked()
        {
            action = MkvImportAction::ImportSelected {
                track_numbers: selected,
                mode: state.multi_mode,
            };
        }
        if ui
            .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
            .clicked()
        {
            action = MkvImportAction::Close;
        }
    });

    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use i18n_catalog::Locale;
    use mkv_import::import_tracks;
    use mkv_subs::list_tracks;
    use std::io::Cursor;
    use theme::{BaseTheme, ThemeSpec};
    use ui_theme::{FontConfig, ThemeController};

    fn ebml_vint_size(len: usize) -> Vec<u8> {
        if len < 0x7F {
            vec![0x80 | u8::try_from(len).expect("vint1")]
        } else if len < 0x3FFF {
            let v = u16::try_from(len).expect("vint2") | 0x4000;
            v.to_be_bytes().to_vec()
        } else {
            let v = u32::try_from(len).expect("vint4") | 0x1000_0000;
            v.to_be_bytes().to_vec()
        }
    }

    fn ebml_elem(id: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(id.len() + 4 + payload.len());
        out.extend_from_slice(id);
        out.extend_from_slice(&ebml_vint_size(payload.len()));
        out.extend_from_slice(payload);
        out
    }

    fn build_synthetic_mkv_with_68_sub_tracks() -> Vec<u8> {
        let mut tracks_payload = Vec::new();
        for idx in 1u8..=68u8 {
            let mut te = Vec::new();
            te.extend_from_slice(&ebml_elem(&[0xD7], &[idx])); // TrackNumber
            te.extend_from_slice(&ebml_elem(&[0x83], &[0x11])); // TrackType = 17 (Subtitle)
            let codec = if idx <= 64 {
                b"S_TEXT/UTF8".as_slice()
            } else {
                b"S_HDMV/PGS".as_slice()
            };
            te.extend_from_slice(&ebml_elem(&[0x86], codec)); // CodecID
            let lang = if idx % 2 == 0 { b"vie" } else { b"eng" };
            te.extend_from_slice(&ebml_elem(&[0x22, 0xB5, 0x9C], lang)); // Language
            let name = format!("Track {idx}");
            te.extend_from_slice(&ebml_elem(&[0x53, 0x6E], name.as_bytes())); // Name
            if idx == 1 {
                te.extend_from_slice(&ebml_elem(&[0x88], &[1])); // FlagDefault
            }
            tracks_payload.extend_from_slice(&ebml_elem(&[0xAE], &te));
        }

        let tracks_elem = ebml_elem(&[0x16, 0x54, 0xAE, 0x6B], &tracks_payload);
        // Add one Cluster (0x1F43B675) with one SimpleBlock for track 1 so track 1 has 1 event,
        // while track 2 has 0 events (testing non-silent warning).
        let mut simple_block = vec![0x81, 0x00, 0x00, 0x00]; // track 1, tc 0, flags 0
        simple_block.extend_from_slice(b"Hello from track 1");
        let mut cluster_payload = Vec::new();
        cluster_payload.extend_from_slice(&ebml_elem(&[0xE7], &[0x00])); // Timecode 0
        cluster_payload.extend_from_slice(&ebml_elem(&[0xA3], &simple_block));
        let cluster_elem = ebml_elem(&[0x1F, 0x43, 0xB6, 0x75], &cluster_payload);

        let mut seg_payload = Vec::new();
        seg_payload.extend_from_slice(&tracks_elem);
        seg_payload.extend_from_slice(&cluster_elem);

        let ebml_hdr = ebml_elem(&[0x1A, 0x45, 0xDF, 0xA3], &ebml_elem(&[0x42, 0x86], &[1]));
        let seg = ebml_elem(&[0x18, 0x53, 0x80, 0x67], &seg_payload);
        let mut mkv = Vec::new();
        mkv.extend_from_slice(&ebml_hdr);
        mkv.extend_from_slice(&seg);
        mkv
    }

    #[test]
    fn handles_68_subtitle_tracks_zero_event_warning_and_all_3_themes() {
        let mkv = build_synthetic_mkv_with_68_sub_tracks();
        let tracks = list_tracks(Cursor::new(&mkv)).expect("list_tracks");
        assert_eq!(
            tracks.len(),
            68,
            "must list all 68 tracks with no 32-track cap"
        );

        let picker = TrackPickerModel::from_tracks(&tracks);
        assert_eq!(picker.rows().len(), 68);
        // Tracks 65..=68 are PGS bitmap -> disabled with reason
        for row in &picker.rows()[64..68] {
            assert!(!row.selectable);
            assert!(row.unselectable_reason.is_some());
        }

        // Import track 2 (0 events) -> must surface non-silent warning!
        let mut state = MkvImportDialogState::from_picker(picker);
        let out_empty = import_tracks(Cursor::new(&mkv), &[2], MultiTrackMode::SeparateScripts);
        assert!(out_empty.is_err());
        let cat_vi = Catalog::for_locale(Locale::Vi);
        state.record_import_result(out_empty.as_ref(), &cat_vi);
        assert!(state.status_warning.is_some());

        // Render in all 3 themes (Light, Dark, EyeComfort) with egui_kittest
        let themes = [
            ThemeSpec::Light,
            ThemeSpec::Dark,
            ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                strength: 60,
            },
        ];
        for spec in themes {
            let theme_ctl = ThemeController::new(spec, FontConfig::default());
            let mut inited = false;
            let cat = Catalog::for_locale(Locale::En);
            let mut st = state.clone();
            let builder = Harness::builder().with_accessibility_check(false);
            let mut harness = builder.build_ui(move |ui| {
                if !inited {
                    theme_ctl.install_fonts(ui.ctx());
                    theme_ctl.apply_style(ui.ctx());
                    inited = true;
                }
                let _ = show_mkv_import_dialog(ui, &mut st, &cat);
            });
            harness.run();
        }
    }
}
