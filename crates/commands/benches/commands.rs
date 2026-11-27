use commands::{CommandRegistry, HotkeyContext, KeyChord};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_commands(c: &mut Criterion) {
    c.bench_function("commands_build_registry", |b| {
        b.iter(|| {
            let reg = CommandRegistry::new_default();
            black_box(reg);
        });
    });

    let reg = CommandRegistry::new_default();
    let chord = KeyChord::from_parts(KeyChord::CTRL, "D").expect("chord");

    c.bench_function("commands_hotkey_lookup", |b| {
        b.iter(|| {
            let cmd = reg.lookup(black_box(HotkeyContext::SubtitleGrid), black_box(chord));
            black_box(cmd);
        });
    });
}

criterion_group!(benches, bench_commands);
criterion_main!(benches);
