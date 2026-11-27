use criterion::{Criterion, black_box, criterion_group, criterion_main};
use egui::{Pos2, RawInput, Rect, Vec2};
use playback::Clock;
use ui_shell::{AppShell, DummyAudioProvider, DummyVideoSupplier, generate_demo_script};

struct ZeroClock;
impl Clock for ZeroClock {
    fn now_ns(&self) -> u64 {
        0
    }
}

fn bench_ui_shell(c: &mut Criterion) {
    let script = generate_demo_script(200_000);
    let mut shell = AppShell::new(script);
    shell.grid.model.click(100_000);
    shell.grid.scroll_to_active();
    let vid = DummyVideoSupplier::new(128, 72, 24_000);
    let aud = DummyAudioProvider {
        duration_ms: 1_000_000,
    };
    let ctx = egui::Context::default();
    let raw = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
        ..Default::default()
    };

    // Warm up 3 frames (fonts + textures).
    for _ in 0..3 {
        let _ = ctx.run_ui(raw.clone(), |ui| {
            shell.show(ui, &vid, &aud, &ZeroClock);
        });
    }

    c.bench_function("ui_shell_full_frame_200k_lines", |b| {
        b.iter(|| {
            let out = ctx.run_ui(raw.clone(), |ui| {
                shell.show(ui, &vid, &aud, &ZeroClock);
            });
            black_box(out.shapes.len());
        });
    });
}

criterion_group!(benches, bench_ui_shell);
criterion_main!(benches);
