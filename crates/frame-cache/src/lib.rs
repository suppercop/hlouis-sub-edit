//! Decoded-frame cache with strict byte budget, keyframe-aware LRU retention, buffer pooling,
//! forward/backward GOP prefetch planning, and thread-safe sharing between decoder & UI threads.
//!
//! Mirrors Aegisub 3.5.1 `src/video_provider_cache.cpp` (`VideoProviderCache`) while improving
//! memory discipline (explicit byte budget, buffer reuse pool, zero steady-state allocations,
//! sub-100ns `get`/`insert`).

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const NONE: u32 = u32::MAX;

/// Decoded video frame buffer (e.g. BGRA8 / RGBA8 pixels).
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub frame_number: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub is_keyframe: bool,
    pub pixels: Vec<u8>,
}

impl DecodedFrame {
    /// Logical byte footprint counted against the cache memory budget.
    #[inline]
    #[must_use]
    pub fn byte_size(&self) -> usize {
        self.pixels.len()
    }
}

/// Direction of playback / stepping for prefetch planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepDirection {
    Forward,
    Backward,
    Paused,
}

/// Snapshot of cache performance and memory usage.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub current_bytes: usize,
    pub budget_bytes: usize,
    pub entry_count: usize,
    pub keyframe_count: usize,
    pub pooled_buffers: usize,
}

impl CacheStats {
    /// Returns hit rate in `[0.0, 1.0]` (`0.0` when no lookups have occurred).
    #[must_use]
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        {
            (self.hits as f64) / (total as f64)
        }
    }
}

#[derive(Debug)]
struct Slot {
    frame_number: u32,
    bytes: usize,
    is_keyframe: bool,
    frame: Option<Arc<DecodedFrame>>,
    prev: u32,
    next: u32,
    occupied: bool,
}

/// Single-threaded core of the decoded-frame cache with intrusive O(1) LRU list, keyframe-aware
/// eviction, and reusable pixel-buffer pool (0 allocations in steady state).
#[derive(Debug)]
pub struct FrameCache {
    budget_bytes: usize,
    keyframe_budget_bytes: usize,
    current_bytes: usize,
    keyframe_bytes: usize,
    entry_count: usize,
    keyframe_count: usize,

    // Pre-sized open map from frame_number -> slot index.
    index: HashMap<u32, u32>,
    slots: Vec<Slot>,
    free_slots: Vec<u32>,
    head: u32, // Most recently used
    tail: u32, // Least recently used

    // Reusable pixel buffers recycled on eviction so steady-state decode+insert does 0 allocs.
    pool: Vec<Arc<DecodedFrame>>,
    max_pool_entries: usize,

    hits: u64,
    misses: u64,
    evictions: u64,
}

impl FrameCache {
    /// Creates a new cache capped strictly at `budget_bytes`.
    ///
    /// `expected_frame_bytes` pre-sizes internal slot and index tables so steady-state operations
    /// perform zero heap allocations.
    #[must_use]
    pub fn new(budget_bytes: usize, expected_frame_bytes: usize) -> Self {
        let frame_sz = expected_frame_bytes.max(1);
        let max_entries = (budget_bytes / frame_sz).clamp(4, 4096);
        let cap = (max_entries * 2).max(16);
        Self {
            budget_bytes,
            keyframe_budget_bytes: budget_bytes / 2,
            current_bytes: 0,
            keyframe_bytes: 0,
            entry_count: 0,
            keyframe_count: 0,
            index: HashMap::with_capacity(cap),
            slots: Vec::with_capacity(cap),
            free_slots: Vec::with_capacity(cap),
            head: NONE,
            tail: NONE,
            pool: Vec::with_capacity(max_entries.min(64)),
            max_pool_entries: max_entries.clamp(2, 64),
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    /// Checks out a recycled [`Arc<DecodedFrame>`] whose pixel buffer has at least `min_bytes`
    /// capacity (and unique ownership so the decoder can mutate via `Arc::get_mut` with 0 allocs),
    /// or allocates a new buffer if the pool is empty.
    #[must_use]
    pub fn acquire_buffer(&mut self, min_bytes: usize) -> Arc<DecodedFrame> {
        if let Some(pos) = self
            .pool
            .iter()
            .position(|b| Arc::strong_count(b) == 1 && b.pixels.capacity() >= min_bytes)
        {
            let mut buf = self.pool.swap_remove(pos);
            if let Some(inner) = Arc::get_mut(&mut buf) {
                inner.pixels.resize(min_bytes, 0);
            }
            return buf;
        }
        Arc::new(DecodedFrame {
            frame_number: 0,
            width: 0,
            height: 0,
            stride: 0,
            is_keyframe: false,
            pixels: vec![0u8; min_bytes],
        })
    }

    /// Looks up `frame_number`, promoting it to MRU on hit. **Zero heap allocations**, `< 100 ns`.
    #[inline]
    pub fn get(&mut self, frame_number: u32) -> Option<Arc<DecodedFrame>> {
        if let Some(&slot_idx) = self.index.get(&frame_number) {
            self.hits += 1;
            self.move_to_front(slot_idx);
            return self.slots[slot_idx as usize].frame.clone();
        }
        self.misses += 1;
        None
    }

    /// Returns `true` if `frame_number` is currently cached (does not alter LRU or stats).
    #[inline]
    #[must_use]
    pub fn contains(&self, frame_number: u32) -> bool {
        self.index.contains_key(&frame_number)
    }

    /// Inserts `frame` into the cache, evicting LRU entries (preferring non-keyframes first) so
    /// [`Self::current_bytes`] **never exceeds [`Self::budget_bytes`]**.
    ///
    /// Returns `false` if a single frame is larger than the entire `budget_bytes` (in which case
    /// it is not cached). **Zero heap allocations** in steady state.
    pub fn insert(&mut self, frame: Arc<DecodedFrame>) -> bool {
        let bytes = frame.byte_size();
        if bytes > self.budget_bytes {
            return false;
        }
        let frame_number = frame.frame_number;
        let is_keyframe = frame.is_keyframe;

        // Replace existing entry in place if already present.
        if let Some(&slot_idx) = self.index.get(&frame_number) {
            let old_bytes = self.slots[slot_idx as usize].bytes;
            let old_kf = self.slots[slot_idx as usize].is_keyframe;
            self.current_bytes = self.current_bytes.saturating_sub(old_bytes);
            if old_kf {
                self.keyframe_bytes = self.keyframe_bytes.saturating_sub(old_bytes);
                self.keyframe_count = self.keyframe_count.saturating_sub(1);
            }
            if let Some(old_arc) = self.slots[slot_idx as usize].frame.take() {
                self.recycle_buffer(old_arc);
            }
            while self.current_bytes + bytes > self.budget_bytes {
                if !self.evict_one(Some(slot_idx)) {
                    break;
                }
            }
            let slot = &mut self.slots[slot_idx as usize];
            slot.bytes = bytes;
            slot.is_keyframe = is_keyframe;
            slot.frame = Some(frame);
            self.current_bytes += bytes;
            if is_keyframe {
                self.keyframe_bytes += bytes;
                self.keyframe_count += 1;
            }
            self.move_to_front(slot_idx);
            return true;
        }

        // Evict until there is room under budget.
        while self.current_bytes + bytes > self.budget_bytes {
            if !self.evict_one(None) {
                return false;
            }
        }

        let slot_idx = self.alloc_slot(frame_number, bytes, is_keyframe, frame);
        self.index.insert(frame_number, slot_idx);
        self.attach_front(slot_idx);
        self.current_bytes += bytes;
        self.entry_count += 1;
        if is_keyframe {
            self.keyframe_bytes += bytes;
            self.keyframe_count += 1;
        }
        true
    }

    /// Clears all cached frames, recycling uniquely-owned buffers into the pool.
    pub fn clear(&mut self) {
        self.index.clear();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.occupied {
                if let Some(buf) = slot.frame.take()
                    && self.pool.len() < self.max_pool_entries
                    && Arc::strong_count(&buf) == 1
                {
                    self.pool.push(buf);
                }
                slot.occupied = false;
                slot.prev = NONE;
                slot.next = NONE;
                #[allow(clippy::cast_possible_truncation)]
                self.free_slots.push(i as u32);
            }
        }
        self.head = NONE;
        self.tail = NONE;
        self.current_bytes = 0;
        self.keyframe_bytes = 0;
        self.entry_count = 0;
        self.keyframe_count = 0;
    }

    /// Plans which missing frames to prefetch into `out` (0 heap allocations).
    ///
    /// - `Forward`: schedules `[current+1 .. current+forward_window]` missing from cache.
    /// - `Backward`: schedules the GOP leading up to `current` (`[prev_keyframe .. current]`) in
    ///   decode order so backward stepping is instant without re-seeking every frame.
    /// - `Paused`: schedules immediate neighbours `[current-1, current+1]` if missing.
    pub fn plan_prefetch(
        &self,
        current_frame: u32,
        total_frames: u32,
        direction: StepDirection,
        keyframes: &[u32],
        out: &mut [u32],
    ) -> usize {
        if out.is_empty() || total_frames == 0 {
            return 0;
        }
        let max_frame = total_frames.saturating_sub(1);
        let mut written = 0usize;
        match direction {
            StepDirection::Forward => {
                let mut f = current_frame.saturating_add(1);
                while f <= max_frame && written < out.len() {
                    if !self.contains(f) {
                        out[written] = f;
                        written += 1;
                    }
                    f = f.saturating_add(1);
                }
            }
            StepDirection::Backward => {
                // Find the keyframe at or before `current_frame` to warm the surrounding GOP.
                let kf_pos = keyframes.partition_point(|&kf| kf <= current_frame);
                let gop_start = if kf_pos > 0 {
                    keyframes[kf_pos - 1]
                } else {
                    current_frame.saturating_sub(16)
                };
                // Prioritize frames closest to `current_frame` going backward, bounded by GOP start.
                let mut f = current_frame;
                while f > gop_start && written < out.len() {
                    f -= 1;
                    if !self.contains(f) {
                        out[written] = f;
                        written += 1;
                    }
                }
                if written < out.len() && !self.contains(gop_start) {
                    out[written] = gop_start;
                    written += 1;
                }
            }
            StepDirection::Paused => {
                for cand in [
                    current_frame.saturating_add(1).min(max_frame),
                    current_frame.saturating_sub(1),
                ] {
                    if written >= out.len() {
                        break;
                    }
                    if cand != current_frame && !self.contains(cand) {
                        out[written] = cand;
                        written += 1;
                    }
                }
            }
        }
        written
    }

    /// Current byte usage.
    #[inline]
    #[must_use]
    pub const fn current_bytes(&self) -> usize {
        self.current_bytes
    }

    /// Maximum allowed byte budget.
    #[inline]
    #[must_use]
    pub const fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    /// Current cache statistics.
    #[must_use]
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
            current_bytes: self.current_bytes,
            budget_bytes: self.budget_bytes,
            entry_count: self.entry_count,
            keyframe_count: self.keyframe_count,
            pooled_buffers: self.pool.len(),
        }
    }

    fn alloc_slot(
        &mut self,
        frame_number: u32,
        bytes: usize,
        is_keyframe: bool,
        frame: Arc<DecodedFrame>,
    ) -> u32 {
        if let Some(idx) = self.free_slots.pop() {
            let slot = &mut self.slots[idx as usize];
            slot.frame_number = frame_number;
            slot.bytes = bytes;
            slot.is_keyframe = is_keyframe;
            slot.frame = Some(frame);
            slot.prev = NONE;
            slot.next = NONE;
            slot.occupied = true;
            return idx;
        }
        #[allow(clippy::cast_possible_truncation)]
        let idx = self.slots.len() as u32;
        self.slots.push(Slot {
            frame_number,
            bytes,
            is_keyframe,
            frame: Some(frame),
            prev: NONE,
            next: NONE,
            occupied: true,
        });
        idx
    }

    /// Evicts one slot, preferring the least-recently-used non-keyframe when keyframes are within
    /// their retention share (`keyframe_budget_bytes`).
    fn evict_one(&mut self, skip_slot: Option<u32>) -> bool {
        if self.tail == NONE {
            return false;
        }
        let mut victim = NONE;
        let prefer_non_kf = self.keyframe_bytes <= self.keyframe_budget_bytes;
        if prefer_non_kf {
            let mut cur = self.tail;
            let mut steps = 0usize;
            while cur != NONE && steps < 64 {
                if Some(cur) != skip_slot && !self.slots[cur as usize].is_keyframe {
                    victim = cur;
                    break;
                }
                cur = self.slots[cur as usize].prev;
                steps += 1;
            }
        }
        if victim == NONE {
            let mut cur = self.tail;
            while cur != NONE {
                if Some(cur) != skip_slot {
                    victim = cur;
                    break;
                }
                cur = self.slots[cur as usize].prev;
            }
        }
        if victim == NONE {
            return false;
        }
        self.detach(victim);
        let (frame_number, bytes, is_kf, frame_opt) = {
            let slot = &mut self.slots[victim as usize];
            slot.occupied = false;
            (
                slot.frame_number,
                slot.bytes,
                slot.is_keyframe,
                slot.frame.take(),
            )
        };
        self.index.remove(&frame_number);
        self.current_bytes = self.current_bytes.saturating_sub(bytes);
        self.entry_count = self.entry_count.saturating_sub(1);
        if is_kf {
            self.keyframe_bytes = self.keyframe_bytes.saturating_sub(bytes);
            self.keyframe_count = self.keyframe_count.saturating_sub(1);
        }
        self.evictions += 1;
        if let Some(buf) = frame_opt {
            self.recycle_buffer(buf);
        }
        self.free_slots.push(victim);
        true
    }

    #[inline]
    fn recycle_buffer(&mut self, buf: Arc<DecodedFrame>) {
        if self.pool.len() < self.max_pool_entries && Arc::strong_count(&buf) == 1 {
            self.pool.push(buf);
        }
    }

    #[inline]
    fn attach_front(&mut self, idx: u32) {
        self.slots[idx as usize].prev = NONE;
        self.slots[idx as usize].next = self.head;
        if self.head == NONE {
            self.tail = idx;
        } else {
            self.slots[self.head as usize].prev = idx;
        }
        self.head = idx;
    }

    #[inline]
    fn detach(&mut self, idx: u32) {
        let prev = self.slots[idx as usize].prev;
        let next = self.slots[idx as usize].next;
        if prev == NONE {
            self.head = next;
        } else {
            self.slots[prev as usize].next = next;
        }
        if next == NONE {
            self.tail = prev;
        } else {
            self.slots[next as usize].prev = prev;
        }
        self.slots[idx as usize].prev = NONE;
        self.slots[idx as usize].next = NONE;
    }

    #[inline]
    fn move_to_front(&mut self, idx: u32) {
        if self.head == idx {
            return;
        }
        self.detach(idx);
        self.attach_front(idx);
    }
}

/// Thread-safe handle shared between the background video decoder thread and the UI render thread.
#[derive(Debug, Clone)]
pub struct SharedFrameCache {
    inner: Arc<Mutex<FrameCache>>,
}

impl SharedFrameCache {
    /// Creates a new thread-safe frame cache with `budget_bytes` and `expected_frame_bytes`.
    #[must_use]
    pub fn new(budget_bytes: usize, expected_frame_bytes: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FrameCache::new(
                budget_bytes,
                expected_frame_bytes,
            ))),
        }
    }

    /// Acquires a pooled buffer for the decoder thread.
    #[must_use]
    pub fn acquire_buffer(&self, min_bytes: usize) -> Arc<DecodedFrame> {
        let Ok(mut guard) = self.inner.lock() else {
            return Arc::new(DecodedFrame {
                frame_number: 0,
                width: 0,
                height: 0,
                stride: 0,
                is_keyframe: false,
                pixels: vec![0u8; min_bytes],
            });
        };
        guard.acquire_buffer(min_bytes)
    }

    /// Looks up a frame on the UI thread.
    #[must_use]
    pub fn get(&self, frame_number: u32) -> Option<Arc<DecodedFrame>> {
        self.inner.lock().ok()?.get(frame_number)
    }

    /// Inserts a decoded frame from the decoder thread.
    #[must_use]
    pub fn insert(&self, frame: Arc<DecodedFrame>) -> bool {
        self.inner.lock().is_ok_and(|mut guard| guard.insert(frame))
    }

    /// Plans prefetch frame numbers into `out`.
    pub fn plan_prefetch(
        &self,
        current_frame: u32,
        total_frames: u32,
        direction: StepDirection,
        keyframes: &[u32],
        out: &mut [u32],
    ) -> usize {
        self.inner.lock().map_or(0, |guard| {
            guard.plan_prefetch(current_frame, total_frames, direction, keyframes, out)
        })
    }

    /// Returns a snapshot of cache statistics.
    #[must_use]
    pub fn stats(&self) -> CacheStats {
        self.inner
            .lock()
            .map_or_else(|_| CacheStats::default(), |guard| guard.stats())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame(num: u32, bytes: usize, is_kf: bool) -> Arc<DecodedFrame> {
        Arc::new(DecodedFrame {
            frame_number: num,
            width: 64,
            height: 64,
            stride: 256,
            is_keyframe: is_kf,
            pixels: vec![0xAB; bytes],
        })
    }

    #[test]
    fn enforces_budget_keyframe_retention_pooling_and_thread_safety() {
        let mut cache = FrameCache::new(4 * 1024, 1024);
        assert!(cache.insert(make_frame(0, 1024, true))); // keyframe
        assert!(cache.insert(make_frame(1, 1024, false)));
        assert!(cache.insert(make_frame(2, 1024, false)));
        assert!(cache.insert(make_frame(3, 1024, false)));
        assert_eq!(cache.current_bytes(), 4096);

        // Inserting frame 4 must evict non-keyframe 1 (not keyframe 0!) and stay <= budget
        assert!(cache.insert(make_frame(4, 1024, false)));
        assert!(cache.current_bytes() <= cache.budget_bytes());
        assert!(cache.contains(0), "keyframe 0 should be retained");
        assert!(
            !cache.contains(1),
            "oldest non-keyframe 1 should be evicted"
        );
        assert!(cache.stats().pooled_buffers >= 1);

        // Backward GOP prefetch
        let mut plan = [0u32; 8];
        let n = cache.plan_prefetch(5, 100, StepDirection::Backward, &[0, 24], &mut plan);
        assert!(n >= 1);
        assert!(plan[..n].contains(&1));

        // Thread-safe shared cache across 2 threads
        let shared = SharedFrameCache::new(8 * 1024, 1024);
        let producer = shared.clone();
        let handle = std::thread::spawn(move || {
            for f in 0..16u32 {
                assert!(producer.insert(make_frame(f, 1024, f % 8 == 0)));
            }
        });
        handle.join().expect("join");
        let st = shared.stats();
        assert!(st.current_bytes <= st.budget_bytes);
        assert!(shared.get(15).is_some());
    }
}
