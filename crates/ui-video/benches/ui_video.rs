use criterion::{Criterion, black_box, criterion_group, criterion_main};
use egui::{Pos2, Rect, Vec2};
use theme::ThemeSpec;
use ui_video::{ActiveVisualTool, VideoPanelState, paint_visual_overlay};
use visual_tools::Vec2 as V2;

fn bench_ui_video(c: &mut Criterion) {
    let mut state = VideoPanelState::new(1920, 1080);
    state.active_tool = ActiveVisualTool::VectorClip;
    for i in 0..500u32 {
        let x = f64::from(i) * 3.5;
        let y = 540.0 + (x * 0.02).sin() * 200.0;
        state.vclip_path.append_line(V2::new(x, y));
    }
    let pal = theme::palette(ThemeSpec::Dark);
    let ctx = egui::Context::default();
    let raw_input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 500.0))),
        ..Default::default()
    };
    let vr = Rect::from_min_size(Pos2::new(40.0, 20.0), Vec2::new(640.0, 360.0));

    c.bench_function("ui_video_overlay_500_point_clip", |b| {
        b.iter(|| {
            let out = ctx.run_ui(raw_input.clone(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let painter = ui.painter_at(vr);
                    paint_visual_overlay(&painter, vr, 1920.0, 1080.0, &mut state, &pal, 1.0);
                });
            });
            black_box(out.shapes.len());
        });
    });
}

criterion_group!(benches, bench_ui_video);
criterion_main!(benches);
