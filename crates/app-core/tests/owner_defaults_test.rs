//! Verifies first-run Aegisub 3.5.1 (3.2.2-style) `config.json` + `hotkey.json` detection,
//! ask-before-importing prompt, unmapped-hotkey report, and live UI column/theme parity.

#![forbid(unsafe_code)]

use app_core::{EditorApp, build_demo_script};
use egui::Vec2;
use egui_kittest::Harness;
use playback::Clock;
use std::fs;
use tempfile::tempdir;

struct ZeroClock;
impl Clock for ZeroClock {
    fn now_ns(&self) -> u64 {
        0
    }
}

#[test]
fn first_run_aegisub_322_import_prompts_applies_columns_and_reports_unmapped() {
    let dir = tempdir().expect("tempdir");
    let cfg_fixture = include_str!("fixtures/aegisub_322_config.json");
    let hk_fixture = include_str!("fixtures/aegisub_322_hotkey.json");
    fs::write(dir.path().join("config.json"), cfg_fixture).expect("write config.json");
    fs::write(dir.path().join("hotkey.json"), hk_fixture).expect("write hotkey.json");

    let mut app = EditorApp::new(build_demo_script(20));
    // Start with Layer/Actor shown to prove import hides them to match the owner's 3.2.2 config.
    app.shell.grid.model.columns.layer = true;
    app.shell.grid.model.columns.actor = true;
    app.shell.audio.spectrum_mode = false;

    // 1. Detection must open the prompt WITHOUT silently modifying settings yet.
    assert!(app.check_first_run_aegisub_import(Some(dir.path())));
    assert!(app.first_run_import.prompt_open);
    assert!(app.shell.grid.model.columns.layer);

    // 2. Accepting applies all settings and reports unmapped hotkeys cleanly.
    let report = app.accept_aegisub_import().expect("accept import");
    assert!(report.config_loaded);
    assert!(report.hotkeys_loaded);
    assert!(!app.first_run_import.prompt_open);
    assert!(
        report
            .unmapped_hotkeys
            .iter()
            .any(|u| u.aegisub_name == "custom/legacy_lua_macro_unmapped"),
        "expected unmapped legacy hotkey in report: {:?}",
        report.unmapped_hotkeys
    );

    // 3. Verify owner's 3.2.2-style columns & audio spectrum applied to the live UI.
    let cols = app.shell.grid.model.columns;
    assert!(!cols.layer);
    assert!(!cols.actor);
    assert!(!cols.effect);
    assert!(!cols.margin_l);
    assert!(!cols.margin_r);
    assert!(!cols.margin_v);
    assert!(cols.line_number && cols.start && cols.end && cols.cps && cols.style && cols.text);
    assert!(app.shell.audio.spectrum_mode);

    // 4. Render the resulting UI cleanly through egui_kittest.
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1280.0, 800.0))
        .with_accessibility_check(false)
        .build_ui(move |ui| {
            app.show(ui, &ZeroClock);
        });
    harness.run();
}
