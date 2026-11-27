//! Background FFMS2 video indexing/decoding + libass rendering worker.

use media::{FfmsVideo, MediaError, VideoInfo};
use render::AssSubRenderer;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Commands sent from the UI thread to the background media/render worker.
pub enum PlayerCmd {
    OpenVideo {
        path: PathBuf,
        open_id: u64,
        cancel: Arc<AtomicBool>,
    },
    LoadAssMemory(String),
    LoadAssFile(PathBuf),
    SeekFrame(usize),
    StepFrame(isize),
    NextKeyframe,
    PrevKeyframe,
    SetPlaying(bool),
    SetLoop(bool),
    Shutdown,
}

/// Decoded and subtitle-blended RGBA frame ready for `wgpu` texture upload.
pub struct ReadyFrame {
    pub generation: u64,
    pub frame_index: usize,
    pub pts_ms: i64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub decode_ms: f32,
    pub render_ms: f32,
}

/// Events emitted by the background worker to the UI thread.
pub enum PlayerEvent {
    IndexingProgress { current: i64, total: i64 },
    IndexingCancelled,
    VideoReady { info: VideoInfo, file_name: String },
    Frame(ReadyFrame),
    PlaybackEnded,
    AssError(String),
    Error(String),
}

/// Handle to the background video + subtitle worker thread.
pub struct VideoPlayerHandle {
    cmd_tx: mpsc::Sender<PlayerCmd>,
    evt_rx: mpsc::Receiver<PlayerEvent>,
    recycle_tx: mpsc::Sender<Vec<u8>>,
    latest_open_id: Arc<AtomicU64>,
    active_cancel: Mutex<Option<Arc<AtomicBool>>>,
    queued_frames: Arc<AtomicUsize>,
    worker_dead_reported: AtomicBool,
    join_handle: Option<JoinHandle<()>>,
}

impl VideoPlayerHandle {
    /// Spawns the worker thread with an initial ASS script and repaint callback.
    pub fn spawn(initial_ass: String, ctx: egui::Context) -> anyhow::Result<Self> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (evt_tx, evt_rx) = mpsc::channel();
        let (recycle_tx, recycle_rx) = mpsc::channel();
        let latest_open_id = Arc::new(AtomicU64::new(0));
        let queued_frames = Arc::new(AtomicUsize::new(0));

        let open_id_clone = Arc::clone(&latest_open_id);
        let queued_clone = Arc::clone(&queued_frames);

        let join_handle = thread::Builder::new()
            .name("hlouis-video-worker".to_owned())
            .spawn(move || {
                worker_loop(
                    &cmd_rx,
                    &evt_tx,
                    &recycle_rx,
                    &open_id_clone,
                    &queued_clone,
                    &initial_ass,
                    &ctx,
                );
            })
            .map_err(|err| anyhow::anyhow!("failed to spawn video worker thread: {err}"))?;

        Ok(Self {
            cmd_tx,
            evt_rx,
            recycle_tx,
            latest_open_id,
            active_cancel: Mutex::new(None),
            queued_frames,
            worker_dead_reported: AtomicBool::new(false),
            join_handle: Some(join_handle),
        })
    }

    /// Opens a new video with its own cancellation token and monotonic open ID,
    /// cancelling any in-flight indexing from an earlier open request.
    pub fn open_video(&self, path: PathBuf) -> Result<(), String> {
        let open_id = self.latest_open_id.fetch_add(1, Ordering::AcqRel) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut guard = self
                .active_cancel
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(prev) = guard.replace(Arc::clone(&cancel)) {
                prev.store(true, Ordering::Release);
            }
        }
        self.send(PlayerCmd::OpenVideo {
            path,
            open_id,
            cancel,
        })
    }

    /// Sends a command to the worker thread, returning an error if the worker has exited.
    pub fn send(&self, cmd: PlayerCmd) -> Result<(), String> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| "video worker thread is no longer running".to_owned())
    }

    /// Non-blocking receive of pending worker events, surfacing unexpected worker exit once.
    pub fn drain_events(&self) -> Vec<PlayerEvent> {
        let mut out = Vec::new();
        loop {
            match self.evt_rx.try_recv() {
                Ok(evt) => out.push(evt),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if !self.worker_dead_reported.swap(true, Ordering::AcqRel) {
                        out.push(PlayerEvent::Error(
                            "video worker thread terminated unexpectedly".to_owned(),
                        ));
                    }
                    break;
                }
            }
        }
        out
    }

    /// Releases `count` buffered frame slots back to the worker lookahead budget.
    pub fn release_frame_slots(&self, count: usize) {
        if count == 0 {
            return;
        }
        let _ = self
            .queued_frames
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(count))
            });
    }

    /// Returns a consumed RGBA frame buffer to the worker pool to avoid OS page-fault reallocations.
    pub fn recycle_buffer(&self, buf: Vec<u8>) {
        let _ = self.recycle_tx.send(buf);
    }

    /// Signals cancellation for the currently active indexing operation (if any).
    pub fn cancel_indexing(&self) {
        let guard = self
            .active_cancel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cancel) = guard.as_ref() {
            cancel.store(true, Ordering::Release);
        }
    }
}

impl Drop for VideoPlayerHandle {
    fn drop(&mut self) {
        self.cancel_indexing();
        let _ = self.cmd_tx.send(PlayerCmd::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

const MAX_QUEUED_FRAMES: usize = 3;
const MAX_POOLED_BUFFERS_PLAYING: usize = 4;
const MAX_POOLED_BUFFERS_PAUSED: usize = 1;
const MAX_CLEAN_PAUSED_FRAMES: usize = 4;

struct CleanPausedFrame {
    frame_index: usize,
    pts_ms: i64,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

struct WorkerState {
    video: Option<FfmsVideo>,
    renderer: Option<AssSubRenderer>,
    ass_script: AssContent,
    current_frame: usize,
    playing: bool,
    loop_playback: bool,
    generation: u64,
    buffer_pool: Vec<Vec<u8>>,
    clean_paused_cache: VecDeque<CleanPausedFrame>,
}

enum AssContent {
    Memory(String),
    File(PathBuf),
}

impl WorkerState {
    fn pool_cap(&self) -> usize {
        if self.playing {
            MAX_POOLED_BUFFERS_PLAYING
        } else {
            MAX_POOLED_BUFFERS_PAUSED
        }
    }

    fn trim_pool(&mut self) {
        let cap = self.pool_cap();
        if self.buffer_pool.len() > cap {
            self.buffer_pool.truncate(cap);
            self.buffer_pool.shrink_to_fit();
        }
    }

    fn clear_clean_paused(&mut self) {
        self.clean_paused_cache.clear();
    }

    fn ensure_renderer(&mut self, width: u32, height: u32) -> Result<&mut AssSubRenderer, String> {
        if self.renderer.is_none() {
            let mut r = AssSubRenderer::new(width, height).map_err(|e| e.to_string())?;
            apply_ass_content(&mut r, &self.ass_script)?;
            self.renderer = Some(r);
        } else if let Some(r) = self.renderer.as_mut() {
            r.resize(width, height);
        }
        self.renderer
            .as_mut()
            .ok_or_else(|| "renderer unavailable".to_owned())
    }

    fn reload_ass(&mut self) -> Result<(), String> {
        if let Some(r) = self.renderer.as_mut() {
            apply_ass_content(r, &self.ass_script)?;
        }
        Ok(())
    }

    fn render_frame(
        &mut self,
        frame_index: usize,
        cache_clean: bool,
    ) -> Result<ReadyFrame, String> {
        let mut rgba = self.buffer_pool.pop().unwrap_or_default();
        let cached = if cache_clean {
            self.clean_paused_cache
                .iter()
                .position(|c| c.frame_index == frame_index)
                .and_then(|pos| self.clean_paused_cache.remove(pos))
        } else {
            None
        };
        let (pts_ms, width, height, decode_ms) = if let Some(clean) = cached {
            rgba.clear();
            rgba.extend_from_slice(&clean.rgba);
            let out = (clean.pts_ms, clean.width, clean.height, 0.0);
            self.clean_paused_cache.push_back(clean);
            out
        } else {
            let video = self
                .video
                .as_mut()
                .ok_or_else(|| "no video loaded".to_owned())?;
            let info = video.info().clone();
            if frame_index >= info.num_frames {
                self.buffer_pool.push(rgba);
                return Err("frame out of range".to_owned());
            }

            let t_decode = Instant::now();
            let pts_ms = match video.decode_frame_into(frame_index, &mut rgba) {
                Ok(pts) => pts,
                Err(err) => {
                    self.buffer_pool.push(rgba);
                    return Err(err.to_string());
                }
            };
            let decode_ms = t_decode.elapsed().as_secs_f32() * 1000.0;
            if cache_clean {
                let buf = if self.clean_paused_cache.len() >= MAX_CLEAN_PAUSED_FRAMES {
                    if let Some(mut evicted) = self.clean_paused_cache.pop_front() {
                        evicted.rgba.clear();
                        evicted.rgba.extend_from_slice(&rgba);
                        evicted.rgba
                    } else {
                        rgba.clone()
                    }
                } else {
                    rgba.clone()
                };
                self.clean_paused_cache.push_back(CleanPausedFrame {
                    frame_index,
                    pts_ms,
                    width: info.width,
                    height: info.height,
                    rgba: buf,
                });
            }
            (pts_ms, info.width, info.height, decode_ms)
        };

        let t_render = Instant::now();
        let render_res = {
            let renderer = self.ensure_renderer(width, height)?;
            renderer.render_and_blend_rgba(pts_ms, &mut rgba, width, height)
        };
        let render_ms = t_render.elapsed().as_secs_f32() * 1000.0;

        if let Err(err) = render_res {
            self.buffer_pool.push(rgba);
            return Err(err.to_string());
        }

        Ok(ReadyFrame {
            generation: self.generation,
            frame_index,
            pts_ms,
            width,
            height,
            rgba,
            decode_ms,
            render_ms,
        })
    }
}

fn apply_ass_content(renderer: &mut AssSubRenderer, content: &AssContent) -> Result<(), String> {
    match content {
        AssContent::Memory(text) => renderer.load_ass_memory(text).map_err(|e| e.to_string()),
        AssContent::File(path) => renderer.load_ass_file(path).map_err(|e| e.to_string()),
    }
}

fn coalesce_commands(cmds: Vec<PlayerCmd>) -> Vec<PlayerCmd> {
    let mut out: Vec<PlayerCmd> = Vec::with_capacity(cmds.len());
    for cmd in cmds {
        match cmd {
            PlayerCmd::SeekFrame(target) => {
                while matches!(
                    out.last(),
                    Some(PlayerCmd::SeekFrame(_) | PlayerCmd::StepFrame(_))
                ) {
                    out.pop();
                }
                out.push(PlayerCmd::SeekFrame(target));
            }
            PlayerCmd::StepFrame(delta) => match out.last_mut() {
                Some(PlayerCmd::StepFrame(prev)) => {
                    *prev = prev.saturating_add(delta);
                }
                Some(PlayerCmd::SeekFrame(prev_target)) => {
                    *prev_target = prev_target.saturating_add_signed(delta);
                }
                _ => out.push(PlayerCmd::StepFrame(delta)),
            },
            PlayerCmd::LoadAssMemory(text) => {
                while matches!(out.last(), Some(PlayerCmd::LoadAssMemory(_))) {
                    out.pop();
                }
                out.push(PlayerCmd::LoadAssMemory(text));
            }
            other => out.push(other),
        }
    }
    out
}

fn worker_loop(
    cmd_rx: &mpsc::Receiver<PlayerCmd>,
    evt_tx: &mpsc::Sender<PlayerEvent>,
    recycle_rx: &mpsc::Receiver<Vec<u8>>,
    latest_open_id: &AtomicU64,
    queued_frames: &AtomicUsize,
    initial_ass: &str,
    ctx: &egui::Context,
) {
    let mut state = WorkerState {
        video: None,
        renderer: None,
        ass_script: AssContent::Memory(initial_ass.to_owned()),
        current_frame: 0,
        playing: false,
        loop_playback: false,
        generation: 1,
        buffer_pool: Vec::new(),
        clean_paused_cache: VecDeque::with_capacity(MAX_CLEAN_PAUSED_FRAMES),
    };

    loop {
        let cap = state.pool_cap();
        while let Ok(buf) = recycle_rx.try_recv() {
            if state.buffer_pool.len() < cap {
                state.buffer_pool.push(buf);
            }
        }
        state.trim_pool();
        let cmd = if state.playing && state.video.is_some() {
            if queued_frames.load(Ordering::Relaxed) < MAX_QUEUED_FRAMES {
                match cmd_rx.try_recv() {
                    Ok(c) => Some(c),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }
            } else {
                match cmd_rx.recv_timeout(Duration::from_millis(4)) {
                    Ok(c) => Some(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        } else {
            match cmd_rx.recv() {
                Ok(c) => Some(c),
                Err(_) => break,
            }
        };

        if let Some(first) = cmd {
            let mut batch = vec![first];
            // Drain any additional queued commands and coalesce consecutive seeks/steps.
            while let Ok(next_c) = cmd_rx.try_recv() {
                batch.push(next_c);
            }
            for next_c in coalesce_commands(batch) {
                if !handle_command(
                    next_c,
                    &mut state,
                    evt_tx,
                    latest_open_id,
                    queued_frames,
                    ctx,
                ) {
                    return;
                }
            }
        }

        if state.playing {
            let num_frames = state.video.as_ref().map_or(0, |v| v.info().num_frames);
            if num_frames == 0 {
                state.playing = false;
                continue;
            }
            let target = state.current_frame + 1;
            let next_frame = if target < num_frames {
                target
            } else if state.loop_playback {
                0
            } else {
                state.playing = false;
                state.trim_pool();
                let _ = evt_tx.send(PlayerEvent::PlaybackEnded);
                ctx.request_repaint();
                continue;
            };

            match state.render_frame(next_frame, false) {
                Ok(frame) => {
                    state.current_frame = next_frame;
                    queued_frames.fetch_add(1, Ordering::Relaxed);
                    let _ = evt_tx.send(PlayerEvent::Frame(frame));
                    ctx.request_repaint();
                }
                Err(err) => {
                    state.playing = false;
                    let _ = evt_tx.send(PlayerEvent::Error(err));
                    ctx.request_repaint();
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_open_video(
    path: &Path,
    open_id: u64,
    cancel: &Arc<AtomicBool>,
    state: &mut WorkerState,
    evt_tx: &mpsc::Sender<PlayerEvent>,
    latest_open_id: &AtomicU64,
    queued_frames: &AtomicUsize,
    ctx: &egui::Context,
) {
    state.playing = false;
    state.clear_clean_paused();
    state.buffer_pool.clear();
    state.buffer_pool.shrink_to_fit();
    state.generation = state.generation.wrapping_add(1);
    if cancel.load(Ordering::Acquire) || open_id != latest_open_id.load(Ordering::Acquire) {
        if open_id == latest_open_id.load(Ordering::Acquire) {
            let _ = evt_tx.send(PlayerEvent::IndexingCancelled);
            ctx.request_repaint();
        }
        return;
    }
    let mut last_report = Instant::now();
    let res = FfmsVideo::open_with_progress(path, cancel, |current, total| {
        if open_id == latest_open_id.load(Ordering::Acquire)
            && (last_report.elapsed() >= Duration::from_millis(50) || current == total)
        {
            last_report = Instant::now();
            let _ = evt_tx.send(PlayerEvent::IndexingProgress { current, total });
            ctx.request_repaint();
        }
    });
    match res {
        Ok(video) => {
            if open_id != latest_open_id.load(Ordering::Acquire) {
                return;
            }
            let info = video.info().clone();
            let file_name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            state.video = Some(video);
            state.current_frame = 0;
            let _ = evt_tx.send(PlayerEvent::VideoReady { info, file_name });
            emit_seek_frame(state, 0, evt_tx, queued_frames, ctx);
        }
        Err(MediaError::Cancelled) => {
            if open_id == latest_open_id.load(Ordering::Acquire) {
                let _ = evt_tx.send(PlayerEvent::IndexingCancelled);
                ctx.request_repaint();
            }
        }
        Err(err) => {
            if open_id == latest_open_id.load(Ordering::Acquire) {
                let _ = evt_tx.send(PlayerEvent::Error(err.to_string()));
                ctx.request_repaint();
            }
        }
    }
}

fn handle_command(
    cmd: PlayerCmd,
    state: &mut WorkerState,
    evt_tx: &mpsc::Sender<PlayerEvent>,
    latest_open_id: &AtomicU64,
    queued_frames: &AtomicUsize,
    ctx: &egui::Context,
) -> bool {
    match cmd {
        PlayerCmd::Shutdown => return false,
        PlayerCmd::SetLoop(enabled) => {
            state.loop_playback = enabled;
        }
        PlayerCmd::SetPlaying(play) => {
            state.playing = play && state.video.is_some();
            if state.playing {
                state.clear_clean_paused();
            } else {
                state.trim_pool();
            }
        }
        PlayerCmd::OpenVideo {
            path,
            open_id,
            cancel,
        } => {
            handle_open_video(
                &path,
                open_id,
                &cancel,
                state,
                evt_tx,
                latest_open_id,
                queued_frames,
                ctx,
            );
        }
        PlayerCmd::LoadAssMemory(text) => {
            reload_ass_and_emit(state, AssContent::Memory(text), evt_tx, queued_frames, ctx);
        }
        PlayerCmd::LoadAssFile(path) => {
            reload_ass_and_emit(state, AssContent::File(path), evt_tx, queued_frames, ctx);
        }
        PlayerCmd::SeekFrame(frame) => {
            if let Some(video) = state.video.as_ref() {
                let max_idx = video.info().num_frames.saturating_sub(1);
                let target = frame.min(max_idx);
                state.generation = state.generation.wrapping_add(1);
                emit_seek_frame(state, target, evt_tx, queued_frames, ctx);
            }
        }
        PlayerCmd::StepFrame(delta) => {
            if let Some(video) = state.video.as_ref() {
                state.playing = false;
                let max_idx = video.info().num_frames.saturating_sub(1);
                let target = state
                    .current_frame
                    .saturating_add_signed(delta)
                    .min(max_idx);
                state.generation = state.generation.wrapping_add(1);
                emit_seek_frame(state, target, evt_tx, queued_frames, ctx);
            }
        }
        PlayerCmd::NextKeyframe => {
            if let Some(video) = state.video.as_ref() {
                if let Some(kf) = video.next_keyframe(state.current_frame) {
                    state.playing = false;
                    state.generation = state.generation.wrapping_add(1);
                    emit_seek_frame(state, kf, evt_tx, queued_frames, ctx);
                }
            }
        }
        PlayerCmd::PrevKeyframe => {
            if let Some(video) = state.video.as_ref() {
                if let Some(kf) = video.prev_keyframe(state.current_frame) {
                    state.playing = false;
                    state.generation = state.generation.wrapping_add(1);
                    emit_seek_frame(state, kf, evt_tx, queued_frames, ctx);
                }
            }
        }
    }
    true
}

fn reload_ass_and_emit(
    state: &mut WorkerState,
    content: AssContent,
    evt_tx: &mpsc::Sender<PlayerEvent>,
    queued_frames: &AtomicUsize,
    ctx: &egui::Context,
) {
    state.ass_script = content;
    if let Err(err) = state.reload_ass() {
        let _ = evt_tx.send(PlayerEvent::AssError(err));
    } else if state.video.is_some() && !state.playing {
        state.generation = state.generation.wrapping_add(1);
        emit_seek_frame(state, state.current_frame, evt_tx, queued_frames, ctx);
    }
    ctx.request_repaint();
}

fn emit_seek_frame(
    state: &mut WorkerState,
    target: usize,
    evt_tx: &mpsc::Sender<PlayerEvent>,
    queued_frames: &AtomicUsize,
    ctx: &egui::Context,
) {
    match state.render_frame(target, true) {
        Ok(frame) => {
            state.current_frame = target;
            queued_frames.fetch_add(1, Ordering::Relaxed);
            let _ = evt_tx.send(PlayerEvent::Frame(frame));
        }
        Err(err) => {
            let _ = evt_tx.send(PlayerEvent::Error(err));
        }
    }
    state.trim_pool();
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_consecutive_seeks_and_steps() {
        let cmds = vec![
            PlayerCmd::StepFrame(1),
            PlayerCmd::StepFrame(1),
            PlayerCmd::StepFrame(1),
            PlayerCmd::SeekFrame(100),
            PlayerCmd::StepFrame(2),
            PlayerCmd::StepFrame(-1),
            PlayerCmd::SetLoop(true),
            PlayerCmd::StepFrame(1),
            PlayerCmd::StepFrame(1),
        ];
        let coalesced = coalesce_commands(cmds);
        assert_eq!(coalesced.len(), 3);
        assert!(matches!(coalesced[0], PlayerCmd::SeekFrame(101)));
        assert!(matches!(coalesced[1], PlayerCmd::SetLoop(true)));
        assert!(matches!(coalesced[2], PlayerCmd::StepFrame(2)));
    }
}
