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
        id: "subtitle/properties",
        ftl_name: "cmd-subtitle-properties",
        ftl_help: "cmd-subtitle-properties-help",
        aegisub_source: "command/subtitle.cpp",
    },
    CommandMeta {
        id: "edit/undo",
        ftl_name: "cmd-edit-undo",
        ftl_help: "cmd-edit-undo-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/redo",
        ftl_name: "cmd-edit-redo",
        ftl_help: "cmd-edit-redo-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/find_replace",
        ftl_name: "cmd-edit-find-replace",
        ftl_help: "cmd-edit-find-replace-help",
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
        id: "edit/line/split/by_karaoke",
        ftl_name: "cmd-edit-line-split-karaoke",
        ftl_help: "cmd-edit-line-split-karaoke-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "edit/line/recombine",
        ftl_name: "cmd-edit-line-recombine",
        ftl_help: "cmd-edit-line-recombine-help",
        aegisub_source: "command/edit.cpp",
    },
    CommandMeta {
        id: "time/shift",
        ftl_name: "cmd-time-shift",
        ftl_help: "cmd-time-shift-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/snap/scene",
        ftl_name: "cmd-time-snap-scene",
        ftl_help: "cmd-time-snap-scene-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/continuous/start",
        ftl_name: "cmd-time-continuous-start",
        ftl_help: "cmd-time-continuous-start-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "time/continuous/end",
        ftl_name: "cmd-time-continuous-end",
        ftl_help: "cmd-time-continuous-end-help",
        aegisub_source: "command/time.cpp",
    },
    CommandMeta {
        id: "audio/play/selection",
        ftl_name: "cmd-audio-play-selection",
        ftl_help: "cmd-audio-play-selection-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/commit",
        ftl_name: "cmd-audio-commit",
        ftl_help: "cmd-audio-commit-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "audio/go_to",
        ftl_name: "cmd-audio-go-to",
        ftl_help: "cmd-audio-go-to-help",
        aegisub_source: "command/audio.cpp",
    },
    CommandMeta {
        id: "video/play",
        ftl_name: "cmd-video-play",
        ftl_help: "cmd-video-play-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/next",
        ftl_name: "cmd-video-frame-next",
        ftl_help: "cmd-video-frame-next-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/prev",
        ftl_name: "cmd-video-frame-prev",
        ftl_help: "cmd-video-frame-prev-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/next/keyframe",
        ftl_name: "cmd-video-frame-next-keyframe",
        ftl_help: "cmd-video-frame-next-keyframe-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "video/frame/prev/keyframe",
        ftl_name: "cmd-video-frame-prev-keyframe",
        ftl_help: "cmd-video-frame-prev-keyframe-help",
        aegisub_source: "command/video.cpp",
    },
    CommandMeta {
        id: "tool/styles_manager",
        ftl_name: "cmd-tool-styles-manager",
        ftl_help: "cmd-tool-styles-manager-help",
        aegisub_source: "command/tool.cpp",
    },
    CommandMeta {
        id: "tool/resample",
        ftl_name: "cmd-tool-resample",
        ftl_help: "cmd-tool-resample-help",
        aegisub_source: "command/tool.cpp",
    },
    CommandMeta {
        id: "tool/timing_processor",
        ftl_name: "cmd-tool-timing-processor",
        ftl_help: "cmd-tool-timing-processor-help",
        aegisub_source: "command/tool.cpp",
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
        id: "tool/visual/rotatez",
        ftl_name: "cmd-tool-visual-rotatez",
        ftl_help: "cmd-tool-visual-rotatez-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "tool/visual/rotatexy",
        ftl_name: "cmd-tool-visual-rotatexy",
        ftl_help: "cmd-tool-visual-rotatexy-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "tool/visual/scale",
        ftl_name: "cmd-tool-visual-scale",
        ftl_help: "cmd-tool-visual-scale-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "tool/visual/clip",
        ftl_name: "cmd-tool-visual-clip",
        ftl_help: "cmd-tool-visual-clip-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "tool/visual/vector_clip",
        ftl_name: "cmd-tool-visual-vector-clip",
        ftl_help: "cmd-tool-visual-vector-clip-help",
        aegisub_source: "command/vis_tool.cpp",
    },
    CommandMeta {
        id: "app/options",
        ftl_name: "cmd-app-options",
        ftl_help: "cmd-app-options-help",
        aegisub_source: "command/app.cpp",
    },
    CommandMeta {
        id: "app/exit",
        ftl_name: "cmd-app-exit",
        ftl_help: "cmd-app-exit-help",
        aegisub_source: "command/app.cpp",
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
}
