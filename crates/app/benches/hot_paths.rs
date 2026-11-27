use criterion::{black_box, criterion_group, criterion_main, Criterion};
use eframe::egui;
use egui::{FontId, Pos2, Rect, Vec2};
use hlouis_sub_edit::grid::GridState;
use hlouis_sub_edit::syntax;
use hlouis_sub_edit::theme::ThemePalette;
use std::time::Duration;

fn bench_grid_and_syntax(c: &mut Criterion) {
    let mut group = c.benchmark_group("app_hot_paths");
    group
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(3))
        .sample_size(20);

    group.bench_function("grid_200k_generate", |b| {
        b.iter(|| black_box(GridState::generate(200_000)));
    });

    let mut grid_sel = GridState::generate(200_000);
    group.bench_function("grid_200k_select_all", |b| {
        b.iter(|| {
            grid_sel.selected.clear();
            grid_sel.select_all();
            black_box(grid_sel.selected.len());
        });
    });

    group.sample_size(40);
    let ctx = egui::Context::default();
    let mut grid_scroll = GridState::generate(200_000);
    let palette = ThemePalette::dark();
    let headers = ["#", "Start", "End", "CPS", "Style", "Text"];

    let warm_raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 420.0))),
        ..Default::default()
    };
    let mut warm_out = ctx.run_ui(warm_raw, |ui| {
        hlouis_sub_edit::render_grid_table(ui, &mut grid_scroll, &palette, headers);
    });
    warm_out.textures_delta.clear();

    let mut wheel_step = 0usize;
    group.bench_function("grid_200k_scroll_frame", |b| {
        b.iter(|| {
            wheel_step = wheel_step.wrapping_add(1);
            let dy = if wheel_step.is_multiple_of(2) {
                -44.0
            } else {
                44.0
            };
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 420.0))),
                events: vec![
                    egui::Event::PointerMoved(Pos2::new(400.0, 200.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: Vec2::new(0.0, dy),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                hlouis_sub_edit::render_grid_table(ui, &mut grid_scroll, &palette, headers);
            });
            out.textures_delta.clear();
            black_box(out.shapes.len());
        });
    });

    let sample_line = r"{\an5\fs48\frz-12\t(0,400,\fscx115\c&H00FFFF&)}Xin chào Tiếng Việt\N{\k30}Ka{\kf45}raoke — 日本語テスト{\p1\pos(100,100)}m 0 0 l 120 0 120 40{\p0}";
    let font = FontId::monospace(15.0);
    group.sample_size(60);
    group.bench_function("syntax_layout_job", |b| {
        b.iter(|| {
            let job = syntax::build_layout_job(black_box(sample_line), &font, 720.0, &palette);
            black_box(job.sections.len());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_grid_and_syntax);
criterion_main!(benches);
