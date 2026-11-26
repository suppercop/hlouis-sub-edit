//! Background FFMS2 video indexing/decoding + libass rendering worker.

use media::{FfmsVideo, VideoInfo};
use render::AssSubRenderer;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Commands sent from the UI thread to the background media/render worker.
pub enum PlayerCmd {
    OpenVideo(PathBuf),
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
    cancel_index: Arc<AtomicBool>,
    queued_frames: Arc<AtomicUsize>,
    join_handle: Option<JoinHandle<()>>,
}

impl VideoPlayerHandle {
    /// Spawns the worker thread with an initial ASS script and repaint callback.
    #[must_use]
    pub fn spawn(initial_ass: String, ctx: egui::Context) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (evt_tx, evt_rx) = mpsc::channel();
        let (recycle_tx, recycle_rx) = mpsc::channel();
        let cancel_index = Arc::new(AtomicBool::new(false));
        let queued_frames = Arc::new(AtomicUsize::new(0));

        let cancel_clone = Arc::clone(&cancel_index);
        let queued_clone = Arc::clone(&queued_frames);

        let join_handle = thread::Builder::new()
            .name("hlouis-video-worker".to_owned())
            .spawn(move || {
                worker_loop(
                    &cmd_rx,
                    &evt_tx,
                    &recycle_rx,
                    &cancel_clone,
                    &queued_clone,
                    &initial_ass,
                    &ctx,
                );
            })
            .ok();

        Self {
            cmd_tx,
            evt_rx,
            recycle_tx,
            cancel_index,
            queued_frames,
            join_handle,
        }
    }

    /// Sends a command to the worker thread.
    pub fn send(&self, cmd: PlayerCmd) {
        if matches!(cmd, PlayerCmd::OpenVideo(_)) {
            self.cancel_index.store(true, Ordering::Relaxed);
        }
        let _ = self.cmd_tx.send(cmd);
    }

    /// Non-blocking receive of pending worker events.
    pub fn drain_events(&self) -> Vec<PlayerEvent> {
        let mut out = Vec::new();
        while let Ok(evt) = self.evt_rx.try_recv() {
            out.push(evt);
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

    /// Signals cancellation for any in-flight indexing operation.
    pub fn cancel_indexing(&self) {
        self.cancel_index.store(true, Ordering::Relaxed);
    }
}

impl Drop for VideoPlayerHandle {
    fn drop(&mut self) {
        self.cancel_index.store(true, Ordering::Relaxed);
        let _ = self.cmd_tx.send(PlayerCmd::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
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
}

enum AssContent {
    Memory(String),
    File(PathBuf),
}

impl WorkerState {
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

    fn render_frame(&mut self, frame_index: usize) -> Result<ReadyFrame, String> {
        let mut rgba = self.buffer_pool.pop().unwrap_or_default();
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

        let t_render = Instant::now();
        let width = info.width;
        let height = info.height;
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

fn worker_loop(
    cmd_rx: &mpsc::Receiver<PlayerCmd>,
    evt_tx: &mpsc::Sender<PlayerEvent>,
    recycle_rx: &mpsc::Receiver<Vec<u8>>,
    cancel_index: &AtomicBool,
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
    };

    loop {
        while let Ok(buf) = recycle_rx.try_recv() {
            if state.buffer_pool.len() < 8 {
                state.buffer_pool.push(buf);
            }
        }
        let cmd = if state.playing && state.video.is_some() {
            if queued_frames.load(Ordering::Relaxed) < 4 {
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

        if let Some(c) = cmd {
            if !handle_command(c, &mut state, evt_tx, cancel_index, queued_frames, ctx) {
                break;
            }
            // Drain any additional queued commands before decoding the next playback frame.
            while let Ok(next_c) = cmd_rx.try_recv() {
                if !handle_command(next_c, &mut state, evt_tx, cancel_index, queued_frames, ctx) {
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
                let _ = evt_tx.send(PlayerEvent::PlaybackEnded);
                ctx.request_repaint();
                continue;
            };

            match state.render_frame(next_frame) {
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

fn handle_command(
    cmd: PlayerCmd,
    state: &mut WorkerState,
    evt_tx: &mpsc::Sender<PlayerEvent>,
    cancel_index: &AtomicBool,
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
        }
        PlayerCmd::OpenVideo(path) => {
            state.playing = false;
            state.generation = state.generation.wrapping_add(1);
            cancel_index.store(false, Ordering::Relaxed);
            let mut last_report = Instant::now();
            let res = FfmsVideo::open_with_progress(&path, cancel_index, |current, total| {
                if last_report.elapsed() >= Duration::from_millis(50) || current == total {
                    last_report = Instant::now();
                    let _ = evt_tx.send(PlayerEvent::IndexingProgress { current, total });
                    ctx.request_repaint();
                }
            });
            match res {
                Ok(video) => {
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
                Err(err) => {
                    let _ = evt_tx.send(PlayerEvent::Error(err.to_string()));
                    ctx.request_repaint();
                }
            }
        }
        PlayerCmd::LoadAssMemory(text) => {
            state.ass_script = AssContent::Memory(text);
            if let Err(err) = state.reload_ass() {
                let _ = evt_tx.send(PlayerEvent::AssError(err));
            } else if state.video.is_some() && !state.playing {
                state.generation = state.generation.wrapping_add(1);
                emit_seek_frame(state, state.current_frame, evt_tx, queued_frames, ctx);
            }
            ctx.request_repaint();
        }
        PlayerCmd::LoadAssFile(path) => {
            state.ass_script = AssContent::File(path);
            if let Err(err) = state.reload_ass() {
                let _ = evt_tx.send(PlayerEvent::AssError(err));
            } else if state.video.is_some() && !state.playing {
                state.generation = state.generation.wrapping_add(1);
                emit_seek_frame(state, state.current_frame, evt_tx, queued_frames, ctx);
            }
            ctx.request_repaint();
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

fn emit_seek_frame(
    state: &mut WorkerState,
    target: usize,
    evt_tx: &mpsc::Sender<PlayerEvent>,
    queued_frames: &AtomicUsize,
    ctx: &egui::Context,
) {
    match state.render_frame(target) {
        Ok(frame) => {
            state.current_frame = target;
            queued_frames.fetch_add(1, Ordering::Relaxed);
            let _ = evt_tx.send(PlayerEvent::Frame(frame));
        }
        Err(err) => {
            let _ = evt_tx.send(PlayerEvent::Error(err));
        }
    }
    ctx.request_repaint();
}
