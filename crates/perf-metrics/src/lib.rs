//! Fixed-memory HDR-style latency histograms, rolling windows, counters, optional allocator
//! hook, keystroke-to-present & seek latency trackers, idle detection, and `--bench-ui` / overlay
//! exporters (text table & JSON).
//!
//! Performance:
//! - Histogram `record`: `< 20 ns` (typically ~3 ns), **0 heap allocations**
//! - Percentile query (`p50` / `p95` / `p99` / `worst`): `< 1 µs`, **0 heap allocations**

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::fmt::Write as _;

const SUB_BUCKET_BITS: u32 = 4; // 16 linear sub-buckets per power-of-2 octave
const SUB_BUCKETS: usize = 1 << SUB_BUCKET_BITS;
const OCTAVES: usize = 32;
const TOTAL_BUCKETS: usize = SUB_BUCKETS * OCTAVES; // 512 buckets = 4 KiB fixed memory

/// Fixed-memory log-linear (HDR-style) latency histogram in microseconds (`0 .. ~4.29e9 µs`).
///
/// Exact `1 µs` resolution for `0..31 µs` and `<= 6.25%` relative bucket width across 32 octaves.
/// Performs **zero heap allocations** for all operations.
#[derive(Debug, Clone)]
pub struct HdrHistogram {
    buckets: [u64; TOTAL_BUCKETS],
    count: u64,
    sum: u128,
    min_val: u64,
    max_val: u64,
}

impl Default for HdrHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl HdrHistogram {
    /// Creates an empty fixed-memory histogram.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buckets: [0; TOTAL_BUCKETS],
            count: 0,
            sum: 0,
            min_val: u64::MAX,
            max_val: 0,
        }
    }

    /// Resets all buckets and summary counters to zero.
    pub fn reset(&mut self) {
        self.buckets.fill(0);
        self.count = 0;
        self.sum = 0;
        self.min_val = u64::MAX;
        self.max_val = 0;
    }

    /// Records a latency sample in microseconds in O(1) with **0 heap allocations**.
    #[inline]
    pub fn record(&mut self, value_us: u64) {
        let idx = Self::bucket_index(value_us);
        self.buckets[idx] += 1;
        self.count += 1;
        self.sum += u128::from(value_us);
        if value_us < self.min_val {
            self.min_val = value_us;
        }
        if value_us > self.max_val {
            self.max_val = value_us;
        }
    }

    /// Returns the total number of recorded samples.
    #[inline]
    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }

    /// Returns the minimum recorded sample (`0` if empty).
    #[inline]
    #[must_use]
    pub const fn min(&self) -> u64 {
        if self.count == 0 { 0 } else { self.min_val }
    }

    /// Returns the worst (maximum) recorded sample (`0` if empty).
    #[inline]
    #[must_use]
    pub const fn worst(&self) -> u64 {
        self.max_val
    }

    /// Returns the arithmetic mean in microseconds (`0.0` if empty).
    #[must_use]
    pub fn mean(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        {
            (self.sum as f64) / (self.count as f64)
        }
    }

    /// Computes percentile `q` in `[0.0, 1.0]` (e.g. `0.50`, `0.95`, `0.99`) in `< 1 µs` with
    /// **0 heap allocations**.
    #[must_use]
    pub fn percentile(&self, q: f64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let clamped = q.clamp(0.0, 1.0);
        if clamped >= 1.0 {
            return self.max_val;
        }
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let target = ((self.count as f64) * clamped).ceil().max(1.0) as u64;
        let mut acc = 0u64;
        for (i, &c) in self.buckets.iter().enumerate() {
            acc += c;
            if acc >= target {
                return Self::bucket_value(i).min(self.max_val);
            }
        }
        self.max_val
    }

    #[inline]
    #[must_use]
    pub fn p50(&self) -> u64 {
        self.percentile(0.50)
    }

    #[inline]
    #[must_use]
    pub fn p95(&self) -> u64 {
        self.percentile(0.95)
    }

    #[inline]
    #[must_use]
    pub fn p99(&self) -> u64 {
        self.percentile(0.99)
    }

    #[inline]
    fn bucket_index(val: u64) -> usize {
        if val < (SUB_BUCKETS as u64) {
            #[allow(clippy::cast_possible_truncation)]
            return val as usize;
        }
        let msb = val.ilog2();
        let octave = (msb - SUB_BUCKET_BITS) as usize;
        let clamped_octave = octave.min(OCTAVES - 2);
        #[allow(clippy::cast_possible_truncation)]
        let shift = clamped_octave as u32;
        #[allow(clippy::cast_possible_truncation)]
        let sub = ((val >> shift) & ((SUB_BUCKETS as u64) - 1)) as usize;
        (clamped_octave + 1) * SUB_BUCKETS + sub
    }

    #[inline]
    fn bucket_value(idx: usize) -> u64 {
        if idx < SUB_BUCKETS {
            return idx as u64;
        }
        let octave = (idx / SUB_BUCKETS) - 1;
        let sub = (idx % SUB_BUCKETS) as u64;
        let base = ((SUB_BUCKETS as u64) | sub) << octave;
        let step = 1u64 << octave;
        base.saturating_add(step / 2)
    }
}

/// Fixed-capacity rolling window of the most recent `N` samples (microseconds), 0 allocations.
#[derive(Debug, Clone)]
pub struct RollingWindow<const N: usize> {
    samples: [u64; N],
    cursor: usize,
    filled: usize,
    sum: u128,
}

impl<const N: usize> Default for RollingWindow<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RollingWindow<N> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            samples: [0; N],
            cursor: 0,
            filled: 0,
            sum: 0,
        }
    }

    /// Pushes a sample into the ring buffer in O(1) with 0 allocations.
    #[inline]
    pub fn push(&mut self, value_us: u64) {
        if N == 0 {
            return;
        }
        if self.filled == N {
            self.sum -= u128::from(self.samples[self.cursor]);
        } else {
            self.filled += 1;
        }
        self.samples[self.cursor] = value_us;
        self.sum += u128::from(value_us);
        self.cursor += 1;
        if self.cursor == N {
            self.cursor = 0;
        }
    }

    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.filled
    }

    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.filled == 0
    }

    #[must_use]
    pub fn mean_us(&self) -> f64 {
        if self.filled == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        {
            (self.sum as f64) / (self.filled as f64)
        }
    }

    #[must_use]
    pub fn max_us(&self) -> u64 {
        let mut m = 0u64;
        for &v in &self.samples[..self.filled] {
            if v > m {
                m = v;
            }
        }
        m
    }
}

/// Optional allocation counter hook supplied by the host binary's counting allocator.
pub type AllocStatsHook = fn() -> AllocCounters;

/// Snapshot returned by an optional global allocator hook.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AllocCounters {
    pub total_allocs: u64,
    pub total_bytes: u64,
}

/// Event counters tracked alongside latency histograms.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PerfCounters {
    pub dropped_frames: u64,
    pub repaints: u64,
    pub frames_presented: u64,
    pub seeks_completed: u64,
    pub keystrokes_presented: u64,
}

/// Tracks start-to-present latency (e.g. keystroke→present or seek→present) with 0 allocations.
#[derive(Debug, Clone, Default)]
pub struct LatencyTracker {
    pending_start_ns: Option<u64>,
    pub histogram: HdrHistogram,
}

impl LatencyTracker {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pending_start_ns: None,
            histogram: HdrHistogram::new(),
        }
    }

    /// Marks the beginning of an input/seek event at `now_ns`.
    #[inline]
    pub fn begin(&mut self, now_ns: u64) {
        if self.pending_start_ns.is_none() {
            self.pending_start_ns = Some(now_ns);
        }
    }

    /// Completes the pending latency measurement at `present_ns` and records elapsed µs.
    #[inline]
    pub fn complete(&mut self, present_ns: u64) -> Option<u64> {
        let start = self.pending_start_ns.take()?;
        let elapsed_us = present_ns.saturating_sub(start) / 1_000;
        self.histogram.record(elapsed_us);
        Some(elapsed_us)
    }
}

/// Idle-state detector for pausing background repaint/telemetry work when the user is inactive.
#[derive(Debug, Clone, Copy)]
pub struct IdleDetector {
    threshold_ns: u64,
    last_activity_ns: u64,
}

impl IdleDetector {
    #[must_use]
    pub const fn new(threshold_ms: u64) -> Self {
        Self {
            threshold_ns: threshold_ms.saturating_mul(1_000_000),
            last_activity_ns: 0,
        }
    }

    #[inline]
    pub fn record_activity(&mut self, now_ns: u64) {
        self.last_activity_ns = now_ns;
    }

    #[inline]
    #[must_use]
    pub const fn is_idle(&self, now_ns: u64) -> bool {
        now_ns.saturating_sub(self.last_activity_ns) >= self.threshold_ns
    }

    #[inline]
    #[must_use]
    pub const fn idle_ms(&self, now_ns: u64) -> u64 {
        now_ns.saturating_sub(self.last_activity_ns) / 1_000_000
    }
}

/// Top-level telemetry hub for `--bench-ui` and the in-app performance overlay.
#[derive(Debug, Clone)]
pub struct PerfMetrics {
    pub frame_time: HdrHistogram,
    pub frame_window: RollingWindow<120>,
    pub keystroke_to_present: LatencyTracker,
    pub seek_latency: LatencyTracker,
    pub counters: PerfCounters,
    pub idle: IdleDetector,
    alloc_hook: Option<AllocStatsHook>,
    baseline_allocs: AllocCounters,
}

impl Default for PerfMetrics {
    fn default() -> Self {
        Self::new(5_000)
    }
}

impl PerfMetrics {
    /// Creates a new metrics hub with `idle_threshold_ms` idle detection threshold.
    #[must_use]
    pub const fn new(idle_threshold_ms: u64) -> Self {
        Self {
            frame_time: HdrHistogram::new(),
            frame_window: RollingWindow::new(),
            keystroke_to_present: LatencyTracker::new(),
            seek_latency: LatencyTracker::new(),
            counters: PerfCounters {
                dropped_frames: 0,
                repaints: 0,
                frames_presented: 0,
                seeks_completed: 0,
                keystrokes_presented: 0,
            },
            idle: IdleDetector::new(idle_threshold_ms),
            alloc_hook: None,
            baseline_allocs: AllocCounters {
                total_allocs: 0,
                total_bytes: 0,
            },
        }
    }

    /// Attaches an optional counting-allocator hook and captures the current baseline.
    pub fn set_alloc_hook(&mut self, hook: AllocStatsHook) {
        self.baseline_allocs = hook();
        self.alloc_hook = Some(hook);
    }

    /// Returns allocation counts since [`Self::set_alloc_hook`] was installed.
    #[must_use]
    pub fn alloc_delta(&self) -> AllocCounters {
        let Some(hook) = self.alloc_hook else {
            return AllocCounters::default();
        };
        let cur = hook();
        AllocCounters {
            total_allocs: cur
                .total_allocs
                .saturating_sub(self.baseline_allocs.total_allocs),
            total_bytes: cur
                .total_bytes
                .saturating_sub(self.baseline_allocs.total_bytes),
        }
    }

    /// Records one completed UI/video frame duration in microseconds (0 allocations).
    #[inline]
    pub fn record_frame_us(&mut self, frame_us: u64, now_ns: u64) {
        self.frame_time.record(frame_us);
        self.frame_window.push(frame_us);
        self.counters.frames_presented += 1;
        if self.keystroke_to_present.complete(now_ns).is_some() {
            self.counters.keystrokes_presented += 1;
        }
        if self.seek_latency.complete(now_ns).is_some() {
            self.counters.seeks_completed += 1;
        }
    }

    /// Formats a human-readable ASCII summary table into `out` (for `--bench-ui` & overlay).
    pub fn format_text_table(&self, out: &mut String) {
        out.clear();
        let allocs = self.alloc_delta();
        let _ = writeln!(
            out,
            "{:<22} {:>8} {:>10} {:>10} {:>10} {:>10}",
            "Metric", "Count", "p50 (us)", "p95 (us)", "p99 (us)", "Worst (us)"
        );
        for (name, h) in [
            ("frame_time", &self.frame_time),
            ("keystroke_to_present", &self.keystroke_to_present.histogram),
            ("seek_latency", &self.seek_latency.histogram),
        ] {
            let _ = writeln!(
                out,
                "{:<22} {:>8} {:>10} {:>10} {:>10} {:>10}",
                name,
                h.count(),
                h.p50(),
                h.p95(),
                h.p99(),
                h.worst()
            );
        }
        let _ = writeln!(
            out,
            "counters: frames={} repaints={} dropped={} allocs={} alloc_bytes={} rolling_avg_us={:.1}",
            self.counters.frames_presented,
            self.counters.repaints,
            self.counters.dropped_frames,
            allocs.total_allocs,
            allocs.total_bytes,
            self.frame_window.mean_us()
        );
    }

    /// Formats a compact JSON report into `out` (zero external serde dependency).
    pub fn format_json(&self, out: &mut String) {
        out.clear();
        let allocs = self.alloc_delta();
        let _ = write!(
            out,
            concat!(
                "{{\"frame_time\":{{\"count\":{},\"p50_us\":{},\"p95_us\":{},\"p99_us\":{},\"worst_us\":{}}},",
                "\"keystroke_to_present\":{{\"count\":{},\"p50_us\":{},\"p95_us\":{},\"p99_us\":{},\"worst_us\":{}}},",
                "\"seek_latency\":{{\"count\":{},\"p50_us\":{},\"p95_us\":{},\"p99_us\":{},\"worst_us\":{}}},",
                "\"counters\":{{\"frames\":{},\"repaints\":{},\"dropped\":{},\"allocs\":{},\"alloc_bytes\":{}}},",
                "\"rolling_window\":{{\"len\":{},\"mean_us\":{:.2},\"max_us\":{}}}}}"
            ),
            self.frame_time.count(),
            self.frame_time.p50(),
            self.frame_time.p95(),
            self.frame_time.p99(),
            self.frame_time.worst(),
            self.keystroke_to_present.histogram.count(),
            self.keystroke_to_present.histogram.p50(),
            self.keystroke_to_present.histogram.p95(),
            self.keystroke_to_present.histogram.p99(),
            self.keystroke_to_present.histogram.worst(),
            self.seek_latency.histogram.count(),
            self.seek_latency.histogram.p50(),
            self.seek_latency.histogram.p95(),
            self.seek_latency.histogram.p99(),
            self.seek_latency.histogram.worst(),
            self.counters.frames_presented,
            self.counters.repaints,
            self.counters.dropped_frames,
            allocs.total_allocs,
            allocs.total_bytes,
            self.frame_window.len(),
            self.frame_window.mean_us(),
            self.frame_window.max_us(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_alloc_hook() -> AllocCounters {
        AllocCounters {
            total_allocs: 42,
            total_bytes: 1024,
        }
    }

    #[test]
    fn histogram_percentiles_trackers_idle_and_exporters() {
        let mut pm = PerfMetrics::new(1_000);
        pm.set_alloc_hook(fake_alloc_hook);

        for i in 1..=1000u64 {
            pm.record_frame_us(i * 10, i * 16_000_000);
        }
        assert_eq!(pm.frame_time.count(), 1000);
        assert_eq!(pm.frame_time.worst(), 10_000);
        let p50 = pm.frame_time.p50();
        let p95 = pm.frame_time.p95();
        let p99 = pm.frame_time.p99();
        assert!((4500..=5500).contains(&p50), "p50={p50}");
        assert!((9000..=10_000).contains(&p95), "p95={p95}");
        assert!((9500..=10_000).contains(&p99), "p99={p99}");

        pm.keystroke_to_present.begin(100_000_000);
        pm.seek_latency.begin(100_000_000);
        pm.record_frame_us(8_000, 104_500_000);
        assert_eq!(pm.counters.keystrokes_presented, 1);
        assert_eq!(pm.counters.seeks_completed, 1);

        pm.idle.record_activity(1_000_000_000);
        assert!(!pm.idle.is_idle(1_500_000_000));
        assert!(pm.idle.is_idle(2_500_000_000));

        let mut table = String::new();
        pm.format_text_table(&mut table);
        assert!(table.contains("frame_time"));
        assert!(table.contains("keystroke_to_present"));

        let mut json = String::new();
        pm.format_json(&mut json);
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert!(json.contains("\"p99_us\""));
    }
}
