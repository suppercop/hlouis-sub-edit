//! Allocation-free fixed-capacity latency histogram and benchmark report helpers.

use std::fmt::Write as _;

const HIST_CAP: usize = 256;

/// Fixed-capacity ring buffer histogram tracking `p50`, `p95`, `p99`, and `worst` latencies in ms
/// with zero heap allocations on `record()`.
#[derive(Debug, Clone)]
pub struct LatencyHistogram {
    samples: [f64; HIST_CAP],
    count: usize,
    write_pos: usize,
    last: f64,
    worst: f64,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary percentiles computed from a [`LatencyHistogram`].
#[derive(Debug, Clone, Copy, Default)]
pub struct LatencyStats {
    pub samples: usize,
    pub last: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub worst: f64,
}

impl LatencyHistogram {
    /// Creates an empty histogram without heap allocation.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            samples: [0.0; HIST_CAP],
            count: 0,
            write_pos: 0,
            last: 0.0,
            worst: 0.0,
        }
    }

    /// Clears all recorded samples.
    pub fn clear(&mut self) {
        self.count = 0;
        self.write_pos = 0;
        self.last = 0.0;
        self.worst = 0.0;
    }

    /// Records a single latency observation in milliseconds (zero heap allocations).
    #[inline]
    pub fn record(&mut self, ms: f64) {
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        self.samples[self.write_pos] = ms;
        self.write_pos = (self.write_pos + 1) % HIST_CAP;
        if self.count < HIST_CAP {
            self.count += 1;
        }
        self.last = ms;
        if ms > self.worst {
            self.worst = ms;
        }
    }

    /// Returns the number of retained samples (`<= 256`).
    #[must_use]
    pub const fn len(&self) -> usize {
        self.count
    }

    /// Returns `true` when no samples have been recorded yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns the most recently recorded sample in ms.
    #[must_use]
    pub const fn last(&self) -> f64 {
        self.last
    }

    /// Computes `(p50, p95, p99, worst)` on the stack without allocating.
    #[must_use]
    pub fn summary(&self) -> LatencyStats {
        if self.count == 0 {
            return LatencyStats::default();
        }
        let mut sorted = self.samples;
        let active = &mut sorted[..self.count];
        active.sort_by(f64::total_cmp);
        LatencyStats {
            samples: self.count,
            last: self.last,
            p50: percentile_sorted(active, 50),
            p95: percentile_sorted(active, 95),
            p99: percentile_sorted(active, 99),
            worst: self.worst.max(active[self.count - 1]),
        }
    }
}

#[inline]
fn percentile_sorted(sorted: &[f64], pct: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) * pct).div_ceil(100);
    sorted[idx.min(sorted.len() - 1)]
}

/// One row of the `--bench-ui` report table.
pub struct BenchReportRow {
    pub area: String,
    pub budget: String,
    pub measured: String,
    pub status: &'static str,
}

/// Formats and prints the `--bench-ui` report table to stdout.
pub fn print_bench_table(mode: &str, rows: &[BenchReportRow]) {
    let mut out = String::with_capacity(2048);
    let _ = writeln!(out, "\n=== hlouis-sub-edit --bench-ui ({mode}) ===");
    let _ = writeln!(out, "| Metric | New Budget | Measured | Status |");
    let _ = writeln!(out, "|---|---|---|---|");
    for r in rows {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            r.area, r.budget, r.measured, r.status
        );
    }
    println!("{out}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_computes_exact_percentiles_and_worst() {
        let mut h = LatencyHistogram::new();
        for i in 1..=100 {
            h.record(f64::from(i));
        }
        let s = h.summary();
        assert_eq!(s.samples, 100);
        assert!((s.last - 100.0).abs() < 1e-9);
        assert!((s.p50 - 51.0).abs() < 1e-9);
        assert!((s.p95 - 96.0).abs() < 1e-9);
        assert!((s.p99 - 100.0).abs() < 1e-9);
        assert!((s.worst - 100.0).abs() < 1e-9);
    }
}
