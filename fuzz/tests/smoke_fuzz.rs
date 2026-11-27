//! Deterministic fuzz smoke test running on stable Rust.
//!
//! Exercises all 11 parser fuzz entrypoints across generated seeds plus 500 PRNG-mutated
//! byte streams per target (5,500+ total fuzz executions) to verify zero panics.

use std::fs;
use std::path::PathBuf;

struct XorShift64(u64);

impl XorShift64 {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn mutate(&mut self, seed: &[u8]) -> Vec<u8> {
        let mut buf = seed.to_vec();
        if buf.is_empty() {
            let len = usize::try_from(self.next_u64() % 64).unwrap_or(0);
            buf.resize(len, 0);
        }
        let ops = 1 + usize::try_from(self.next_u64() % 6).unwrap_or(0);
        for _ in 0..ops {
            match self.next_u64() % 4 {
                0 if !buf.is_empty() => {
                    let idx = usize::try_from(self.next_u64()).unwrap_or(0) % buf.len();
                    buf[idx] ^= u8::try_from(self.next_u64() & 0xFF).unwrap_or(0xFF);
                }
                1 if buf.len() < 4096 => {
                    let idx = usize::try_from(self.next_u64()).unwrap_or(0) % (buf.len() + 1);
                    let byte = u8::try_from(self.next_u64() & 0xFF).unwrap_or(0);
                    buf.insert(idx, byte);
                }
                2 if !buf.is_empty() => {
                    let idx = usize::try_from(self.next_u64()).unwrap_or(0) % buf.len();
                    buf.remove(idx);
                }
                _ => {
                    let keep = usize::try_from(self.next_u64()).unwrap_or(0) % (buf.len() + 1);
                    buf.truncate(keep);
                }
            }
        }
        buf
    }
}

fn load_seeds(target: &str) -> Vec<Vec<u8>> {
    let dir = PathBuf::from(env!("OUT_DIR")).join("seeds").join(target);
    let mut seeds = vec![Vec::new()];
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            if let Ok(bytes) = fs::read(e.path()) {
                seeds.push(bytes);
            }
        }
    }
    seeds
}

fn run_target(name: &str, f: fn(&[u8]), rng: &mut XorShift64) {
    let seeds = load_seeds(name);
    for seed in &seeds {
        f(seed);
    }
    for i in 0..500usize {
        let base = &seeds[i % seeds.len()];
        let mutated = rng.mutate(base);
        f(&mutated);
    }
}

#[test]
fn smoke_fuzz_all_eleven_parsers_zero_panics() {
    let mut rng = XorShift64(0x5EED_CAFE_BABE_1337);
    run_target("fuzz_ass_core", hlouis_fuzz::fuzz_ass_core, &mut rng);
    run_target("fuzz_mkv_subs", hlouis_fuzz::fuzz_mkv_subs, &mut rng);
    run_target("fuzz_keyframes", hlouis_fuzz::fuzz_keyframes, &mut rng);
    run_target(
        "fuzz_aegisub_config",
        hlouis_fuzz::fuzz_aegisub_config,
        &mut rng,
    );
    run_target("fuzz_settings", hlouis_fuzz::fuzz_settings, &mut rng);
    run_target("fuzz_project", hlouis_fuzz::fuzz_project, &mut rng);
    run_target(
        "fuzz_styles_catalog",
        hlouis_fuzz::fuzz_styles_catalog,
        &mut rng,
    );
    run_target("fuzz_karaskel", hlouis_fuzz::fuzz_karaskel, &mut rng);
    run_target(
        "fuzz_syntax_highlight",
        hlouis_fuzz::fuzz_syntax_highlight,
        &mut rng,
    );
    run_target(
        "fuzz_visual_tools",
        hlouis_fuzz::fuzz_visual_tools,
        &mut rng,
    );
    run_target(
        "fuzz_template_doctor",
        hlouis_fuzz::fuzz_template_doctor,
        &mut rng,
    );
}
