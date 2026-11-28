use audio_peaks::PeakBucket;
use audio_view::TimelineMarkerIndex;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use i18n_catalog::{Catalog, Locale};
use theme::ThemeSpec;
use ui_audio::{AudioColumnProvider, AudioPanelState, show_audio_panel};

struct BenchProvider;
impl AudioColumnProvider for BenchProvider {
    fn duration_ms(&self) -> i64 {
        7_200_000
    }
    fn query_columns_into(
        &self,
        _start_ms: i64,
        _end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        out.clear();
        for i in 0..width_px {
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let v = ((i & 63) * 400) as i16;
            out.push(PeakBucket {
                min: -v,
                max: v,
                rms: v.unsigned_abs(),
            });
        }
    }
}

// Realistic 10 s PCM signal for first-screen and incremental scroll benchmarks at 2000 px.
struct PcmBenchProvider {
    samples: Vec<i16>,
    sr: u32,
}
#[allow(clippy::cast_precision_loss)]
impl AudioColumnProvider for PcmBenchProvider {
    fn duration_ms(&self) -> i64 {
        30_000
    }
    fn sample_rate(&self) -> u32 {
        self.sr
    }
    fn read_mono_i16(&self, start_sample: usize, count: usize, out: &mut Vec<i16>) {
        out.clear();
        if start_sample >= self.samples.len() {
            return;
        }
        let end = (start_sample + count).min(self.samples.len());
        out.extend_from_slice(&self.samples[start_sample..end]);
    }
    fn query_columns_into(
        &self,
        start_ms: i64,
        end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        out.clear();
        let span = (end_ms - start_ms).max(1) as f64;
        let sr_f = f64::from(self.sr);
        let n = self.samples.len();
        for col in 0..width_px {
            let t0 = start_ms as f64 + (col as f64 / width_px.max(1) as f64) * span;
            let t1 = start_ms as f64 + ((col + 1) as f64 / width_px.max(1) as f64) * span;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let s0 = ((t0.max(0.0) * sr_f) / 1_000.0) as usize;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let s1 = (((t1.max(0.0) * sr_f) / 1_000.0) as usize).max(s0 + 1);
            if s0 >= n {
                out.push(PeakBucket {
                    min: 0,
                    max: 0,
                    rms: 0,
                });
                continue;
            }
            let mut mn = i16::MAX;
            let mut mx = i16::MIN;
            let mut sq = 0u64;
            let slice = &self.samples[s0..s1.min(n)];
            for &s in slice {
                mn = mn.min(s);
                mx = mx.max(s);
                let v = i64::from(s);
                sq = sq.saturating_add((v * v).unsigned_abs());
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let rms = ((sq as f64 / slice.len().max(1) as f64).sqrt()) as u16;
            out.push(PeakBucket {
                min: mn,
                max: mx,
                rms,
            });
        }
    }
}

#[allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn bench_ui_audio(c: &mut Criterion) {
    let mut state = AudioPanelState::new(7_200_000);
    let provider = BenchProvider;
    let kfs: Vec<i64> = (0..2_000).map(|i| i * 3_500).collect();
    let bounds: Vec<i64> = (0..20_000).map(|i| i * 350).collect();
    let markers = TimelineMarkerIndex::new(kfs, bounds);
    let pal = theme::palette(ThemeSpec::Dark);
    let cat = Catalog::for_locale(Locale::En);

    let ctx = egui::Context::default();
    ctx.set_fonts(ui_theme::build_font_definitions(
        &ui_theme::FontConfig::default(),
    ));
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(2040.0, 260.0),
        )),
        ..Default::default()
    };

    // Warm up one frame so the waveform texture is uploaded once.
    let _ = ctx.run_ui(raw_input.clone(), |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            let _ = show_audio_panel(
                ui, &mut state, &provider, &markers, None, None, &pal, 1, &cat,
            );
        });
    });
    assert_eq!(state.texture_uploads(), 1);

    c.bench_function("ui_audio_frame_2000px_20000_lines", |b| {
        b.iter(|| {
            let out = ctx.run_ui(raw_input.clone(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let act = show_audio_panel(
                        ui, &mut state, &provider, &markers, None, None, &pal, 1, &cat,
                    );
                    let _ = black_box(act);
                });
            });
            black_box(out.shapes.len());
        });
    });
    assert_eq!(
        state.texture_uploads(),
        1,
        "zero texture re-upload when unchanged"
    );

    let sr = 16_000u32;
    let samples: Vec<i16> = (0..(sr as usize * 30))
        .map(|i| {
            let t = i as f32 / sr as f32;
            let burst = if ((t * 2.0) as usize).is_multiple_of(3) {
                0.08
            } else {
                0.65
            };
            let sig = burst
                * (0.55 * (std::f32::consts::TAU * 180.0 * t).sin()
                    + 0.30 * (std::f32::consts::TAU * 720.0 * t).sin()
                    + 0.15 * (std::f32::consts::TAU * 2_100.0 * t).sin());
            #[allow(clippy::cast_possible_truncation)]
            let v = (sig * 28_000.0) as i16;
            v
        })
        .collect();
    let pcm_provider = PcmBenchProvider { samples, sr };

    c.bench_function("ui_audio_first_screen_10s_2000px", |b| {
        b.iter(|| {
            let mut st = AudioPanelState::new(30_000);
            st.viewport.ms_per_px = 5.0; // 10 s across 2000 px
            let mut out = ctx.run_ui(raw_input.clone(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let act = show_audio_panel(
                        ui,
                        &mut st,
                        &pcm_provider,
                        &markers,
                        None,
                        None,
                        &pal,
                        1,
                        &cat,
                    );
                    let _ = black_box(act);
                });
            });
            out.textures_delta.clear();
            black_box(out.shapes.len());
        });
    });

    let mut scroll_state = AudioPanelState::new(30_000);
    scroll_state.viewport.ms_per_px = 5.0; // 10 s across 2000 px
    let mut warm = ctx.run_ui(raw_input.clone(), |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            let _ = show_audio_panel(
                ui,
                &mut scroll_state,
                &pcm_provider,
                &markers,
                None,
                None,
                &pal,
                1,
                &cat,
            );
        });
    });
    warm.textures_delta.clear();
    let mut scroll_step = 0i64;
    c.bench_function("ui_audio_scroll_10s_2000px", |b| {
        b.iter(|| {
            // Scroll by 8 px (40 ms at 5 ms/px) back and forth within 0..10_000 ms.
            scroll_step = (scroll_step + 1) % 200;
            let pos = if scroll_step < 100 {
                scroll_step * 40
            } else {
                (200 - scroll_step) * 40
            };
            scroll_state.viewport.scroll_ms = pos as f64;
            let mut out = ctx.run_ui(raw_input.clone(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let act = show_audio_panel(
                        ui,
                        &mut scroll_state,
                        &pcm_provider,
                        &markers,
                        None,
                        None,
                        &pal,
                        1,
                        &cat,
                    );
                    let _ = black_box(act);
                });
            });
            out.textures_delta.clear();
            black_box(out.shapes.len());
        });
    });
}

criterion_group!(benches, bench_ui_audio);
criterion_main!(benches);
