use criterion::{Criterion, black_box, criterion_group, criterion_main};
use dialogs_model::DialogsState;

fn bench_dialogs_model(c: &mut Criterion) {
    c.bench_function("dialogs_model_field_update_and_validate", |b| {
        let mut state = DialogsState::default();
        state.find_replace.query.push_str("needle");
        state.jump_to.total_frames = 100_000;
        let mut i = 1u32;
        b.iter(|| {
            i = (i + 1) % 50_000 + 1;
            let r1 = state.shift_times.set_time_cs(black_box(i * 10));
            let r2 = state.tpp.set_adjacent_bias(black_box(0.75));
            let r3 = state.resample.set_dest_resolution(black_box(1920), 1080);
            let r4 = state.jump_to.set_frame(black_box(i));
            let _ = black_box((r1.is_ok(), r2.is_ok(), r3.is_ok(), r4.is_ok()));
        });
    });
}

criterion_group!(benches, bench_dialogs_model);
criterion_main!(benches);
