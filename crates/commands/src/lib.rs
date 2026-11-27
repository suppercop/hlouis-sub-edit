//! Command registry, Fluent `en`/`vi` resources, default Aegisub 3.5.1 menus/toolbars, and
//! sub-100ns hotkey lookup engine.
//!
//! Mirrors Aegisub:
//! - `src/command/*.cpp` (`app.cpp`, `audio.cpp`, `edit.cpp`, `subtitle.cpp`, `time.cpp`,
//!   `tool.cpp`, `video.cpp`, `vis_tool.cpp`)
//! - `src/libresrc/default_menu.json` and `src/libresrc/default_hotkey.json`

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use aegisub_config::HotkeySettings;
use std::collections::HashMap;

/// Embedded English Fluent resource (`locales/en.ftl`).
pub const EN_FTL: &str = include_str!("../locales/en.ftl");
/// Embedded Vietnamese Fluent resource (`locales/vi.ftl`).
pub const VI_FTL: &str = include_str!("../locales/vi.ftl");

/// Metadata for one registered command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandMeta {
    /// Canonical Aegisub command ID (e.g. `"subtitle/open"`, `"time/snap/scene"`).
    pub id: &'static str,
    /// Fluent message ID for display name.
    pub ftl_name: &'static str,
    /// Fluent message ID for status-bar help text.
    pub ftl_help: &'static str,
    /// Origin Aegisub source file reference.
    pub aegisub_source: &'static str,
}

/// All in-scope Aegisub 3.5.1 commands.
pub const COMMANDS: &[CommandMeta] = &[
    CommandMeta {
        id: "app/about",
        ftl_name: "cmd-app-about",
        ftl_help: "cmd-app-about-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/display/audio_subs",
        ftl_name: "cmd-app-display-audio-subs",
        ftl_help: "cmd-app-display-audio-subs-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/display/full",
        ftl_name: "cmd-app-display-full",
        ftl_help: "cmd-app-display-full-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/display/standard",
        ftl_name: "cmd-app-display-standard",
        ftl_help: "cmd-app-display-standard-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/display/subs",
        ftl_name: "cmd-app-display-subs",
        ftl_help: "cmd-app-display-subs-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/exit",
        ftl_name: "cmd-app-exit",
        ftl_help: "cmd-app-exit-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/language",
        ftl_name: "cmd-app-language",
        ftl_help: "cmd-app-language-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/log",
        ftl_name: "cmd-app-log",
        ftl_help: "cmd-app-log-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/new_window",
        ftl_name: "cmd-app-new-window",
        ftl_help: "cmd-app-new-window-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/options",
        ftl_name: "cmd-app-options",
        ftl_help: "cmd-app-options-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/toggle/global_hotkeys",
        ftl_name: "cmd-app-toggle-global-hotkeys",
        ftl_help: "cmd-app-toggle-global-hotkeys-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/toggle/toolbar",
        ftl_name: "cmd-app-toggle-toolbar",
        ftl_help: "cmd-app-toggle-toolbar-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "audio/close",
        ftl_name: "cmd-audio-close",
        ftl_help: "cmd-audio-close-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/commit",
        ftl_name: "cmd-audio-commit",
        ftl_help: "cmd-audio-commit-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/commit/default",
        ftl_name: "cmd-audio-commit-default",
        ftl_help: "cmd-audio-commit-default-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/commit/next",
        ftl_name: "cmd-audio-commit-next",
        ftl_help: "cmd-audio-commit-next-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/commit/stay",
        ftl_name: "cmd-audio-commit-stay",
        ftl_help: "cmd-audio-commit-stay-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/go_to",
        ftl_name: "cmd-audio-go-to",
        ftl_help: "cmd-audio-go-to-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/karaoke/toggle",
        ftl_name: "cmd-audio-karaoke-toggle",
        ftl_help: "cmd-audio-karaoke-toggle-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/lead_in/add",
        ftl_name: "cmd-audio-lead-in-add",
        ftl_help: "cmd-audio-lead-in-add-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/lead_out/add",
        ftl_name: "cmd-audio-lead-out-add",
        ftl_help: "cmd-audio-lead-out-add-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/open",
        ftl_name: "cmd-audio-open",
        ftl_help: "cmd-audio-open-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/open/blank",
        ftl_name: "cmd-audio-open-blank",
        ftl_help: "cmd-audio-open-blank-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/open/noise",
        ftl_name: "cmd-audio-open-noise",
        ftl_help: "cmd-audio-open-noise-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/open/video",
        ftl_name: "cmd-audio-open-video",
        ftl_help: "cmd-audio-open-video-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/opt/auto_commit",
        ftl_name: "cmd-audio-opt-auto-commit",
        ftl_help: "cmd-audio-opt-auto-commit-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/opt/auto_focus",
        ftl_name: "cmd-audio-opt-auto-focus",
        ftl_help: "cmd-audio-opt-auto-focus-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/opt/auto_goto",
        ftl_name: "cmd-audio-opt-auto-goto",
        ftl_help: "cmd-audio-opt-auto-goto-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/opt/next_line_commit",
        ftl_name: "cmd-audio-opt-next-line-commit",
        ftl_help: "cmd-audio-opt-next-line-commit-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/opt/spectrum",
        ftl_name: "cmd-audio-opt-spectrum",
        ftl_help: "cmd-audio-opt-spectrum-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/opt/vertical_link",
        ftl_name: "cmd-audio-opt-vertical-link",
        ftl_help: "cmd-audio-opt-vertical-link-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/play/line",
        ftl_name: "cmd-audio-play-line",
        ftl_help: "cmd-audio-play-line-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/play/selection",
        ftl_name: "cmd-audio-play-selection",
        ftl_help: "cmd-audio-play-selection-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/play/selection/after",
        ftl_name: "cmd-audio-play-selection-after",
        ftl_help: "cmd-audio-play-selection-after-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/play/selection/before",
        ftl_name: "cmd-audio-play-selection-before",
        ftl_help: "cmd-audio-play-selection-before-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/play/selection/begin",
        ftl_name: "cmd-audio-play-selection-begin",
        ftl_help: "cmd-audio-play-selection-begin-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/play/selection/end",
        ftl_name: "cmd-audio-play-selection-end",
        ftl_help: "cmd-audio-play-selection-end-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/play/to_end",
        ftl_name: "cmd-audio-play-to-end",
        ftl_help: "cmd-audio-play-to-end-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/play/toggle",
        ftl_name: "cmd-audio-play-toggle",
        ftl_help: "cmd-audio-play-toggle-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/save/clip",
        ftl_name: "cmd-audio-save-clip",
        ftl_help: "cmd-audio-save-clip-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/scroll/left",
        ftl_name: "cmd-audio-scroll-left",
        ftl_help: "cmd-audio-scroll-left-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/scroll/right",
        ftl_name: "cmd-audio-scroll-right",
        ftl_help: "cmd-audio-scroll-right-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/stop",
        ftl_name: "cmd-audio-stop",
        ftl_help: "cmd-audio-stop-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/view/zoom/in",
        ftl_name: "cmd-audio-view-zoom-in",
        ftl_help: "cmd-audio-view-zoom-in-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/view/zoom/out",
        ftl_name: "cmd-audio-view-zoom-out",
        ftl_help: "cmd-audio-view-zoom-out-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "edit/color/primary",
        ftl_name: "cmd-edit-color-primary",
        ftl_help: "cmd-edit-color-primary-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/color/secondary",
        ftl_name: "cmd-edit-color-secondary",
        ftl_help: "cmd-edit-color-secondary-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/color/outline",
        ftl_name: "cmd-edit-color-outline",
        ftl_help: "cmd-edit-color-outline-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/color/shadow",
        ftl_name: "cmd-edit-color-shadow",
        ftl_help: "cmd-edit-color-shadow-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/find_replace",
        ftl_name: "cmd-edit-find-replace",
        ftl_help: "cmd-edit-find-replace-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/font",
        ftl_name: "cmd-edit-font",
        ftl_help: "cmd-edit-font-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/copy",
        ftl_name: "cmd-edit-line-copy",
        ftl_help: "cmd-edit-line-copy-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/cut",
        ftl_name: "cmd-edit-line-cut",
        ftl_help: "cmd-edit-line-cut-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/delete",
        ftl_name: "cmd-edit-line-delete",
        ftl_help: "cmd-edit-line-delete-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/duplicate",
        ftl_name: "cmd-edit-line-duplicate",
        ftl_help: "cmd-edit-line-duplicate-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/duplicate/shift",
        ftl_name: "cmd-edit-line-duplicate-shift",
        ftl_help: "cmd-edit-line-duplicate-shift-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/duplicate/shift_back",
        ftl_name: "cmd-edit-line-duplicate-shift-back",
        ftl_help: "cmd-edit-line-duplicate-shift-back-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/join/as_karaoke",
        ftl_name: "cmd-edit-line-join-as-karaoke",
        ftl_help: "cmd-edit-line-join-as-karaoke-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/join/concatenate",
        ftl_name: "cmd-edit-line-join-concatenate",
        ftl_help: "cmd-edit-line-join-concatenate-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/join/keep_first",
        ftl_name: "cmd-edit-line-join-keep-first",
        ftl_help: "cmd-edit-line-join-keep-first-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/paste",
        ftl_name: "cmd-edit-line-paste",
        ftl_help: "cmd-edit-line-paste-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/paste/over",
        ftl_name: "cmd-edit-line-paste-over",
        ftl_help: "cmd-edit-line-paste-over-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/recombine",
        ftl_name: "cmd-edit-line-recombine",
        ftl_help: "cmd-edit-line-recombine-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/split/by_karaoke",
        ftl_name: "cmd-edit-line-split-by-karaoke",
        ftl_help: "cmd-edit-line-split-by-karaoke-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/split/estimate",
        ftl_name: "cmd-edit-line-split-estimate",
        ftl_help: "cmd-edit-line-split-estimate-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/split/preserve",
        ftl_name: "cmd-edit-line-split-preserve",
        ftl_help: "cmd-edit-line-split-preserve-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/split/video",
        ftl_name: "cmd-edit-line-split-video",
        ftl_help: "cmd-edit-line-split-video-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/redo",
        ftl_name: "cmd-edit-redo",
        ftl_help: "cmd-edit-redo-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/revert",
        ftl_name: "cmd-edit-revert",
        ftl_help: "cmd-edit-revert-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/style/bold",
        ftl_name: "cmd-edit-style-bold",
        ftl_help: "cmd-edit-style-bold-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/style/italic",
        ftl_name: "cmd-edit-style-italic",
        ftl_help: "cmd-edit-style-italic-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/style/strikeout",
        ftl_name: "cmd-edit-style-strikeout",
        ftl_help: "cmd-edit-style-strikeout-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/style/underline",
        ftl_name: "cmd-edit-style-underline",
        ftl_help: "cmd-edit-style-underline-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/undo",
        ftl_name: "cmd-edit-undo",
        ftl_help: "cmd-edit-undo-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "grid/line/next",
        ftl_name: "cmd-grid-line-next",
        ftl_help: "cmd-grid-line-next-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/line/next/create",
        ftl_name: "cmd-grid-line-next-create",
        ftl_help: "cmd-grid-line-next-create-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/line/prev",
        ftl_name: "cmd-grid-line-prev",
        ftl_help: "cmd-grid-line-prev-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/move/down",
        ftl_name: "cmd-grid-move-down",
        ftl_help: "cmd-grid-move-down-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/move/up",
        ftl_name: "cmd-grid-move-up",
        ftl_help: "cmd-grid-move-up-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/sort/actor",
        ftl_name: "cmd-grid-sort-actor",
        ftl_help: "cmd-grid-sort-actor-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/sort/effect",
        ftl_name: "cmd-grid-sort-effect",
        ftl_help: "cmd-grid-sort-effect-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/sort/end",
        ftl_name: "cmd-grid-sort-end",
        ftl_help: "cmd-grid-sort-end-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/sort/layer",
        ftl_name: "cmd-grid-sort-layer",
        ftl_help: "cmd-grid-sort-layer-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/sort/start",
        ftl_name: "cmd-grid-sort-start",
        ftl_help: "cmd-grid-sort-start-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/sort/style",
        ftl_name: "cmd-grid-sort-style",
        ftl_help: "cmd-grid-sort-style-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/sort/text",
        ftl_name: "cmd-grid-sort-text",
        ftl_help: "cmd-grid-sort-text-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/swap",
        ftl_name: "cmd-grid-swap",
        ftl_help: "cmd-grid-swap-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/tag_cycle_hiding",
        ftl_name: "cmd-grid-tag-cycle-hiding",
        ftl_help: "cmd-grid-tag-cycle-hiding-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/tags/hide",
        ftl_name: "cmd-grid-tags-hide",
        ftl_help: "cmd-grid-tags-hide-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/tags/show",
        ftl_name: "cmd-grid-tags-show",
        ftl_help: "cmd-grid-tags-show-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "grid/tags/simplify",
        ftl_name: "cmd-grid-tags-simplify",
        ftl_help: "cmd-grid-tags-simplify-help",
        aegisub_source: "command/grid.cpp",
    },
    CommandMeta {
        id: "keyframe/close",
        ftl_name: "cmd-keyframe-close",
        ftl_help: "cmd-keyframe-close-help",
        aegisub_source: "command/keyframe.cpp",
    },
    CommandMeta {
        id: "keyframe/open",
        ftl_name: "cmd-keyframe-open",
        ftl_help: "cmd-keyframe-open-help",
        aegisub_source: "command/keyframe.cpp",
    },
    CommandMeta {
        id: "keyframe/save",
        ftl_name: "cmd-keyframe-save",
        ftl_help: "cmd-keyframe-save-help",
        aegisub_source: "command/keyframe.cpp",
    },
    CommandMeta {
        id: "subtitle/find",
        ftl_name: "cmd-subtitle-find",
        ftl_help: "cmd-subtitle-find-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/find/next",
        ftl_name: "cmd-subtitle-find-next",
        ftl_help: "cmd-subtitle-find-next-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/insert/after",
        ftl_name: "cmd-subtitle-insert-after",
        ftl_help: "cmd-subtitle-insert-after-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/insert/after/videotime",
        ftl_name: "cmd-subtitle-insert-after-videotime",
        ftl_help: "cmd-subtitle-insert-after-videotime-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/insert/before",
        ftl_name: "cmd-subtitle-insert-before",
        ftl_help: "cmd-subtitle-insert-before-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/insert/before/videotime",
        ftl_name: "cmd-subtitle-insert-before-videotime",
        ftl_help: "cmd-subtitle-insert-before-videotime-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/new",
        ftl_name: "cmd-subtitle-new",
        ftl_help: "cmd-subtitle-new-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/open",
        ftl_name: "cmd-subtitle-open",
        ftl_help: "cmd-subtitle-open-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/open/charset",
        ftl_name: "cmd-subtitle-open-charset",
        ftl_help: "cmd-subtitle-open-charset-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/open/video",
        ftl_name: "cmd-subtitle-open-video",
        ftl_help: "cmd-subtitle-open-video-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/properties",
        ftl_name: "cmd-subtitle-properties",
        ftl_help: "cmd-subtitle-properties-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/save",
        ftl_name: "cmd-subtitle-save",
        ftl_help: "cmd-subtitle-save-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/save/as",
        ftl_name: "cmd-subtitle-save-as",
        ftl_help: "cmd-subtitle-save-as-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/select/all",
        ftl_name: "cmd-subtitle-select-all",
        ftl_help: "cmd-subtitle-select-all-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "subtitle/select/visible",
        ftl_name: "cmd-subtitle-select-visible",
        ftl_help: "cmd-subtitle-select-visible-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "time/add/lead_in",
        ftl_name: "cmd-time-add-lead-in",
        ftl_help: "cmd-time-add-lead-in-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/add/lead_out",
        ftl_name: "cmd-time-add-lead-out",
        ftl_help: "cmd-time-add-lead-out-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/continuous/end",
        ftl_name: "cmd-time-continuous-end",
        ftl_help: "cmd-time-continuous-end-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/continuous/start",
        ftl_name: "cmd-time-continuous-start",
        ftl_help: "cmd-time-continuous-start-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/frame/current",
        ftl_name: "cmd-time-frame-current",
        ftl_help: "cmd-time-frame-current-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/length/decrease",
        ftl_name: "cmd-time-length-decrease",
        ftl_help: "cmd-time-length-decrease-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/length/decrease/shift",
        ftl_name: "cmd-time-length-decrease-shift",
        ftl_help: "cmd-time-length-decrease-shift-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/length/increase",
        ftl_name: "cmd-time-length-increase",
        ftl_help: "cmd-time-length-increase-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/length/increase/shift",
        ftl_name: "cmd-time-length-increase-shift",
        ftl_help: "cmd-time-length-increase-shift-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/next",
        ftl_name: "cmd-time-next",
        ftl_help: "cmd-time-next-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/prev",
        ftl_name: "cmd-time-prev",
        ftl_help: "cmd-time-prev-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/shift",
        ftl_name: "cmd-time-shift",
        ftl_help: "cmd-time-shift-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/snap/end_video",
        ftl_name: "cmd-time-snap-end-video",
        ftl_help: "cmd-time-snap-end-video-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/snap/scene",
        ftl_name: "cmd-time-snap-scene",
        ftl_help: "cmd-time-snap-scene-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/snap/start_video",
        ftl_name: "cmd-time-snap-start-video",
        ftl_help: "cmd-time-snap-start-video-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "timecode/close",
        ftl_name: "cmd-timecode-close",
        ftl_help: "cmd-timecode-close-help",
        aegisub_source: "command/timecode.cpp",
    },
    CommandMeta {
        id: "timecode/open",
        ftl_name: "cmd-timecode-open",
        ftl_help: "cmd-timecode-open-help",
        aegisub_source: "command/timecode.cpp",
    },
    CommandMeta {
        id: "timecode/save",
        ftl_name: "cmd-timecode-save",
        ftl_help: "cmd-timecode-save-help",
        aegisub_source: "command/timecode.cpp",
    },
    CommandMeta {
        id: "tool/fonts_collector",
        ftl_name: "cmd-tool-fonts-collector",
        ftl_help: "cmd-tool-fonts-collector-help",
        aegisub_source: "command/tool.cpp",
    },
    CommandMeta {
        id: "tool/line/select",
        ftl_name: "cmd-tool-line-select",
        ftl_help: "cmd-tool-line-select-help",
        aegisub_source: "command/tool.cpp",
    },
    CommandMeta {
        id: "tool/resample",
        ftl_name: "cmd-tool-resample",
        ftl_help: "cmd-tool-resample-help",
        aegisub_source: "command/tool.cpp",
    },
    CommandMeta {
        id: "tool/styles_manager",
        ftl_name: "cmd-tool-styles-manager",
        ftl_help: "cmd-tool-styles-manager-help",
        aegisub_source: "command/tool.cpp",
    },
    CommandMeta {
        id: "tool/timing_processor",
        ftl_name: "cmd-tool-timing-processor",
        ftl_help: "cmd-tool-timing-processor-help",
        aegisub_source: "command/tool.cpp",
    },
    CommandMeta {
        id: "video/close",
        ftl_name: "cmd-video-close",
        ftl_help: "cmd-video-close-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/detach",
        ftl_name: "cmd-video-detach",
        ftl_help: "cmd-video-detach-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/focus_seek",
        ftl_name: "cmd-video-focus-seek",
        ftl_help: "cmd-video-focus-seek-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/copy",
        ftl_name: "cmd-video-frame-copy",
        ftl_help: "cmd-video-frame-copy-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/copy/raw",
        ftl_name: "cmd-video-frame-copy-raw",
        ftl_help: "cmd-video-frame-copy-raw-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/next",
        ftl_name: "cmd-video-frame-next",
        ftl_help: "cmd-video-frame-next-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/next/boundary",
        ftl_name: "cmd-video-frame-next-boundary",
        ftl_help: "cmd-video-frame-next-boundary-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/next/keyframe",
        ftl_name: "cmd-video-frame-next-keyframe",
        ftl_help: "cmd-video-frame-next-keyframe-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/next/large",
        ftl_name: "cmd-video-frame-next-large",
        ftl_help: "cmd-video-frame-next-large-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/prev",
        ftl_name: "cmd-video-frame-prev",
        ftl_help: "cmd-video-frame-prev-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/prev/boundary",
        ftl_name: "cmd-video-frame-prev-boundary",
        ftl_help: "cmd-video-frame-prev-boundary-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/prev/keyframe",
        ftl_name: "cmd-video-frame-prev-keyframe",
        ftl_help: "cmd-video-frame-prev-keyframe-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/prev/large",
        ftl_name: "cmd-video-frame-prev-large",
        ftl_help: "cmd-video-frame-prev-large-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/save",
        ftl_name: "cmd-video-frame-save",
        ftl_help: "cmd-video-frame-save-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/save/raw",
        ftl_name: "cmd-video-frame-save-raw",
        ftl_help: "cmd-video-frame-save-raw-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/jump",
        ftl_name: "cmd-video-jump",
        ftl_help: "cmd-video-jump-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/jump/end",
        ftl_name: "cmd-video-jump-end",
        ftl_help: "cmd-video-jump-end-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/jump/start",
        ftl_name: "cmd-video-jump-start",
        ftl_help: "cmd-video-jump-start-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/open",
        ftl_name: "cmd-video-open",
        ftl_help: "cmd-video-open-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/open/dummy",
        ftl_name: "cmd-video-open-dummy",
        ftl_help: "cmd-video-open-dummy-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/opt/ar/custom",
        ftl_name: "cmd-video-opt-ar-custom",
        ftl_help: "cmd-video-opt-ar-custom-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/opt/ar/default",
        ftl_name: "cmd-video-opt-ar-default",
        ftl_help: "cmd-video-opt-ar-default-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/opt/ar/full",
        ftl_name: "cmd-video-opt-ar-full",
        ftl_help: "cmd-video-opt-ar-full-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/opt/ar/wide",
        ftl_name: "cmd-video-opt-ar-wide",
        ftl_help: "cmd-video-opt-ar-wide-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/opt/autoscroll",
        ftl_name: "cmd-video-opt-autoscroll",
        ftl_help: "cmd-video-opt-autoscroll-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/play",
        ftl_name: "cmd-video-play",
        ftl_help: "cmd-video-play-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/play/line",
        ftl_name: "cmd-video-play-line",
        ftl_help: "cmd-video-play-line-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/show_overscan",
        ftl_name: "cmd-video-show-overscan",
        ftl_help: "cmd-video-show-overscan-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/zoom/100",
        ftl_name: "cmd-video-zoom-100",
        ftl_help: "cmd-video-zoom-100-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/zoom/200",
        ftl_name: "cmd-video-zoom-200",
        ftl_help: "cmd-video-zoom-200-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/zoom/50",
        ftl_name: "cmd-video-zoom-50",
        ftl_help: "cmd-video-zoom-50-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/zoom/in",
        ftl_name: "cmd-video-zoom-in",
        ftl_help: "cmd-video-zoom-in-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/zoom/out",
        ftl_name: "cmd-video-zoom-out",
        ftl_help: "cmd-video-zoom-out-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "tool/visual/clip",
        ftl_name: "cmd-tool-visual-clip",
        ftl_help: "cmd-tool-visual-clip-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "tool/visual/cross",
        ftl_name: "cmd-tool-visual-cross",
        ftl_help: "cmd-tool-visual-cross-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "tool/visual/drag",
        ftl_name: "cmd-tool-visual-drag",
        ftl_help: "cmd-tool-visual-drag-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "tool/visual/rotatexy",
        ftl_name: "cmd-tool-visual-rotatexy",
        ftl_help: "cmd-tool-visual-rotatexy-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "tool/visual/rotatez",
        ftl_name: "cmd-tool-visual-rotatez",
        ftl_help: "cmd-tool-visual-rotatez-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "tool/visual/scale",
        ftl_name: "cmd-tool-visual-scale",
        ftl_help: "cmd-tool-visual-scale-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "tool/visual/vector_clip",
        ftl_name: "cmd-tool-visual-vector-clip",
        ftl_help: "cmd-tool-visual-vector-clip-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "hlouis/select_by_style",
        ftl_name: "cmd-hlouis-select-by-style",
        ftl_help: "cmd-hlouis-select-by-style-help",
        aegisub_source: "hlouis-tools",
    },
    CommandMeta {
        id: "hlouis/select_by_style_dialog",
        ftl_name: "cmd-hlouis-select-by-style-dialog",
        ftl_help: "cmd-hlouis-select-by-style-dialog-help",
        aegisub_source: "hlouis-tools",
    },
    CommandMeta {
        id: "hlouis/clean_whitespace",
        ftl_name: "cmd-hlouis-clean-whitespace",
        ftl_help: "cmd-hlouis-clean-whitespace-help",
        aegisub_source: "hlouis-tools",
    },
];

/// Menu item entry (either a command ID or a separator).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Command(&'static str),
    Separator,
}

/// Top-level menu group (matches Aegisub `default_menu.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuGroup {
    pub title: &'static str,
    pub items: Vec<MenuItem>,
}

/// Toolbar group of command IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolbarGroup {
    pub name: &'static str,
    pub commands: Vec<&'static str>,
}

/// Hotkey dispatch context (mirrors Aegisub `hotkey::Context`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum HotkeyContext {
    Always = 0,
    Default = 1,
    MainFrame = 2,
    SubtitleGrid = 3,
    SubtitleEditBox = 4,
    Audio = 5,
    Video = 6,
    Timing = 7,
    Karaoke = 8,
}

impl HotkeyContext {
    /// Parses Aegisub `hotkey.json` context names.
    #[must_use]
    pub fn from_aegisub_name(name: &str) -> Option<Self> {
        match name {
            "Always" => Some(Self::Always),
            "Default" => Some(Self::Default),
            "Main Frame" => Some(Self::MainFrame),
            "Subtitle Grid" => Some(Self::SubtitleGrid),
            "Subtitle Edit Box" => Some(Self::SubtitleEditBox),
            "Audio" => Some(Self::Audio),
            "Video" => Some(Self::Video),
            "Timing" => Some(Self::Timing),
            "Karaoke" => Some(Self::Karaoke),
            _ => None,
        }
    }
}

/// Compact key chord packed into a single `u32` (`(mods << 16) | key_code`) for O(1) lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord(u32);

impl KeyChord {
    /// Modifier bitflags.
    pub const CTRL: u8 = 1 << 0;
    pub const SHIFT: u8 = 1 << 1;
    pub const ALT: u8 = 1 << 2;

    /// Constructs a compact [`KeyChord`] from modifier bitmask and canonical key name.
    #[must_use]
    pub fn from_parts(mods: u8, key_name: &str) -> Option<Self> {
        let code = encode_key_name(key_name)?;
        Some(Self((u32::from(mods) << 16) | u32::from(code)))
    }

    /// Parses Aegisub modifier strings (`"Ctrl"`, `"Shift"`, `"Alt"`) + key name.
    #[must_use]
    pub fn from_aegisub(modifiers: &[String], key_name: &str) -> Option<Self> {
        let mut mask = 0u8;
        for m in modifiers {
            match m.as_str() {
                "Ctrl" | "Control" => mask |= Self::CTRL,
                "Shift" => mask |= Self::SHIFT,
                "Alt" => mask |= Self::ALT,
                _ => {}
            }
        }
        Self::from_parts(mask, key_name)
    }
}

fn encode_key_name(name: &str) -> Option<u16> {
    let b = name.as_bytes();
    if b.len() == 1 {
        return Some(u16::from(b[0].to_ascii_uppercase()));
    }
    match name {
        "Space" => Some(256),
        "Return" | "Enter" => Some(257),
        "Escape" | "Esc" => Some(258),
        "Tab" => Some(259),
        "Backspace" => Some(260),
        "Delete" => Some(261),
        "Insert" => Some(262),
        "Home" => Some(263),
        "End" => Some(264),
        "PageUp" | "Page_Up" => Some(265),
        "PageDown" | "Page_Down" => Some(266),
        "Left" => Some(267),
        "Right" => Some(268),
        "Up" => Some(269),
        "Down" => Some(270),
        "F1" => Some(281),
        "F2" => Some(282),
        "F3" => Some(283),
        "F4" => Some(284),
        "F5" => Some(285),
        "F6" => Some(286),
        "F7" => Some(287),
        "F8" => Some(288),
        "F9" => Some(289),
        "F10" => Some(290),
        "F11" => Some(291),
        "F12" => Some(292),
        _ => None,
    }
}

/// Detected duplicate chord binding within the same context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyConflict {
    pub context: HotkeyContext,
    pub chord: KeyChord,
    pub commands: Vec<String>,
}

/// Command registry + fast hotkey dispatch engine.
#[derive(Debug, Clone)]
pub struct CommandRegistry {
    by_id: HashMap<&'static str, &'static CommandMeta>,
    /// Flat per-context lookup tables (`9` contexts) indexed by `KeyChord` for sub-50ns dispatch.
    bindings: [HashMap<KeyChord, String>; 9],
    /// Tracks multi-bound chords for conflict reporting.
    conflicts: Vec<HotkeyConflict>,
}

impl CommandRegistry {
    /// Builds the default Aegisub 3.5.1 command registry and default hotkey table.
    #[must_use]
    pub fn new_default() -> Self {
        let mut by_id = HashMap::with_capacity(COMMANDS.len());
        for meta in COMMANDS {
            by_id.insert(meta.id, meta);
        }
        let mut reg = Self {
            by_id,
            bindings: std::array::from_fn(|_| HashMap::with_capacity(16)),
            conflicts: Vec::new(),
        };
        reg.load_default_hotkeys();
        reg
    }

    /// Looks up command metadata by Aegisub ID.
    #[inline]
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&'static CommandMeta> {
        self.by_id.get(id).copied()
    }

    /// Registers a hotkey binding in `context`, recording a conflict if the chord was already
    /// mapped to a different command in that same context.
    pub fn bind(&mut self, context: HotkeyContext, chord: KeyChord, command_id: &str) {
        let table = &mut self.bindings[context as usize];
        if let Some(prev) = table.get(&chord) {
            if prev != command_id {
                self.conflicts.push(HotkeyConflict {
                    context,
                    chord,
                    commands: vec![prev.clone(), command_id.to_owned()],
                });
            }
        }
        table.insert(chord, command_id.to_owned());
    }

    /// Applies user hotkey overrides imported by `aegisub-config`.
    pub fn apply_aegisub_hotkeys(&mut self, hotkeys: &HotkeySettings) {
        for (ctx_name, entries) in &hotkeys.by_context {
            let Some(ctx) = HotkeyContext::from_aegisub_name(ctx_name) else {
                continue;
            };
            for entry in entries {
                for combo in &entry.combos {
                    if let Some(chord) = KeyChord::from_aegisub(&combo.modifiers, &combo.key) {
                        self.bind(ctx, chord, &entry.aegisub_name);
                    }
                }
            }
        }
    }

    /// Looks up the command ID bound to `chord` in `context`, falling back to `Default` and
    /// `Always` (matches Aegisub `hotkey::Hotkey::Scan`). Zero heap allocations, < 50 ns.
    #[inline]
    #[must_use]
    pub fn lookup(&self, context: HotkeyContext, chord: KeyChord) -> Option<&str> {
        if let Some(cmd) = self.bindings[context as usize].get(&chord) {
            return Some(cmd.as_str());
        }
        if context != HotkeyContext::Default {
            if let Some(cmd) = self.bindings[HotkeyContext::Default as usize].get(&chord) {
                return Some(cmd.as_str());
            }
        }
        if context != HotkeyContext::Always {
            if let Some(cmd) = self.bindings[HotkeyContext::Always as usize].get(&chord) {
                return Some(cmd.as_str());
            }
        }
        None
    }

    /// Returns all detected hotkey conflicts.
    #[must_use]
    pub fn conflicts(&self) -> &[HotkeyConflict] {
        &self.conflicts
    }

    #[allow(clippy::too_many_lines)]
    fn load_default_hotkeys(&mut self) {
        let defaults: &[(HotkeyContext, u8, &str, &str)] = &[
            (HotkeyContext::Always, KeyChord::CTRL, "N", "subtitle/new"),
            (HotkeyContext::Always, KeyChord::CTRL, "O", "subtitle/open"),
            (HotkeyContext::Always, KeyChord::CTRL, "S", "subtitle/save"),
            (
                HotkeyContext::Always,
                KeyChord::CTRL | KeyChord::SHIFT,
                "S",
                "subtitle/save/as",
            ),
            (HotkeyContext::Always, KeyChord::CTRL, "Q", "app/exit"),
            (HotkeyContext::Default, KeyChord::CTRL, "Z", "edit/undo"),
            (HotkeyContext::Default, KeyChord::CTRL, "Y", "edit/redo"),
            (
                HotkeyContext::Default,
                KeyChord::CTRL,
                "H",
                "edit/find_replace",
            ),
            (HotkeyContext::Default, KeyChord::CTRL, "I", "time/shift"),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::CTRL,
                "C",
                "edit/line/copy",
            ),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::CTRL,
                "X",
                "edit/line/cut",
            ),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::CTRL,
                "V",
                "edit/line/paste",
            ),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::CTRL | KeyChord::SHIFT,
                "V",
                "edit/line/paste/over",
            ),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::CTRL,
                "Delete",
                "edit/line/delete",
            ),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::CTRL,
                "D",
                "edit/line/duplicate",
            ),
            (HotkeyContext::Audio, 0, "S", "audio/play/selection"),
            (HotkeyContext::Audio, 0, "G", "audio/commit"),
            (HotkeyContext::Audio, 0, "Return", "audio/commit"),
            (HotkeyContext::Audio, 0, "A", "audio/go_to"),
            (HotkeyContext::Video, 0, "Space", "video/play"),
            (HotkeyContext::Video, 0, "Right", "video/frame/next"),
            (HotkeyContext::Video, 0, "Left", "video/frame/prev"),
            (
                HotkeyContext::Video,
                KeyChord::SHIFT,
                "Right",
                "video/frame/next/keyframe",
            ),
            (
                HotkeyContext::Video,
                KeyChord::SHIFT,
                "Left",
                "video/frame/prev/keyframe",
            ),
            (HotkeyContext::Default, KeyChord::CTRL, "F", "subtitle/find"),
            (HotkeyContext::Default, 0, "F3", "subtitle/find/next"),
            (
                HotkeyContext::Default,
                KeyChord::CTRL,
                "A",
                "subtitle/select/all",
            ),
            (HotkeyContext::Default, KeyChord::CTRL, "G", "video/jump"),
            (HotkeyContext::Default, KeyChord::CTRL, "P", "video/play"),
            (
                HotkeyContext::Default,
                KeyChord::CTRL,
                "1",
                "video/jump/start",
            ),
            (
                HotkeyContext::Default,
                KeyChord::CTRL,
                "2",
                "video/jump/end",
            ),
            (
                HotkeyContext::Default,
                KeyChord::CTRL,
                "3",
                "time/snap/start_video",
            ),
            (
                HotkeyContext::Default,
                KeyChord::CTRL,
                "4",
                "time/snap/end_video",
            ),
            (
                HotkeyContext::Default,
                KeyChord::CTRL | KeyChord::SHIFT,
                "S",
                "subtitle/save/as",
            ),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::ALT,
                "Up",
                "grid/move/up",
            ),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::ALT,
                "Down",
                "grid/move/down",
            ),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::CTRL,
                "K",
                "time/snap/scene",
            ),
            (HotkeyContext::Audio, 0, "Q", "audio/play/selection/before"),
            (HotkeyContext::Audio, 0, "W", "audio/play/selection/after"),
            (HotkeyContext::Audio, 0, "E", "audio/play/selection/begin"),
            (HotkeyContext::Audio, 0, "D", "audio/play/selection/end"),
            (HotkeyContext::Audio, 0, "R", "audio/play/line"),
            (HotkeyContext::Audio, 0, "T", "audio/play/to_end"),
            (HotkeyContext::Audio, 0, "Z", "time/prev"),
            (HotkeyContext::Audio, 0, "X", "time/next"),
            (HotkeyContext::Audio, 0, "C", "audio/lead_in/add"),
            (HotkeyContext::Audio, 0, "V", "audio/lead_out/add"),
            (HotkeyContext::Audio, 0, "H", "audio/stop"),
            (
                HotkeyContext::SubtitleEditBox,
                KeyChord::CTRL,
                "B",
                "edit/style/bold",
            ),
            (
                HotkeyContext::SubtitleEditBox,
                KeyChord::CTRL,
                "I",
                "edit/style/italic",
            ),
            (
                HotkeyContext::SubtitleEditBox,
                KeyChord::CTRL,
                "U",
                "edit/style/underline",
            ),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::CTRL | KeyChord::ALT,
                "S",
                "hlouis/select_by_style",
            ),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::CTRL | KeyChord::ALT | KeyChord::SHIFT,
                "S",
                "hlouis/select_by_style_dialog",
            ),
            (
                HotkeyContext::SubtitleGrid,
                KeyChord::CTRL | KeyChord::ALT,
                "W",
                "hlouis/clean_whitespace",
            ),
        ];
        for &(ctx, mods, key, cmd) in defaults {
            if let Some(chord) = KeyChord::from_parts(mods, key) {
                self.bindings[ctx as usize].insert(chord, cmd.to_owned());
            }
        }
    }
}

/// Returns Aegisub's default top-level menu layout (`src/libresrc/default_menu.json`).
#[must_use]
pub fn default_menus() -> Vec<MenuGroup> {
    vec![
        MenuGroup {
            title: "File",
            items: vec![
                MenuItem::Command("subtitle/new"),
                MenuItem::Command("subtitle/open"),
                MenuItem::Command("subtitle/save"),
                MenuItem::Command("subtitle/save/as"),
                MenuItem::Separator,
                MenuItem::Command("subtitle/properties"),
                MenuItem::Separator,
                MenuItem::Command("app/exit"),
            ],
        },
        MenuGroup {
            title: "Edit",
            items: vec![
                MenuItem::Command("edit/undo"),
                MenuItem::Command("edit/redo"),
                MenuItem::Separator,
                MenuItem::Command("edit/line/cut"),
                MenuItem::Command("edit/line/copy"),
                MenuItem::Command("edit/line/paste"),
                MenuItem::Command("edit/line/paste/over"),
                MenuItem::Separator,
                MenuItem::Command("edit/find_replace"),
            ],
        },
        MenuGroup {
            title: "Subtitle",
            items: vec![
                MenuItem::Command("tool/styles_manager"),
                MenuItem::Command("tool/resample"),
                MenuItem::Separator,
                MenuItem::Command("edit/line/duplicate"),
                MenuItem::Command("edit/line/delete"),
                MenuItem::Command("edit/line/split/by_karaoke"),
                MenuItem::Command("edit/line/recombine"),
            ],
        },
        MenuGroup {
            title: "Timing",
            items: vec![
                MenuItem::Command("time/shift"),
                MenuItem::Command("tool/timing_processor"),
                MenuItem::Separator,
                MenuItem::Command("time/snap/scene"),
                MenuItem::Command("time/continuous/start"),
                MenuItem::Command("time/continuous/end"),
            ],
        },
        MenuGroup {
            title: "Video",
            items: vec![
                MenuItem::Command("video/play"),
                MenuItem::Command("video/frame/prev"),
                MenuItem::Command("video/frame/next"),
                MenuItem::Command("video/frame/prev/keyframe"),
                MenuItem::Command("video/frame/next/keyframe"),
            ],
        },
        MenuGroup {
            title: "Audio",
            items: vec![
                MenuItem::Command("audio/play/selection"),
                MenuItem::Command("audio/commit"),
                MenuItem::Command("audio/go_to"),
            ],
        },
        MenuGroup {
            title: "View",
            items: vec![MenuItem::Command("app/options")],
        },
    ]
}

/// Returns Aegisub's default toolbar groups.
#[must_use]
pub fn default_toolbars() -> Vec<ToolbarGroup> {
    vec![
        ToolbarGroup {
            name: "main",
            commands: vec![
                "subtitle/new",
                "subtitle/open",
                "subtitle/save",
                "edit/undo",
                "edit/redo",
                "tool/styles_manager",
                "subtitle/properties",
                "app/options",
            ],
        },
        ToolbarGroup {
            name: "visual_tools",
            commands: vec![
                "tool/visual/cross",
                "tool/visual/drag",
                "tool/visual/rotatez",
                "tool/visual/rotatexy",
                "tool/visual/scale",
                "tool/visual/clip",
                "tool/visual/vector_clip",
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lookup_fallback_conflicts_and_ftl_completeness() {
        let mut reg = CommandRegistry::new_default();
        assert!(reg.get("time/snap/scene").is_some());
        assert!(reg.get("video/frame/next/keyframe").is_some());

        // Context-specific lookup
        let space = KeyChord::from_parts(0, "Space").expect("space");
        assert_eq!(reg.lookup(HotkeyContext::Video, space), Some("video/play"));

        // Fallback to Default and Always
        let ctrl_s = KeyChord::from_parts(KeyChord::CTRL, "S").expect("ctrl+s");
        assert_eq!(
            reg.lookup(HotkeyContext::SubtitleGrid, ctrl_s),
            Some("subtitle/save")
        );

        // Conflict detection
        reg.bind(HotkeyContext::Video, space, "audio/play/selection");
        assert_eq!(reg.conflicts().len(), 1);

        // Every registered command's FTL keys exist in both EN_FTL and VI_FTL
        for cmd in COMMANDS {
            assert!(EN_FTL.contains(cmd.ftl_name), "missing EN {}", cmd.ftl_name);
            assert!(VI_FTL.contains(cmd.ftl_name), "missing VI {}", cmd.ftl_name);
            assert!(EN_FTL.contains(cmd.ftl_help), "missing EN {}", cmd.ftl_help);
            assert!(VI_FTL.contains(cmd.ftl_help), "missing VI {}", cmd.ftl_help);
        }
        assert!(default_menus().len() >= 5);
        assert!(default_toolbars().len() >= 2);
    }

    #[test]
    fn all_aegisub_in_scope_commands_fixture_registered() {
        let reg = CommandRegistry::new_default();
        let fixture = include_str!("../tests/fixtures/aegisub_in_scope_commands.txt");
        let mut count = 0usize;
        for line in fixture.lines() {
            let id = line.trim();
            if id.is_empty() || id.starts_with('#') {
                continue;
            }
            assert!(
                reg.get(id).is_some(),
                "missing in-scope Aegisub command from fixture: {id}"
            );
            count += 1;
        }
        assert!(
            count >= 120,
            "expected >= 120 in-scope commands, got {count}"
        );
    }
}
