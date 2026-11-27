use criterion::{black_box, criterion_group, criterion_main, Criterion};
use grid_model::{FontMetrics, GridModel, OverlapIndex, RowFlags, RowTiming};

struct FixedFont;
impl FontMetrics for FixedFont {
    fn row_height(&self) -> f32 {
        20.0
    }
}

fn build_200k_rows() -> Vec<RowTiming> {
    (0..200_000u32)
        .map(|i| {
            let s = i * 5;
            let e = if i % 50 == 0 { s + 12 } else { s + 4 };
            RowTiming {
                start_cs: s,
                end_cs: e,
                is_comment: i % 100 == 0,
            }
        })
        .collect()
}

fn bench_grid_model(c: &mut Criterion) {
    let rows = build_200k_rows();

    c.bench_function("grid_model_overlap_rebuild_200k", |b| {
        let mut idx = OverlapIndex::default();
        b.iter(|| {
            idx.rebuild(black_box(&rows));
            black_box(idx.len());
        });
    });

    let mut grid = GridModel::new();
    grid.load_rows(&rows);
    grid.scroll_y = 50_000.0;

    let mut vis_buf: Vec<RowFlags> = Vec::with_capacity(64);
    c.bench_function("grid_model_visible_60_rows_flags", |b| {
        b.iter(|| {
            grid.visible_rows_into(1180.0, &FixedFont, Some(12_500), &mut vis_buf);
            black_box(vis_buf.len());
        });
    });

    c.bench_function("grid_model_incremental_overlap_update_1_row", |b| {
        let mut t = 500_000u32;
        b.iter(|| {
            t = t.wrapping_add(3);
            grid.update_row_timing(
                100_000,
                RowTiming {
                    start_cs: t,
                    end_cs: t + 10,
                    is_comment: false,
                },
            );
            black_box(t);
        });
    });

    c.bench_function("grid_model_select_all_200k", |b| {
        b.iter(|| {
            grid.select_all();
            black_box(grid.selected_count());
        });
    });
}

criterion_group!(benches, bench_grid_model);
criterion_main!(benches);
