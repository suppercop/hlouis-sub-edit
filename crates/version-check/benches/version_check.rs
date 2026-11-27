use criterion::{Criterion, black_box, criterion_group, criterion_main};
use version_check::SemVer;

fn bench_version_check(c: &mut Criterion) {
    c.bench_function("version_check_semver_parse", |b| {
        b.iter(|| black_box(SemVer::parse(black_box("v1.23.45-rc.1"))));
    });
}

criterion_group!(benches, bench_version_check);
criterion_main!(benches);
