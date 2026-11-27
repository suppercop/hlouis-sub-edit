use aegisub_config::import_aegisub_json;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

const CONFIG_FIXTURE: &str = include_str!("../tests/fixtures/config.json");
const HOTKEY_FIXTURE: &str = include_str!("../tests/fixtures/hotkey.json");

fn bench_aegisub_config(c: &mut Criterion) {
    c.bench_function("aegisub_config_parse_and_map_both", |b| {
        b.iter(|| {
            let res = import_aegisub_json(black_box(CONFIG_FIXTURE), black_box(HOTKEY_FIXTURE))
                .expect("import");
            black_box(res);
        });
    });
}

criterion_group!(benches, bench_aegisub_config);
criterion_main!(benches);
