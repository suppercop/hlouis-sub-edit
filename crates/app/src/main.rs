//! Phase 0 spike application: 50k virtualized grid, syntax-highlighted IME edit box,
//! FFMS2 + libass + wgpu video preview, Light/Dark/System themes, and en/vi Fluent i18n.

mod video_player;

use crate::video_player::{PlayerCmd, PlayerEvent, ReadyFrame, VideoPlayerHandle};
use eframe::egui;
use egui::{FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect, Sense, Vec2};
use fluent::FluentValue;
use hlouis_sub_edit::bench_ui::{self, WindowedBenchSummary, WindowedVideoSummary};
use hlouis_sub_edit::ffi;
use hlouis_sub_edit::grid::{self, GridState};
use hlouis_sub_edit::metrics::LatencyHistogram;
use hlouis_sub_edit::sample_ass;
use hlouis_sub_edit::syntax;
use hlouis_sub_edit::theme::{self, ThemeMode, ThemePalette};
use i18n::{I18n, Locale};
use media::VideoInfo;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum VideoScaleMode {
    #[default]
    FitPanel,
    Native1to1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PlaybackState {
    #[default]
    Paused,
    Playing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum BenchMode {
    #[default]
    None,
    Windowed,
    Headless,
}

#[derive(Default)]
struct CliOptions {
    video: Option<PathBuf>,
    ass: Option<PathBuf>,
    autoplay: bool,
    loop_playback: bool,
    log_metrics: bool,
    bench: BenchMode,
    theme: Option<ThemeMode>,
    lang: Option<Locale>,
}

impl CliOptions {
    fn parse() -> Self {
        let mut opts = Self::default();
        let mut has_bench_ui = false;
        let mut has_headless = false;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--video" => opts.video = args.next().map(PathBuf::from),
                "--ass" => opts.ass = args.next().map(PathBuf::from),
                "--autoplay" => opts.autoplay = true,
                "--loop" => opts.loop_playback = true,
                "--log-metrics" => opts.log_metrics = true,
                "--bench-ui" => has_bench_ui = true,
                "--headless" => has_headless = true,
                "--theme" => {
                    opts.theme = args.next().as_deref().and_then(|v| match v {
                        "light" => Some(ThemeMode::Light),
                        "dark" => Some(ThemeMode::Dark),
                        "system" => Some(ThemeMode::FollowWindows),
                        _ => None,
                    });
                }
                "--lang" => {
                    opts.lang = args.next().as_deref().and_then(Locale::from_code);
                }
                _ => {}
            }
        }
        opts.bench = match (has_bench_ui, has_headless) {
            (true, true) => BenchMode::Headless,
            (true, false) => BenchMode::Windowed,
            (false, _) => BenchMode::None,
        };
        opts
    }
}

enum AssSource {
    BuiltInSample,
    File(String),
}

struct PlaybackClock {
    start_instant: Instant,
    start_pts_ms: f64,
}

struct QueueAdvanceResult {
    due_frame: Option<ReadyFrame>,
    recycled_buffers: Vec<Vec<u8>>,
    popped_count: usize,
    dropped_count: u64,
}

/// Pure playback-queue advancement with lazy Play clock anchoring and loop wrap-around handling.
fn pop_due_playback_frame(
    frame_queue: &mut VecDeque<ReadyFrame>,
    playback_clock: &mut Option<PlaybackClock>,
    current_frame: usize,
    rendered_frames: u64,
    frame_interval_ms: f64,
    now: Instant,
) -> QueueAdvanceResult {
    let mut result = QueueAdvanceResult {
        due_frame: None,
        recycled_buffers: Vec::new(),
        popped_count: 0,
        dropped_count: 0,
    };

    let Some(clock) = playback_clock.as_mut() else {
        if let Some(first) = frame_queue.pop_front() {
            *playback_clock = Some(PlaybackClock {
                start_instant: now,
                start_pts_ms: first.pts_ms as f64,
            });
            result.due_frame = Some(first);
            result.popped_count = 1;
        }
        return result;
    };

    let target_pts_ms = clock.start_pts_ms
        + now
            .saturating_duration_since(clock.start_instant)
            .as_secs_f64()
            * 1000.0;
    let mut latest_due: Option<ReadyFrame> = None;
    let mut last_frame_idx = current_frame;
    let mut wrapped = false;

    while let Some(front) = frame_queue.front() {
        if front.frame_index < last_frame_idx && front.frame_index == 0 {
            if latest_due.is_some() {
                break;
            }
            clock.start_instant = now;
            clock.start_pts_ms = front.pts_ms as f64;
            latest_due = frame_queue.pop_front();
            result.popped_count += 1;
            wrapped = true;
            break;
        }
        if (front.pts_ms as f64) <= target_pts_ms {
            if let Some(prev) = latest_due.take() {
                result.recycled_buffers.push(prev.rgba);
                result.dropped_count += 1;
            }
            if let Some(popped) = frame_queue.pop_front() {
                last_frame_idx = popped.frame_index;
                result.popped_count += 1;
                latest_due = Some(popped);
            }
        } else {
            break;
        }
    }

    if let Some(due) = latest_due.as_ref() {
        if !wrapped {
            let lateness = target_pts_ms - (due.pts_ms as f64);
            if lateness > frame_interval_ms * 1.5 && rendered_frames > 0 {
                result.dropped_count += 1;
            }
        }
    }
    result.due_frame = latest_due;
    result
}

struct VideoTexture {
    wgpu_texture: wgpu::Texture,
    egui_id: egui::TextureId,
    width: u32,
    height: u32,
}

struct VideoMetrics {
    decode_ms: f32,
    render_ms: f32,
    upload_ms: f32,
    total_decode_ms: f64,
    total_render_ms: f64,
    total_upload_ms: f64,
    max_decode_ms: f32,
    max_render_ms: f32,
    max_upload_ms: f32,
    dropped_frames: u64,
    rendered_frames: u64,
    current_frame: usize,
    current_pts_ms: i64,
}

impl Default for VideoMetrics {
    fn default() -> Self {
        Self {
            decode_ms: 0.0,
            render_ms: 0.0,
            upload_ms: 0.0,
            total_decode_ms: 0.0,
            total_render_ms: 0.0,
            total_upload_ms: 0.0,
            max_decode_ms: 0.0,
            max_render_ms: 0.0,
            max_upload_ms: 0.0,
            dropped_frames: 0,
            rendered_frames: 0,
            current_frame: 0,
            current_pts_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingFrameOp {
    OpenFirstFrame,
    RandomSeek,
    StepOne,
    KeyframeSeek,
    BenchWrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchPhase {
    AwaitColdPresent,
    Setup200k,
    Scroll200k(usize),
    SelectAll200k,
    Keystrokes(usize),
    IdleSettle,
    IdleWait,
    VideoAwaitReady,
    VideoSeeks(usize),
    VideoSteps(usize),
    VideoKeyframes(usize),
    VideoPlayStart,
    VideoPlayCollect,
    Done,
}

struct WindowedBenchDriver {
    phase: BenchPhase,
    has_video: bool,
    last_tick: Instant,
    scroll_cpu_hist: LatencyHistogram,
    scroll_present_hist: LatencyHistogram,
    keystroke_hist: LatencyHistogram,
    key_start: Option<Instant>,
    idle_start: Instant,
    idle_cpu_s: f64,
    idle_repaints: u32,
    rng_state: u64,
    awaiting_frame: bool,
    libass_play_hist: LatencyHistogram,
    play_start: Instant,
    done_frames: u8,
    summary: WindowedBenchSummary,
}

impl WindowedBenchDriver {
    fn new(has_video: bool) -> Self {
        let now = Instant::now();
        Self {
            phase: BenchPhase::AwaitColdPresent,
            has_video,
            last_tick: now,
            scroll_cpu_hist: LatencyHistogram::new(),
            scroll_present_hist: LatencyHistogram::new(),
            keystroke_hist: LatencyHistogram::new(),
            key_start: None,
            idle_start: now,
            idle_cpu_s: 0.0,
            idle_repaints: 0,
            rng_state: 0x9E37_79B9_7F4A_7C15_u64,
            awaiting_frame: false,
            libass_play_hist: LatencyHistogram::new(),
            play_start: now,
            done_frames: 0,
            summary: WindowedBenchSummary::default(),
        }
    }

    fn next_rand_frame(&mut self, num_frames: usize) -> usize {
        self.rng_state ^= self.rng_state << 13;
        self.rng_state ^= self.rng_state >> 7;
        self.rng_state ^= self.rng_state << 17;
        #[allow(clippy::cast_possible_truncation)]
        {
            (self.rng_state as usize) % num_frames.max(1)
        }
    }
}

struct CachedEditGalley {
    text: String,
    wrap_bits: u32,
    is_dark: bool,
    galley: Arc<egui::Galley>,
}

struct SpikeApp {
    i18n: I18n,
    cached_locale: Locale,
    cached_headers: [String; 6],
    theme_mode: ThemeMode,
    palette: ThemePalette,
    last_theme_check: Instant,
    grid: GridState,
    cached_edit_galley: Option<CachedEditGalley>,
    syntax_scratch: Vec<syntax::HighlightSpan>,
    player: VideoPlayerHandle,
    video_info: Option<VideoInfo>,
    video_file_name: String,
    ass_source: AssSource,
    indexing_progress: Option<(i64, i64)>,
    playback_state: PlaybackState,
    loop_playback: bool,
    log_metrics: bool,
    pending_autoplay: bool,
    scale_mode: VideoScaleMode,
    seek_input: String,
    latest_generation: u64,
    frame_queue: VecDeque<ReadyFrame>,
    playback_clock: Option<PlaybackClock>,
    video_texture: Option<VideoTexture>,
    video_metrics: VideoMetrics,
    error_banner: Option<String>,
    process_start: Instant,
    ui_frame_count: u64,
    cold_start_ui_ms: Option<f64>,
    cold_start_present_ms: Option<f64>,
    scroll_hist: LatencyHistogram,
    keystroke_hist: LatencyHistogram,
    rerender_hist: LatencyHistogram,
    seek_hist: LatencyHistogram,
    step_hist: LatencyHistogram,
    keyframe_hist: LatencyHistogram,
    upload_hist: LatencyHistogram,
    select_all_ms: Option<f64>,
    open_first_frame_ms: Option<f64>,
    pending_keystroke_start: Option<Instant>,
    pending_rerender_start: Option<Instant>,
    pending_frame_op: Option<(PendingFrameOp, Instant)>,
    idle_window_start: Instant,
    idle_window_frames: u32,
    idle_cpu_start_s: f64,
    idle_repaints_per_sec: f64,
    idle_cpu_pct: f64,
    windowed_bench: Option<WindowedBenchDriver>,
    last_frame_instant: Instant,
    last_input_at: Instant,
    smoothed_frame_ms: f32,
    working_set_mib: f64,
    private_mib: f64,
    last_mem_check: Instant,
    last_metrics_log: Instant,
}

fn build_grid_headers(i18n: &I18n) -> [String; 6] {
    [
        i18n.tr("col-index"),
        i18n.tr("col-start"),
        i18n.tr("col-end"),
        i18n.tr("col-cps"),
        i18n.tr("col-style"),
        i18n.tr("col-text"),
    ]
}

struct CliMediaInit {
    player: VideoPlayerHandle,
    ass_source: AssSource,
    video_file_name: String,
    indexing_progress: Option<(i64, i64)>,
    pending_frame_op: Option<(PendingFrameOp, Instant)>,
}

impl SpikeApp {
    fn init_player_from_cli(ctx: &egui::Context, cli: &CliOptions) -> anyhow::Result<CliMediaInit> {
        let initial_ass = sample_ass::build_sample_ass(1920, 1080);
        let player = VideoPlayerHandle::spawn(initial_ass, ctx.clone())?;
        if cli.loop_playback {
            player
                .send(PlayerCmd::SetLoop(true))
                .map_err(anyhow::Error::msg)?;
        }
        let ass_source = if let Some(ass_path) = &cli.ass {
            let name = path_display_name(ass_path);
            player
                .send(PlayerCmd::LoadAssFile(ass_path.clone()))
                .map_err(anyhow::Error::msg)?;
            AssSource::File(name)
        } else {
            AssSource::BuiltInSample
        };

        let mut video_file_name = String::new();
        let mut indexing_progress = None;
        let mut pending_frame_op = None;
        if let Some(video_path) = &cli.video {
            video_file_name = path_display_name(video_path);
            indexing_progress = Some((0, 1));
            pending_frame_op = Some((PendingFrameOp::OpenFirstFrame, Instant::now()));
            player
                .open_video(video_path.clone())
                .map_err(anyhow::Error::msg)?;
        }
        Ok(CliMediaInit {
            player,
            ass_source,
            video_file_name,
            indexing_progress,
            pending_frame_op,
        })
    }

    fn new(
        cc: &eframe::CreationContext<'_>,
        cli: &CliOptions,
        process_start: Instant,
    ) -> anyhow::Result<Self> {
        let trace_startup = cli.bench != BenchMode::None || cli.log_metrics;
        let t_cb_enter_ms = process_start.elapsed().as_secs_f64() * 1000.0;
        let t_stage = Instant::now();
        configure_fonts(&cc.egui_ctx);
        let t_fonts_ms = t_stage.elapsed().as_secs_f64() * 1000.0;

        let i18n = I18n::new(cli.lang.unwrap_or(Locale::En))?;
        let cached_locale = i18n.locale();
        let cached_headers = build_grid_headers(&i18n);
        let theme_mode = cli.theme.unwrap_or(ThemeMode::FollowWindows);
        let palette = theme::resolve_palette(theme_mode);
        theme::apply_to_egui(&cc.egui_ctx, &palette);

        let t_p = Instant::now();
        let CliMediaInit {
            player,
            ass_source,
            video_file_name,
            indexing_progress,
            pending_frame_op,
        } = Self::init_player_from_cli(&cc.egui_ctx, cli)?;
        let t_player_ms = t_p.elapsed().as_secs_f64() * 1000.0;

        let now = Instant::now();
        let grid = GridState::generate(50_000);
        let t_grid_ms = now.elapsed().as_secs_f64() * 1000.0;
        if trace_startup {
            println!(
                "[startup-trace] wgpu_win={t_cb_enter_ms:.1}ms fonts={t_fonts_ms:.1}ms player_spawn={t_player_ms:.1}ms grid50k={t_grid_ms:.1}ms"
            );
        }
        let has_video = cli.video.is_some();
        let mem = ffi::process_memory_mib().unwrap_or_default();
        let cpu_s = ffi::process_cpu_seconds().unwrap_or(0.0);
        let windowed_bench =
            (cli.bench == BenchMode::Windowed).then(|| WindowedBenchDriver::new(has_video));
        Ok(Self {
            i18n,
            cached_locale,
            cached_headers,
            theme_mode,
            palette,
            last_theme_check: now,
            grid,
            cached_edit_galley: None,
            syntax_scratch: Vec::with_capacity(32),
            player,
            video_info: None,
            video_file_name,
            ass_source,
            indexing_progress,
            playback_state: PlaybackState::Paused,
            loop_playback: cli.loop_playback,
            log_metrics: cli.log_metrics,
            pending_autoplay: cli.autoplay,
            scale_mode: VideoScaleMode::FitPanel,
            seek_input: "0".to_owned(),
            latest_generation: 0,
            frame_queue: VecDeque::new(),
            playback_clock: None,
            video_texture: None,
            video_metrics: VideoMetrics::default(),
            error_banner: None,
            process_start,
            ui_frame_count: 0,
            cold_start_ui_ms: None,
            cold_start_present_ms: None,
            scroll_hist: LatencyHistogram::new(),
            keystroke_hist: LatencyHistogram::new(),
            rerender_hist: LatencyHistogram::new(),
            seek_hist: LatencyHistogram::new(),
            step_hist: LatencyHistogram::new(),
            keyframe_hist: LatencyHistogram::new(),
            upload_hist: LatencyHistogram::new(),
            select_all_ms: None,
            open_first_frame_ms: None,
            pending_keystroke_start: None,
            pending_rerender_start: None,
            pending_frame_op,
            idle_window_start: now,
            idle_window_frames: 0,
            idle_cpu_start_s: cpu_s,
            idle_repaints_per_sec: 0.0,
            idle_cpu_pct: 0.0,
            windowed_bench,
            last_frame_instant: now,
            last_input_at: now,
            smoothed_frame_ms: 16.6,
            working_set_mib: mem.working_set_mib,
            private_mib: mem.private_mib,
            last_mem_check: now,
            last_metrics_log: now,
        })
    }

    fn update_theme_if_needed(&mut self, ctx: &egui::Context) {
        if self.theme_mode == ThemeMode::FollowWindows
            && self.last_theme_check.elapsed() >= Duration::from_secs(2)
        {
            self.last_theme_check = Instant::now();
            let resolved = theme::resolve_palette(self.theme_mode);
            if resolved.is_dark != self.palette.is_dark {
                self.palette = resolved;
                theme::apply_to_egui(ctx, &self.palette);
            }
        }
    }

    fn set_theme_mode(&mut self, ctx: &egui::Context, mode: ThemeMode) {
        self.theme_mode = mode;
        self.palette = theme::resolve_palette(mode);
        theme::apply_to_egui(ctx, &self.palette);
    }

    fn send_player_cmd(&mut self, cmd: PlayerCmd) {
        if let Err(err) = self.player.send(cmd) {
            self.indexing_progress = None;
            self.playback_state = PlaybackState::Paused;
            self.playback_clock = None;
            self.error_banner = Some(err);
        }
    }

    fn open_video_file(&mut self, path: PathBuf) {
        self.pause_playback();
        self.indexing_progress = Some((0, 1));
        self.error_banner = None;
        self.pending_frame_op = Some((PendingFrameOp::OpenFirstFrame, Instant::now()));
        if let Err(err) = self.player.open_video(path) {
            self.indexing_progress = None;
            self.pending_frame_op = None;
            self.error_banner = Some(err);
        }
    }

    fn issue_frame_op(&mut self, op: PendingFrameOp, cmd: PlayerCmd) {
        self.pause_playback();
        self.pending_frame_op = Some((op, Instant::now()));
        self.send_player_cmd(cmd);
    }

    fn trigger_ass_rerender(&mut self) {
        if self.video_info.is_none() || self.playback_state == PlaybackState::Playing {
            return;
        }
        let Some(line) = self.grid.active_line() else {
            return;
        };
        let (w, h) = self
            .video_info
            .as_ref()
            .map_or((1920, 1080), |v| (v.width, v.height));
        let script_text = format!(
            "[Script Info]\nScriptType: v4.00+\nPlayResX: {w}\nPlayResY: {h}\nScaledBorderAndShadow: yes\n\n\
             [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
             Style: {},Segoe UI,54,&H00FFFFFF,&H0000FFFF,&H00181820,&H90000000,-1,0,0,0,100,100,0,0,1,3,1.5,2,40,40,36,1\n\n\
             [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
             Dialogue: 0,{},{},{},,,0,0,0,,{}\n",
            line.style, line.start_str, line.end_str, line.style, line.text
        );
        self.pending_rerender_start = Some(Instant::now());
        self.send_player_cmd(PlayerCmd::LoadAssMemory(script_text));
    }

    fn ass_source_display(&self) -> String {
        match &self.ass_source {
            AssSource::BuiltInSample => self.i18n.tr("sample-ass-name"),
            AssSource::File(name) => name.clone(),
        }
    }

    fn record_frame_op_latency(&mut self) {
        if let Some((op, started)) = self.pending_frame_op.take() {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            match op {
                PendingFrameOp::OpenFirstFrame => {
                    self.open_first_frame_ms = Some(ms);
                }
                PendingFrameOp::RandomSeek => self.seek_hist.record(ms),
                PendingFrameOp::StepOne => self.step_hist.record(ms),
                PendingFrameOp::KeyframeSeek => self.keyframe_hist.record(ms),
                PendingFrameOp::BenchWrap => {}
            }
            if let Some(wb) = self.windowed_bench.as_mut() {
                wb.awaiting_frame = false;
            }
        }
    }

    fn process_player_events(&mut self, frame: &eframe::Frame) {
        for evt in self.player.drain_events() {
            match evt {
                PlayerEvent::IndexingProgress { current, total } => {
                    self.indexing_progress = Some((current, total));
                }
                PlayerEvent::IndexingCancelled => {
                    self.indexing_progress = None;
                    self.pending_frame_op = None;
                }
                PlayerEvent::VideoReady { info, file_name } => {
                    self.indexing_progress = None;
                    self.error_banner = None;
                    self.video_file_name = file_name;
                    self.video_info = Some(info);
                    self.clear_frame_queue();
                    self.video_metrics = VideoMetrics::default();
                    "0".clone_into(&mut self.seek_input);
                    if self.pending_autoplay {
                        self.pending_autoplay = false;
                        self.start_playback();
                    }
                }
                PlayerEvent::Frame(ready) => {
                    if ready.generation > self.latest_generation {
                        self.latest_generation = ready.generation;
                        self.clear_frame_queue();
                    }
                    if ready.generation < self.latest_generation {
                        self.player.release_frame_slots(1);
                        self.player.recycle_buffer(ready.rgba);
                        continue;
                    }
                    if self.playback_state == PlaybackState::Playing {
                        self.frame_queue.push_back(ready);
                    } else {
                        self.upload_ready_frame(&ready, frame);
                        if let Some(t_ren) = self.pending_rerender_start.take() {
                            self.rerender_hist
                                .record(t_ren.elapsed().as_secs_f64() * 1000.0);
                        }
                        self.record_frame_op_latency();
                        self.player.release_frame_slots(1);
                        self.player.recycle_buffer(ready.rgba);
                    }
                }
                PlayerEvent::PlaybackEnded => {
                    self.playback_state = PlaybackState::Paused;
                    self.playback_clock = None;
                }
                PlayerEvent::AssError(msg) | PlayerEvent::Error(msg) => {
                    self.indexing_progress = None;
                    self.playback_state = PlaybackState::Paused;
                    self.playback_clock = None;
                    self.pending_frame_op = None;
                    self.pending_rerender_start = None;
                    self.error_banner = Some(msg);
                }
            }
        }

        if self.playback_state == PlaybackState::Playing {
            self.advance_playback_queue(frame);
        }
    }

    fn advance_playback_queue(&mut self, eframe_frame: &eframe::Frame) {
        let frame_interval_ms = self
            .video_info
            .as_ref()
            .map_or(41.7, |info| 1000.0 / info.fps.max(1.0));

        let step = pop_due_playback_frame(
            &mut self.frame_queue,
            &mut self.playback_clock,
            self.video_metrics.current_frame,
            self.video_metrics.rendered_frames,
            frame_interval_ms,
            Instant::now(),
        );
        self.player.release_frame_slots(step.popped_count);
        for buf in step.recycled_buffers {
            self.player.recycle_buffer(buf);
        }
        self.video_metrics.dropped_frames += step.dropped_count;

        if let Some(due) = step.due_frame {
            if let Some(wb) = self.windowed_bench.as_mut() {
                if matches!(wb.phase, BenchPhase::VideoPlayCollect) {
                    wb.libass_play_hist.record(f64::from(due.render_ms));
                }
            }
            self.upload_ready_frame(&due, eframe_frame);
            self.player.recycle_buffer(due.rgba);
        }
    }

    fn upload_ready_frame(&mut self, ready: &ReadyFrame, eframe_frame: &eframe::Frame) {
        let Some(render_state) = eframe_frame.wgpu_render_state() else {
            return;
        };

        let t_upload = Instant::now();
        let needs_recreate = self
            .video_texture
            .as_ref()
            .is_none_or(|t| t.width != ready.width || t.height != ready.height);

        if needs_recreate {
            let texture = render_state
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("hlouis_video_frame"),
                    size: wgpu::Extent3d {
                        width: ready.width,
                        height: ready.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut renderer = render_state.renderer.write();
            let egui_id = if let Some(existing) = self.video_texture.as_ref() {
                renderer.update_egui_texture_from_wgpu_texture(
                    &render_state.device,
                    &view,
                    wgpu::FilterMode::Linear,
                    existing.egui_id,
                );
                existing.egui_id
            } else {
                renderer.register_native_texture(
                    &render_state.device,
                    &view,
                    wgpu::FilterMode::Linear,
                )
            };
            self.video_texture = Some(VideoTexture {
                wgpu_texture: texture,
                egui_id,
                width: ready.width,
                height: ready.height,
            });
        }

        if let Some(tex) = self.video_texture.as_ref() {
            render_state.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex.wgpu_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &ready.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ready.width * 4),
                    rows_per_image: Some(ready.height),
                },
                wgpu::Extent3d {
                    width: ready.width,
                    height: ready.height,
                    depth_or_array_layers: 1,
                },
            );
        }

        let upload_ms = t_upload.elapsed().as_secs_f32() * 1000.0;
        self.upload_hist.record(f64::from(upload_ms));
        self.video_metrics.decode_ms = ready.decode_ms;
        self.video_metrics.render_ms = ready.render_ms;
        self.video_metrics.upload_ms = upload_ms;
        self.video_metrics.total_decode_ms += f64::from(ready.decode_ms);
        self.video_metrics.total_render_ms += f64::from(ready.render_ms);
        self.video_metrics.total_upload_ms += f64::from(upload_ms);
        self.video_metrics.max_decode_ms = self.video_metrics.max_decode_ms.max(ready.decode_ms);
        self.video_metrics.max_render_ms = self.video_metrics.max_render_ms.max(ready.render_ms);
        self.video_metrics.max_upload_ms = self.video_metrics.max_upload_ms.max(upload_ms);
        self.video_metrics.current_frame = ready.frame_index;
        self.video_metrics.current_pts_ms = ready.pts_ms;
        self.video_metrics.rendered_frames += 1;
        {
            use std::fmt::Write as _;
            self.seek_input.clear();
            let _ = write!(self.seek_input, "{}", ready.frame_index);
        }
    }

    fn clear_frame_queue(&mut self) {
        let len = self.frame_queue.len();
        while let Some(item) = self.frame_queue.pop_front() {
            self.player.recycle_buffer(item.rgba);
        }
        self.player.release_frame_slots(len);
    }

    fn start_playback(&mut self) {
        if self.video_info.is_some() {
            self.playback_state = PlaybackState::Playing;
            self.clear_frame_queue();
            // Anchor lazily on the first frame received after Play to avoid stale seek PTS.
            self.playback_clock = None;
            self.send_player_cmd(PlayerCmd::SetPlaying(true));
        }
    }

    fn pause_playback(&mut self) {
        self.playback_state = PlaybackState::Paused;
        self.playback_clock = None;
        self.clear_frame_queue();
        self.send_player_cmd(PlayerCmd::SetPlaying(false));
    }

    fn seek_to_frame(&mut self, target: usize) {
        self.issue_frame_op(PendingFrameOp::RandomSeek, PlayerCmd::SeekFrame(target));
    }

    fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("top_toolbar").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(self.i18n.tr("app-title"));
                ui.separator();

                if ui.button(self.i18n.tr("btn-open-video")).clicked() {
                    let filter_label = self.i18n.tr("dialog-filter-video");
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter(&filter_label, &["mkv", "mp4", "webm", "avi", "mov", "ts"])
                        .pick_file()
                    {
                        self.open_video_file(path);
                    }
                }

                if ui.button(self.i18n.tr("btn-open-ass")).clicked() {
                    let filter_label = self.i18n.tr("dialog-filter-ass");
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter(&filter_label, &["ass", "ssa"])
                        .pick_file()
                    {
                        self.ass_source = AssSource::File(path_display_name(&path));
                        self.send_player_cmd(PlayerCmd::LoadAssFile(path));
                    }
                }

                if ui.button(self.i18n.tr("btn-reset-ass")).clicked() {
                    let (w, h) = self
                        .video_info
                        .as_ref()
                        .map_or((1920, 1080), |v| (v.width, v.height));
                    self.ass_source = AssSource::BuiltInSample;
                    self.send_player_cmd(PlayerCmd::LoadAssMemory(sample_ass::build_sample_ass(
                        w, h,
                    )));
                }

                ui.separator();
                self.render_theme_selector(ui, &ctx);
                ui.separator();
                self.render_language_selector(ui);
            });
        });
    }

    fn render_theme_selector(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(self.i18n.tr("label-theme"));
        let mut selected_theme = self.theme_mode;
        egui::ComboBox::from_id_salt("theme_combo")
            .selected_text(match selected_theme {
                ThemeMode::Light => self.i18n.tr("theme-light"),
                ThemeMode::Dark => self.i18n.tr("theme-dark"),
                ThemeMode::FollowWindows => self.i18n.tr("theme-system"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut selected_theme,
                    ThemeMode::Light,
                    self.i18n.tr("theme-light"),
                );
                ui.selectable_value(
                    &mut selected_theme,
                    ThemeMode::Dark,
                    self.i18n.tr("theme-dark"),
                );
                ui.selectable_value(
                    &mut selected_theme,
                    ThemeMode::FollowWindows,
                    self.i18n.tr("theme-system"),
                );
            });
        if selected_theme != self.theme_mode {
            self.set_theme_mode(ctx, selected_theme);
        }
    }

    fn render_language_selector(&mut self, ui: &mut egui::Ui) {
        ui.label(self.i18n.tr("label-language"));
        let mut selected_locale = self.i18n.locale();
        egui::ComboBox::from_id_salt("lang_combo")
            .selected_text(match selected_locale {
                Locale::En => self.i18n.tr("lang-en"),
                Locale::Vi => self.i18n.tr("lang-vi"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selected_locale, Locale::En, self.i18n.tr("lang-en"));
                ui.selectable_value(&mut selected_locale, Locale::Vi, self.i18n.tr("lang-vi"));
            });
        if selected_locale != self.i18n.locale() {
            self.i18n.set_locale(selected_locale);
        }
    }

    fn render_video_panel(&mut self, ui: &mut egui::Ui, allow_cursor_blink: bool) -> bool {
        let mut seek_focused = false;
        egui::Panel::left("video_panel")
            .resizable(true)
            .default_size(580.0)
            .min_size(320.0)
            .show(ui, |ui| {
                self.render_video_status(ui);
                let ass_label = self.ass_source_display();
                ui.label(
                    egui::RichText::new(self.i18n.tr_args(
                        "status-ass-loaded",
                        &[("source", FluentValue::from(ass_label.as_str()))],
                    ))
                    .color(self.palette.text_muted),
                );
                seek_focused = self.render_transport_controls(ui, allow_cursor_blink);
                ui.separator();

                let avail = ui.available_size();
                let (viewport_rect, _resp) =
                    ui.allocate_exact_size(avail.max(Vec2::new(64.0, 64.0)), Sense::hover());
                ui.painter()
                    .rect_filled(viewport_rect, 0.0, self.palette.video_letterbox);

                let ppp = ui.ctx().pixels_per_point();
                if let Some(tex) = self.video_texture.as_ref() {
                    let target_rect = compute_crisp_video_rect(
                        viewport_rect,
                        tex.width,
                        tex.height,
                        ppp,
                        self.scale_mode,
                    );
                    ui.painter().with_clip_rect(viewport_rect).image(
                        tex.egui_id,
                        target_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }

                self.paint_metrics_overlay(ui, viewport_rect, ppp);
            });
        seek_focused
    }

    fn render_video_status(&mut self, ui: &mut egui::Ui) {
        if let Some(err) = &self.error_banner {
            egui::Frame::NONE
                .fill(self.palette.error_bg)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.colored_label(
                        self.palette.error_text,
                        self.i18n.tr_args(
                            "status-error",
                            &[("message", FluentValue::from(err.as_str()))],
                        ),
                    );
                });
        } else if let Some((cur, tot)) = self.indexing_progress {
            let frac = if tot > 0 {
                (cur as f32 / tot as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let pct_num = f64::from((frac * 100.0).round());
            ui.horizontal(|ui| {
                ui.add(
                    egui::ProgressBar::new(frac)
                        .desired_width((ui.available_width() - 90.0).max(120.0))
                        .text(self.i18n.tr_args(
                            "status-indexing",
                            &[("percent", FluentValue::from(pct_num))],
                        )),
                );
                if ui.button(self.i18n.tr("btn-cancel")).clicked() {
                    self.player.cancel_indexing();
                    self.indexing_progress = None;
                }
            });
        } else if let Some(info) = &self.video_info {
            let fps_str = format!("{:.3}", info.fps);
            ui.label(self.i18n.tr_args(
                "status-video-ready",
                &[
                    ("file", FluentValue::from(self.video_file_name.as_str())),
                    ("width", FluentValue::from(i64::from(info.width))),
                    ("height", FluentValue::from(i64::from(info.height))),
                    ("frames", fv_usize(info.num_frames)),
                    ("fps", FluentValue::from(fps_str.as_str())),
                    ("keyframes", fv_usize(info.keyframes.len())),
                ],
            ));
        } else {
            ui.label(self.i18n.tr("status-no-video"));
        }
    }

    fn render_transport_controls(&mut self, ui: &mut egui::Ui, allow_cursor_blink: bool) -> bool {
        let mut seek_focused = false;
        ui.horizontal_wrapped(|ui| {
            let has_video = self.video_info.is_some() && self.indexing_progress.is_none();
            ui.add_enabled_ui(has_video, |ui| {
                let play_label = if self.playback_state == PlaybackState::Playing {
                    self.i18n.tr("btn-pause")
                } else {
                    self.i18n.tr("btn-play")
                };
                if ui.button(play_label).clicked() {
                    if self.playback_state == PlaybackState::Playing {
                        self.pause_playback();
                    } else {
                        self.start_playback();
                    }
                }

                if ui
                    .checkbox(&mut self.loop_playback, self.i18n.tr("btn-loop"))
                    .changed()
                {
                    self.send_player_cmd(PlayerCmd::SetLoop(self.loop_playback));
                }

                if ui.button(self.i18n.tr("btn-prev-frame")).clicked() {
                    self.issue_frame_op(PendingFrameOp::StepOne, PlayerCmd::StepFrame(-1));
                }
                if ui.button(self.i18n.tr("btn-next-frame")).clicked() {
                    self.issue_frame_op(PendingFrameOp::StepOne, PlayerCmd::StepFrame(1));
                }
                if ui.button(self.i18n.tr("btn-prev-keyframe")).clicked() {
                    self.issue_frame_op(PendingFrameOp::KeyframeSeek, PlayerCmd::PrevKeyframe);
                }
                if ui.button(self.i18n.tr("btn-next-keyframe")).clicked() {
                    self.issue_frame_op(PendingFrameOp::KeyframeSeek, PlayerCmd::NextKeyframe);
                }

                ui.label(self.i18n.tr("label-seek-frame"));
                let seek_resp = ui
                    .scope(|ui| {
                        ui.visuals_mut().text_cursor.blink = allow_cursor_blink;
                        ui.add(egui::TextEdit::singleline(&mut self.seek_input).desired_width(64.0))
                    })
                    .inner;
                seek_focused = seek_resp.has_focus();
                let submit =
                    seek_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button(self.i18n.tr("btn-seek")).clicked() || submit {
                    if let Ok(target) = self.seek_input.trim().parse::<usize>() {
                        self.seek_to_frame(target);
                    }
                }
            });

            ui.separator();
            ui.label(self.i18n.tr("label-video-scale"));
            ui.selectable_value(
                &mut self.scale_mode,
                VideoScaleMode::FitPanel,
                self.i18n.tr("video-scale-fit"),
            );
            ui.selectable_value(
                &mut self.scale_mode,
                VideoScaleMode::Native1to1,
                self.i18n.tr("video-scale-native"),
            );
        });
        seek_focused
    }

    fn paint_metrics_overlay(&self, ui: &egui::Ui, viewport_rect: Rect, ppp: f32) {
        let overlay_pos = viewport_rect.left_top() + Vec2::new(8.0, 8.0);
        egui::Area::new(egui::Id::unique("metrics_overlay"))
            .fixed_pos(overlay_pos)
            .interactable(false)
            .fade_in(false)
            .show(ui.ctx(), |ui| {
                ui.spacing_mut().item_spacing.y = 1.5;
                egui::Frame::NONE
                    .fill(self.palette.overlay_bg)
                    .stroke(egui::Stroke::new(1.0, self.palette.overlay_border))
                    .inner_margin(6.0)
                    .corner_radius(4.0)
                    .show(ui, |ui| {
                        self.render_overlay_ui_section(ui, ppp);
                        if let Some(info) = &self.video_info {
                            ui.separator();
                            self.render_overlay_video_section(ui, info);
                        }
                    });
            });
    }

    fn render_overlay_ui_section(&self, ui: &mut egui::Ui, ppp: f32) {
        let text_color = self.palette.overlay_text;
        let fps = if self.smoothed_frame_ms > 0.0 {
            1000.0 / self.smoothed_frame_ms
        } else {
            0.0
        };
        ui.label(
            egui::RichText::new(self.i18n.tr("overlay-ui-title"))
                .size(11.5)
                .strong()
                .color(text_color),
        );
        paint_overlay_row(
            ui,
            self.i18n.tr_args(
                "overlay-fps",
                &[
                    ("fps", FluentValue::from(format!("{fps:.1}"))),
                    (
                        "ms",
                        FluentValue::from(format!("{:.2}", self.smoothed_frame_ms)),
                    ),
                ],
            ),
            text_color,
        );
        paint_overlay_row(
            ui,
            self.i18n.tr_args(
                "overlay-memory",
                &[("mb", FluentValue::from(format!("{:.1}", self.private_mib)))],
            ),
            text_color,
        );
        paint_overlay_row(
            ui,
            self.i18n.tr_args(
                "overlay-dpi",
                &[
                    ("scale", FluentValue::from(f64::from((ppp * 100.0).round()))),
                    ("ppp", FluentValue::from(format!("{ppp:.2}"))),
                ],
            ),
            text_color,
        );
        self.render_overlay_latency_rows(ui, text_color);
        paint_overlay_row(
            ui,
            self.i18n.tr_args(
                "overlay-idle",
                &[
                    (
                        "repaints",
                        FluentValue::from(format!("{:.1}", self.idle_repaints_per_sec)),
                    ),
                    (
                        "cpu",
                        FluentValue::from(format!("{:.2}", self.idle_cpu_pct)),
                    ),
                ],
            ),
            text_color,
        );
    }

    fn render_overlay_latency_rows(&self, ui: &mut egui::Ui, text_color: egui::Color32) {
        if let Some(cold_ms) = self.cold_start_present_ms.or(self.cold_start_ui_ms) {
            paint_overlay_row(
                ui,
                self.i18n.tr_args(
                    "overlay-cold-start",
                    &[("ms", FluentValue::from(format!("{cold_ms:.1}")))],
                ),
                text_color,
            );
        }
        if !self.keystroke_hist.is_empty() {
            let ks = self.keystroke_hist.summary();
            paint_overlay_row(
                ui,
                self.i18n.tr_args(
                    "overlay-keystroke",
                    &[
                        ("ms", FluentValue::from(format!("{:.2}", ks.last))),
                        ("p95", FluentValue::from(format!("{:.2}", ks.p95))),
                    ],
                ),
                text_color,
            );
        }
        if !self.scroll_hist.is_empty() {
            let sc = self.scroll_hist.summary();
            paint_overlay_row(
                ui,
                self.i18n.tr_args(
                    "overlay-grid-scroll",
                    &[
                        ("p50", FluentValue::from(format!("{:.2}", sc.p50))),
                        ("p95", FluentValue::from(format!("{:.2}", sc.p95))),
                        ("max", FluentValue::from(format!("{:.2}", sc.worst))),
                    ],
                ),
                text_color,
            );
        }
        if let Some(sel_ms) = self.select_all_ms {
            paint_overlay_row(
                ui,
                self.i18n.tr_args(
                    "overlay-select-all",
                    &[("ms", FluentValue::from(format!("{sel_ms:.3}")))],
                ),
                text_color,
            );
        }
        if !self.rerender_hist.is_empty() {
            paint_overlay_row(
                ui,
                self.i18n.tr_args(
                    "overlay-rerender",
                    &[(
                        "ms",
                        FluentValue::from(format!("{:.2}", self.rerender_hist.last())),
                    )],
                ),
                text_color,
            );
        }
    }

    fn render_overlay_video_section(&self, ui: &mut egui::Ui, info: &VideoInfo) {
        let text_color = self.palette.overlay_text;
        ui.label(
            egui::RichText::new(self.i18n.tr("overlay-video-title"))
                .size(11.5)
                .strong()
                .color(text_color),
        );
        paint_overlay_row(
            ui,
            self.i18n.tr_args(
                "overlay-decode",
                &[(
                    "ms",
                    FluentValue::from(format!("{:.2}", self.video_metrics.decode_ms)),
                )],
            ),
            text_color,
        );
        paint_overlay_row(
            ui,
            self.i18n.tr_args(
                "overlay-render",
                &[(
                    "ms",
                    FluentValue::from(format!("{:.2}", self.video_metrics.render_ms)),
                )],
            ),
            text_color,
        );
        paint_overlay_row(
            ui,
            self.i18n.tr_args(
                "overlay-upload",
                &[(
                    "ms",
                    FluentValue::from(format!("{:.2}", self.video_metrics.upload_ms)),
                )],
            ),
            text_color,
        );
        if !self.seek_hist.is_empty() {
            let sk = self.seek_hist.summary();
            paint_overlay_row(
                ui,
                self.i18n.tr_args(
                    "overlay-seek",
                    &[
                        ("ms", FluentValue::from(format!("{:.1}", sk.last))),
                        ("p95", FluentValue::from(format!("{:.1}", sk.p95))),
                        ("max", FluentValue::from(format!("{:.1}", sk.worst))),
                    ],
                ),
                text_color,
            );
        }

        let total_frames = self.video_metrics.rendered_frames + self.video_metrics.dropped_frames;
        let drop_pct = if total_frames > 0 {
            (self.video_metrics.dropped_frames as f64 / total_frames as f64) * 100.0
        } else {
            0.0
        };
        paint_overlay_row(
            ui,
            self.i18n.tr_args(
                "overlay-dropped",
                &[
                    ("dropped", fv_u64(self.video_metrics.dropped_frames)),
                    ("rendered", fv_u64(self.video_metrics.rendered_frames)),
                    ("pct", FluentValue::from(format!("{drop_pct:.2}"))),
                ],
            ),
            text_color,
        );

        self.render_overlay_frame_pos_row(ui, info, text_color);
    }

    fn render_overlay_frame_pos_row(
        &self,
        ui: &mut egui::Ui,
        info: &VideoInfo,
        text_color: egui::Color32,
    ) {
        let time_str = format_ms(self.video_metrics.current_pts_ms);
        ui.horizontal(|ui| {
            paint_overlay_row(
                ui,
                self.i18n.tr_args(
                    "overlay-frame-pos",
                    &[
                        ("frame", fv_usize(self.video_metrics.current_frame)),
                        ("total", fv_usize(info.num_frames)),
                        ("time", FluentValue::from(time_str.as_str())),
                    ],
                ),
                text_color,
            );
            let is_kf = info.is_keyframe(self.video_metrics.current_frame);
            let (badge_key, badge_color) = if is_kf {
                ("overlay-keyframe-yes", self.palette.keyframe_badge)
            } else {
                ("overlay-keyframe-no", self.palette.non_keyframe_badge)
            };
            ui.label(
                egui::RichText::new(self.i18n.tr(badge_key))
                    .size(11.0)
                    .strong()
                    .monospace()
                    .color(badge_color),
            );
        });
    }

    fn render_editor_and_grid(
        &mut self,
        ui: &mut egui::Ui,
        seek_focused: bool,
        allow_cursor_blink: bool,
    ) {
        egui::CentralPanel::default().show(ui, |ui| {
            let edit_focused = self.render_edit_box(ui, allow_cursor_blink);
            ui.separator();

            ui.horizontal(|ui| {
                ui.label(self.i18n.tr_args(
                    "grid-summary",
                    &[
                        ("total", fv_usize(self.grid.lines.len())),
                        ("selected", fv_usize(self.grid.selected.len())),
                        ("active", fv_usize(self.grid.active_row + 1)),
                    ],
                ));
            });

            if !edit_focused && !seek_focused {
                let (up, down, shift, ctrl_a) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::ArrowUp),
                        i.key_pressed(egui::Key::ArrowDown),
                        i.modifiers.shift,
                        (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::A),
                    )
                });
                if ctrl_a {
                    let t_sel = Instant::now();
                    self.grid.select_all();
                    self.select_all_ms = Some(t_sel.elapsed().as_secs_f64() * 1000.0);
                } else if up {
                    self.grid.move_active(-1, shift);
                } else if down {
                    self.grid.move_active(1, shift);
                }
            }

            self.render_grid_table(ui);
        });
    }

    fn render_edit_box_header(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(self.i18n.tr("edit-section-title"));
            ui.separator();
            if let Some(line) = self.grid.active_line() {
                let chars = grid::visible_char_count(&line.text);
                ui.label(self.i18n.tr_args(
                    "edit-line-badge",
                    &[
                        ("line", fv_usize(line.number)),
                        ("start", FluentValue::from(line.start_str.as_str())),
                        ("end", FluentValue::from(line.end_str.as_str())),
                        ("chars", fv_usize(chars)),
                        ("cps", FluentValue::from(line.cps_str.as_str())),
                    ],
                ));
            }
        });

        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(self.i18n.tr("edit-ime-hint"))
                    .small()
                    .color(self.palette.text_muted),
            );
            ui.separator();
            for (key, color) in [
                ("legend-tags", self.palette.syn_tag),
                ("legend-values", self.palette.syn_value),
                ("legend-karaoke", self.palette.syn_karaoke),
                ("legend-escapes", self.palette.syn_escape),
                ("legend-drawing", self.palette.syn_drawing_cmd),
            ] {
                ui.label(egui::RichText::new(self.i18n.tr(key)).small().color(color));
            }
        });
    }

    fn render_edit_box(&mut self, ui: &mut egui::Ui, allow_cursor_blink: bool) -> bool {
        self.render_edit_box_header(ui);

        let palette = self.palette;
        let mono_font = FontId::monospace(15.0);
        let cache = &mut self.cached_edit_galley;
        let syntax_scratch = &mut self.syntax_scratch;
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
            let text_str = text.as_str();
            let wrap_bits = wrap_width.to_bits();
            if let Some(cached) = cache.as_ref() {
                if cached.wrap_bits == wrap_bits
                    && cached.is_dark == palette.is_dark
                    && cached.text == text_str
                {
                    return Arc::clone(&cached.galley);
                }
            }
            let job = syntax::build_layout_job_with_scratch(
                text_str,
                &mono_font,
                wrap_width,
                &palette,
                syntax_scratch,
            );
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            *cache = Some(CachedEditGalley {
                text: text_str.to_owned(),
                wrap_bits,
                is_dark: palette.is_dark,
                galley: Arc::clone(&galley),
            });
            galley
        };

        let key_event_now = ui.input(|i| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Text(_)
                        | egui::Event::Key {
                            pressed: true,
                            key: egui::Key::Backspace | egui::Key::Delete | egui::Key::Enter,
                            ..
                        }
                )
            })
        });
        let key_at = Instant::now();

        let mut edit_focused = false;
        let mut line_changed = false;
        if let Some(line) = self.grid.active_line_mut() {
            let edit_resp = ui
                .scope(|ui| {
                    ui.visuals_mut().text_cursor.blink = allow_cursor_blink;
                    ui.add(
                        egui::TextEdit::multiline(&mut line.text)
                            .id_salt(("edit_box", line.number))
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(4)
                            .layouter(&mut layouter),
                    )
                })
                .inner;
            edit_focused = edit_resp.has_focus();
            if edit_resp.changed() {
                line.recompute_cps();
                line_changed = true;
                if key_event_now {
                    self.pending_keystroke_start = Some(key_at);
                }
            }
        }
        if line_changed {
            self.trigger_ass_rerender();
        }
        edit_focused
    }

    fn render_grid_table(&mut self, ui: &mut egui::Ui) {
        if self.i18n.locale() != self.cached_locale {
            self.cached_locale = self.i18n.locale();
            self.cached_headers = build_grid_headers(&self.i18n);
        }
        let headers = self.cached_headers.each_ref().map(String::as_str);
        hlouis_sub_edit::render_grid_table(ui, &mut self.grid, &self.palette, headers);
    }

    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    fn next_playback_repaint_delay(&self) -> Duration {
        let fallback_ms = self
            .video_info
            .as_ref()
            .map_or(16.0, |info| (1000.0 / info.fps.max(1.0)).clamp(4.0, 16.0));
        let Some(front) = self.frame_queue.front() else {
            return Duration::from_millis(fallback_ms.round() as u64);
        };
        let Some(clock) = &self.playback_clock else {
            return Duration::from_millis(4);
        };
        let elapsed_ms = clock.start_instant.elapsed().as_secs_f64() * 1000.0;
        let current_media_ms = clock.start_pts_ms + elapsed_ms;
        let wait_ms = (front.pts_ms as f64 - current_media_ms).clamp(2.0, 16.0);
        Duration::from_millis(wait_ms.round() as u64)
    }

    fn record_startup_and_idle(&mut self, ctx: &egui::Context, now: Instant, has_input: bool) {
        if self.ui_frame_count == 0 {
            let ui_ms = self.process_start.elapsed().as_secs_f64() * 1000.0;
            self.cold_start_ui_ms = Some(ui_ms);
            ctx.request_repaint();
        } else if self.ui_frame_count == 1 && self.cold_start_present_ms.is_none() {
            let present_ms = self.process_start.elapsed().as_secs_f64() * 1000.0;
            self.cold_start_present_ms = Some(present_ms);
            let ui_ms = self.cold_start_ui_ms.unwrap_or(present_ms);
            println!(
                "[startup] cold_start_ui_ms={ui_ms:.2} cold_start_first_present_ms={present_ms:.2}"
            );
        }

        if let Some(t_key) = self.pending_keystroke_start.take() {
            self.keystroke_hist
                .record(t_key.elapsed().as_secs_f64() * 1000.0);
        }

        if self.playback_state == PlaybackState::Paused
            && !has_input
            && self.indexing_progress.is_none()
            && self.windowed_bench.is_none()
            && self.ui_frame_count > 2
        {
            self.idle_window_frames = self.idle_window_frames.saturating_add(1);
            let elapsed_s = now.duration_since(self.idle_window_start).as_secs_f64();
            if elapsed_s >= 1.0 {
                let cpu_now = ffi::process_cpu_seconds().unwrap_or(self.idle_cpu_start_s);
                self.idle_repaints_per_sec = f64::from(self.idle_window_frames) / elapsed_s;
                self.idle_cpu_pct =
                    ((cpu_now - self.idle_cpu_start_s).max(0.0) / elapsed_s) * 100.0;
                self.idle_window_start = now;
                self.idle_window_frames = 0;
                self.idle_cpu_start_s = cpu_now;
            }
        } else if has_input || self.playback_state == PlaybackState::Playing {
            self.idle_window_start = now;
            self.idle_window_frames = 0;
            self.idle_cpu_start_s = ffi::process_cpu_seconds().unwrap_or(self.idle_cpu_start_s);
        }
    }

    fn step_windowed_bench(&mut self, ctx: &egui::Context) -> bool {
        let Some(mut wb) = self.windowed_bench.take() else {
            return false;
        };
        let mut record_scroll_cpu = false;
        match wb.phase {
            BenchPhase::AwaitColdPresent => {
                if self.ui_frame_count >= 2 {
                    wb.summary.cold_start_ui_ms = self.cold_start_ui_ms.unwrap_or(0.0);
                    wb.summary.cold_start_present_ms = self.cold_start_present_ms.unwrap_or(0.0);
                    wb.phase = BenchPhase::Setup200k;
                    ctx.request_repaint();
                }
            }
            BenchPhase::Setup200k => {
                self.run_bench_setup_200k(&mut wb);
                ctx.request_repaint();
            }
            BenchPhase::Scroll200k(step) => {
                record_scroll_cpu = self.step_bench_scroll(ctx, &mut wb, step);
            }
            BenchPhase::SelectAll200k => {
                let t_sel = Instant::now();
                self.grid.select_all();
                let sel_ms = t_sel.elapsed().as_secs_f64() * 1000.0;
                self.select_all_ms = Some(sel_ms);
                wb.summary.select_all_200k_ms = sel_ms;
                self.grid.click_row(0, false, false);
                wb.phase = BenchPhase::Keystrokes(0);
                wb.key_start = None;
                ctx.request_repaint();
            }
            BenchPhase::Keystrokes(n) => self.step_bench_keystroke(ctx, &mut wb, n),
            BenchPhase::IdleSettle => {
                let elapsed = wb.idle_start.elapsed();
                if elapsed >= Duration::from_millis(280) {
                    wb.idle_start = Instant::now();
                    wb.idle_cpu_s = ffi::process_cpu_seconds().unwrap_or(0.0);
                    wb.idle_repaints = 0;
                    wb.phase = BenchPhase::IdleWait;
                    ctx.request_repaint_after(Duration::from_millis(1550));
                } else {
                    let rem = Duration::from_millis(290)
                        .saturating_sub(elapsed)
                        .max(Duration::from_millis(20));
                    ctx.request_repaint_after(rem);
                }
            }
            BenchPhase::IdleWait => self.step_bench_idle_wait(ctx, &mut wb),
            BenchPhase::VideoAwaitReady
            | BenchPhase::VideoSeeks(_)
            | BenchPhase::VideoSteps(_)
            | BenchPhase::VideoKeyframes(_)
            | BenchPhase::VideoPlayStart
            | BenchPhase::VideoPlayCollect => self.step_windowed_video_bench(ctx, &mut wb),
            BenchPhase::Done => {
                if wb.done_frames == 0 {
                    bench_ui::print_windowed_report(&wb.summary);
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
                wb.done_frames = wb.done_frames.saturating_add(1);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                if wb.done_frames >= 2 {
                    std::process::exit(0);
                }
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        self.windowed_bench = Some(wb);
        record_scroll_cpu
    }

    fn step_bench_scroll(
        &mut self,
        ctx: &egui::Context,
        wb: &mut WindowedBenchDriver,
        step: usize,
    ) -> bool {
        let dt_ms = wb.last_tick.elapsed().as_secs_f64() * 1000.0;
        wb.last_tick = Instant::now();
        let record_cpu = step >= 5;
        if record_cpu {
            wb.scroll_present_hist.record(dt_ms);
        }
        if step < 125 {
            self.grid.active_row = (self.grid.active_row + 8).min(199_999);
            self.grid.scroll_to_active = true;
            wb.phase = BenchPhase::Scroll200k(step + 1);
            ctx.request_repaint();
        } else {
            wb.summary.scroll_cpu = wb.scroll_cpu_hist.summary();
            wb.summary.scroll_present = wb.scroll_present_hist.summary();
            wb.phase = BenchPhase::SelectAll200k;
            ctx.request_repaint();
        }
        record_cpu
    }

    fn step_bench_keystroke(
        &mut self,
        ctx: &egui::Context,
        wb: &mut WindowedBenchDriver,
        n: usize,
    ) {
        if let Some(t_k) = wb.key_start.take() {
            let ms = t_k.elapsed().as_secs_f64() * 1000.0;
            wb.keystroke_hist.record(ms);
            self.keystroke_hist.record(ms);
        }
        if n < 200 {
            let t_now = Instant::now();
            if let Some(line) = self.grid.active_line_mut() {
                if n % 5 == 4 && line.text.len() > 20 {
                    line.text.pop();
                } else {
                    let ch = char::from(b'a' + u8::try_from(n % 26).unwrap_or(0));
                    line.text.push(ch);
                }
                line.recompute_cps();
            }
            self.cached_edit_galley = None;
            wb.key_start = Some(t_now);
            wb.phase = BenchPhase::Keystrokes(n + 1);
            ctx.request_repaint();
        } else {
            wb.summary.keystroke_present = wb.keystroke_hist.summary();
            self.last_input_at = Instant::now()
                .checked_sub(Duration::from_secs(3))
                .unwrap_or_else(Instant::now);
            wb.idle_start = Instant::now();
            wb.phase = BenchPhase::IdleSettle;
            ctx.request_repaint_after(Duration::from_millis(300));
        }
    }

    fn step_bench_idle_wait(&mut self, ctx: &egui::Context, wb: &mut WindowedBenchDriver) {
        let elapsed_s = wb.idle_start.elapsed().as_secs_f64();
        if elapsed_s < 1.5 {
            wb.idle_repaints = wb.idle_repaints.saturating_add(1);
            let rem_s = (1.55 - elapsed_s).max(0.05);
            ctx.request_repaint_after(Duration::from_secs_f64(rem_s));
            return;
        }
        let cpu_now = ffi::process_cpu_seconds().unwrap_or(wb.idle_cpu_s);
        wb.summary.idle_repaints_per_sec = f64::from(wb.idle_repaints) / elapsed_s;
        wb.summary.idle_cpu_pct = ((cpu_now - wb.idle_cpu_s).max(0.0) / elapsed_s) * 100.0;
        wb.summary.private_mib_no_video =
            ffi::process_memory_mib().map_or(self.private_mib, |m| m.private_mib);
        self.idle_repaints_per_sec = wb.summary.idle_repaints_per_sec;
        self.idle_cpu_pct = wb.summary.idle_cpu_pct;
        if wb.has_video {
            wb.phase = BenchPhase::VideoAwaitReady;
        } else {
            wb.phase = BenchPhase::Done;
        }
        ctx.request_repaint();
    }

    fn run_bench_setup_200k(&mut self, wb: &mut WindowedBenchDriver) {
        if let Ok((open_ms, save_ms, mut script_20k)) = bench_ui::bench_open_save_20k_ass() {
            wb.summary.open_20k_ms = open_ms;
            wb.summary.save_20k_ms = save_ms;
            if let Ok((ren_20k, ren_single)) = bench_ui::bench_subtitle_rerender(&mut script_20k) {
                wb.summary.rerender_20k = ren_20k;
                wb.summary.rerender_single = ren_single;
            }
        }
        let t_gen = Instant::now();
        self.grid = GridState::generate(200_000);
        wb.summary.generate_200k_ms = t_gen.elapsed().as_secs_f64() * 1000.0;
        self.last_input_at = Instant::now();
        wb.last_tick = Instant::now();
        wb.phase = BenchPhase::Scroll200k(0);
    }

    fn step_windowed_video_bench(&mut self, ctx: &egui::Context, wb: &mut WindowedBenchDriver) {
        let Some(info) = self.video_info.as_ref() else {
            return;
        };
        let num_frames = info.num_frames.max(1);
        match wb.phase {
            BenchPhase::VideoAwaitReady => {
                if self.indexing_progress.is_none() && self.open_first_frame_ms.is_some() {
                    wb.awaiting_frame = false;
                    self.seek_hist.clear();
                    self.step_hist.clear();
                    self.keyframe_hist.clear();
                    wb.phase = BenchPhase::VideoSeeks(0);
                    ctx.request_repaint();
                }
            }
            BenchPhase::VideoSeeks(n) => {
                if !wb.awaiting_frame {
                    if n < 200 {
                        let target = wb.next_rand_frame(num_frames);
                        wb.awaiting_frame = true;
                        wb.phase = BenchPhase::VideoSeeks(n + 1);
                        self.seek_to_frame(target);
                    } else {
                        wb.phase = BenchPhase::VideoSteps(0);
                        ctx.request_repaint();
                    }
                }
            }
            BenchPhase::VideoSteps(n) => {
                if !wb.awaiting_frame {
                    if n < 50 {
                        let delta = if n < 25 { 1 } else { -1 };
                        wb.awaiting_frame = true;
                        wb.phase = BenchPhase::VideoSteps(n + 1);
                        self.issue_frame_op(PendingFrameOp::StepOne, PlayerCmd::StepFrame(delta));
                    } else {
                        wb.phase = BenchPhase::VideoKeyframes(0);
                        self.issue_frame_op(PendingFrameOp::KeyframeSeek, PlayerCmd::SeekFrame(0));
                        wb.awaiting_frame = true;
                    }
                }
            }
            BenchPhase::VideoKeyframes(n) => {
                if !wb.awaiting_frame {
                    if n >= 20 {
                        wb.phase = BenchPhase::VideoPlayStart;
                        ctx.request_repaint();
                    } else if info
                        .next_keyframe(self.video_metrics.current_frame)
                        .is_none()
                    {
                        wb.awaiting_frame = true;
                        self.issue_frame_op(PendingFrameOp::BenchWrap, PlayerCmd::SeekFrame(0));
                    } else {
                        wb.awaiting_frame = true;
                        wb.phase = BenchPhase::VideoKeyframes(n + 1);
                        self.issue_frame_op(PendingFrameOp::KeyframeSeek, PlayerCmd::NextKeyframe);
                    }
                }
            }
            BenchPhase::VideoPlayStart => {
                self.upload_hist.clear();
                wb.libass_play_hist.clear();
                self.video_metrics.dropped_frames = 0;
                self.video_metrics.rendered_frames = 0;
                wb.play_start = Instant::now();
                self.loop_playback = true;
                self.send_player_cmd(PlayerCmd::SetLoop(true));
                self.start_playback();
                wb.phase = BenchPhase::VideoPlayCollect;
                ctx.request_repaint_after(Duration::from_secs(3));
            }
            BenchPhase::VideoPlayCollect if wb.play_start.elapsed() >= Duration::from_secs(3) => {
                self.pause_playback();
                let priv_mib =
                    ffi::process_memory_mib().map_or(self.private_mib, |m| m.private_mib);
                wb.summary.video = Some(WindowedVideoSummary {
                    open_first_frame_ms: self.open_first_frame_ms.unwrap_or(0.0),
                    seeks_200: self.seek_hist.summary(),
                    step_1_50: self.step_hist.summary(),
                    next_kf_20: self.keyframe_hist.summary(),
                    libass_playback: wb.libass_play_hist.summary(),
                    upload_playback: self.upload_hist.summary(),
                    dropped_frames: self.video_metrics.dropped_frames,
                    rendered_frames: self.video_metrics.rendered_frames,
                    private_mib_with_video: priv_mib,
                });
                wb.phase = BenchPhase::Done;
                ctx.request_repaint();
            }
            _ => {}
        }
    }
}

impl eframe::App for SpikeApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        let dt_ms = (now - self.last_frame_instant).as_secs_f32() * 1000.0;
        self.last_frame_instant = now;
        if dt_ms > 0.0 && (self.playback_state == PlaybackState::Playing || dt_ms < 100.0) {
            self.smoothed_frame_ms = self.smoothed_frame_ms * 0.9 + dt_ms * 0.1;
        }
        if self.last_mem_check.elapsed() >= Duration::from_millis(500) {
            self.last_mem_check = now;
            if let Some(mem) = ffi::process_memory_mib() {
                self.working_set_mib = mem.working_set_mib;
                self.private_mib = mem.private_mib;
            }
        }

        let (has_input, scrolled) = ui.input(|i| {
            let scrolled = i.smooth_scroll_delta != Vec2::ZERO;
            let input =
                !i.events.is_empty() || i.pointer.any_down() || i.pointer.is_moving() || scrolled;
            (input, scrolled)
        });
        if has_input && self.windowed_bench.is_none() {
            self.last_input_at = now;
        }

        self.process_player_events(frame);
        self.record_startup_and_idle(&ctx, now, has_input);
        let record_bench_scroll = self.step_windowed_bench(&ctx);

        let allow_cursor_blink = self.playback_state == PlaybackState::Playing
            || self.indexing_progress.is_some()
            || (self.windowed_bench.is_none()
                && now.duration_since(self.last_input_at) < Duration::from_secs(2));

        self.update_theme_if_needed(&ctx);
        self.log_periodic_metrics(now);

        self.render_top_bar(ui);
        let seek_focused = self.render_video_panel(ui, allow_cursor_blink);
        self.render_editor_and_grid(ui, seek_focused, allow_cursor_blink);

        let cpu_ms = now.elapsed().as_secs_f64() * 1000.0;
        if scrolled {
            self.scroll_hist.record(cpu_ms);
        }
        if record_bench_scroll {
            if let Some(wb) = self.windowed_bench.as_mut() {
                wb.scroll_cpu_hist.record(cpu_ms);
            }
        }
        self.ui_frame_count = self.ui_frame_count.saturating_add(1);

        if self.playback_state == PlaybackState::Playing {
            ctx.request_repaint_after(self.next_playback_repaint_delay());
        }
    }
}

impl SpikeApp {
    fn log_periodic_metrics(&mut self, now: Instant) {
        if !self.log_metrics || self.last_metrics_log.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.last_metrics_log = now;
        let fps = if self.smoothed_frame_ms > 0.0 {
            1000.0 / self.smoothed_frame_ms
        } else {
            0.0
        };
        let n = (self.video_metrics.rendered_frames.max(1)) as f64;
        let avg_dec = self.video_metrics.total_decode_ms / n;
        let avg_ren = self.video_metrics.total_render_ms / n;
        let avg_upl = self.video_metrics.total_upload_ms / n;
        let sk = self.seek_hist.summary();
        let sc = self.scroll_hist.summary();
        println!(
            "[metrics] ui_fps={fps:.1} frame_ms={:.2} priv_mib={:.1} idle_repaints={:.1}/s idle_cpu={:.2}% scroll_p95={:.2} key_p95={:.2} seek_p95={:.1} video_frame={} avg_dec={avg_dec:.2} avg_ren={avg_ren:.2} avg_upl={avg_upl:.2} max_upl={:.2} rendered={} dropped={}",
            self.smoothed_frame_ms,
            self.private_mib,
            self.idle_repaints_per_sec,
            self.idle_cpu_pct,
            sc.p95,
            self.keystroke_hist.summary().p95,
            sk.p95,
            self.video_metrics.current_frame,
            self.video_metrics.max_upload_ms,
            self.video_metrics.rendered_frames,
            self.video_metrics.dropped_frames
        );
    }
}

fn paint_overlay_row(ui: &mut egui::Ui, text: String, color: egui::Color32) {
    ui.label(
        egui::RichText::new(text)
            .size(11.0)
            .monospace()
            .color(color),
    );
}

fn path_display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn fv_usize(n: usize) -> FluentValue<'static> {
    FluentValue::from(i64::try_from(n).unwrap_or(i64::MAX))
}

fn fv_u64(n: u64) -> FluentValue<'static> {
    FluentValue::from(i64::try_from(n).unwrap_or(i64::MAX))
}

fn compute_crisp_video_rect(
    viewport: Rect,
    video_w: u32,
    video_h: u32,
    pixels_per_point: f32,
    scale_mode: VideoScaleMode,
) -> Rect {
    let vw = (video_w.max(1)) as f32;
    let vh = (video_h.max(1)) as f32;
    let ppp = pixels_per_point.max(0.5);

    let (disp_w, disp_h) = match scale_mode {
        VideoScaleMode::Native1to1 => (vw / ppp, vh / ppp),
        VideoScaleMode::FitPanel => {
            let scale = (viewport.width() / vw)
                .min(viewport.height() / vh)
                .max(0.05);
            (vw * scale, vh * scale)
        }
    };

    let center = viewport.center();
    let raw_min_x = center.x - disp_w * 0.5;
    let raw_min_y = center.y - disp_h * 0.5;
    // Snap corners to exact physical pixel grid for crisp rendering on 125%/150%/200% DPI.
    let min_x = (raw_min_x * ppp).round() / ppp;
    let min_y = (raw_min_y * ppp).round() / ppp;
    let max_x = ((raw_min_x + disp_w) * ppp).round() / ppp;
    let max_y = ((raw_min_y + disp_h) * ppp).round() / ppp;
    Rect::from_min_max(Pos2::new(min_x, min_y), Pos2::new(max_x, max_y))
}

fn format_ms(ms: i64) -> String {
    let total_ms = ms.max(0);
    let total_sec = total_ms / 1000;
    let millis = total_ms % 1000;
    let ss = total_sec % 60;
    let total_min = total_sec / 60;
    let mm = total_min % 60;
    let hh = total_min / 60;
    format!("{hh}:{mm:02}:{ss:02}.{millis:03}")
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_owned());
    let fonts_dir = PathBuf::from(windir).join("Fonts");

    let mut add_font = |name: &str, file_name: &str, families: &[FontFamily]| -> bool {
        if let Ok(bytes) = std::fs::read(fonts_dir.join(file_name)) {
            fonts
                .font_data
                .insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
            for family in families {
                if let Some(list) = fonts.families.get_mut(family) {
                    list.insert(0, name.to_owned());
                }
            }
            true
        } else {
            false
        }
    };

    // Insert Japanese fallback first so Latin/Vietnamese fonts end up ahead of it at index 0.
    let jp_loaded = add_font(
        "win_yugothic",
        "YuGothR.ttc",
        &[FontFamily::Proportional, FontFamily::Monospace],
    );
    if !jp_loaded {
        let _ = add_font(
            "win_meiryo",
            "meiryo.ttc",
            &[FontFamily::Proportional, FontFamily::Monospace],
        );
    }

    let _ = add_font("win_segoe_ui", "segoeui.ttf", &[FontFamily::Proportional]);
    let _ = add_font("win_consolas", "consola.ttf", &[FontFamily::Monospace]);

    ctx.set_fonts(fonts);
}

fn main() -> anyhow::Result<()> {
    let process_start = Instant::now();
    let mut cli = CliOptions::parse();
    if cli.bench != BenchMode::None && cli.video.is_none() {
        if let Some(env_vid) = std::env::var_os("HLOUIS_TEST_VIDEO") {
            let p = PathBuf::from(env_vid);
            if p.exists() {
                cli.video = Some(p);
            }
        }
    }
    if cli.bench == BenchMode::Headless {
        return bench_ui::run_headless_bench(cli.video.as_deref());
    }

    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(ref mut setup) = wgpu_options.wgpu_setup {
        // Default to DX12|GL + LowPower on Windows so dual-GPU laptops avoid waking the discrete
        // GPU driver heap and avoid Intel Vulkan driver access violations during idle present.
        if std::env::var_os("WGPU_BACKEND").is_none() {
            // DX12 alone avoids probing/initializing OpenGL WGL ICD drivers at cold startup.
            setup.instance_descriptor.backends = wgpu::Backends::DX12;
        }
        if std::env::var_os("WGPU_POWER_PREF").is_none() {
            setup.power_preference = wgpu::PowerPreference::LowPower;
        }
        setup.device_descriptor = Arc::new(|adapter| {
            let base_limits = if adapter.get_info().backend == wgpu::Backend::Gl {
                wgpu::Limits::downlevel_webgl2_defaults()
            } else {
                wgpu::Limits::default()
            };
            wgpu::DeviceDescriptor {
                label: Some("hlouis_wgpu_device"),
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: 8192,
                    ..base_limits
                },
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            }
        });
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 740.0])
            .with_min_inner_size([840.0, 520.0])
            .with_title("hlouis-sub-edit"),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };

    eframe::run_native(
        "hlouis-sub-edit",
        native_options,
        Box::new(move |cc| Ok(Box::new(SpikeApp::new(cc, &cli, process_start)?))),
    )
    .map_err(|e| anyhow::anyhow!("eframe failed: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crisp_video_rect_snaps_on_hidpi() {
        let viewport = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(650.0, 500.0));
        for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
            let fit = compute_crisp_video_rect(viewport, 1920, 1080, ppp, VideoScaleMode::FitPanel);
            assert!(fit.width() > 0.0 && fit.height() > 0.0);
            for coord in [fit.min.x, fit.min.y, fit.max.x, fit.max.y] {
                let phys = coord * ppp;
                assert!(
                    (phys - phys.round()).abs() < 1e-4,
                    "coord {coord} at {ppp}x not snapped to physical pixel ({phys})"
                );
            }
            let aspect = fit.width() / fit.height();
            assert!((aspect - (16.0 / 9.0)).abs() < 0.02, "aspect={aspect}");

            let native =
                compute_crisp_video_rect(viewport, 1920, 1080, ppp, VideoScaleMode::Native1to1);
            assert!((native.width() * ppp - 1920.0).abs() <= 1.0);
            assert!((native.height() * ppp - 1080.0).abs() <= 1.0);
        }
    }

    fn run_edit_pass(
        ctx: &egui::Context,
        input: egui::RawInput,
        buf: &mut String,
        palette: &ThemePalette,
        request_focus: bool,
    ) -> (egui::Id, egui::FullOutput) {
        let mut edit_id = egui::Id::NULL;
        let mut out = ctx.run_ui(input, |ui| {
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
                let job = syntax::build_layout_job(
                    text.as_str(),
                    &FontId::monospace(15.0),
                    wrap_width,
                    palette,
                );
                ui.fonts_mut(|f| f.layout_job(job))
            };
            let resp = ui.add(
                egui::TextEdit::multiline(buf)
                    .desired_width(400.0)
                    .layouter(&mut layouter),
            );
            edit_id = resp.id;
            if request_focus {
                resp.request_focus();
            }
        });
        out.textures_delta.clear();
        (edit_id, out)
    }

    #[test]
    fn edit_box_handles_ime_and_unikey_event_sequences() {
        let ctx = egui::Context::default();
        let palette = ThemePalette::dark();
        let mut buf = String::new();

        let (edit_id, _) = run_edit_pass(&ctx, egui::RawInput::default(), &mut buf, &palette, true);
        assert!(ctx.memory(|m| m.has_focus(edit_id)));

        // UniKey/EVKey style sequence: "Tiees" -> 3x Backspace -> "ế" + "ng".
        let unikey_input = egui::RawInput {
            focused: true,
            events: vec![
                egui::Event::Text("T".to_owned()),
                egui::Event::Text("i".to_owned()),
                egui::Event::Text("e".to_owned()),
                egui::Event::Text("e".to_owned()),
                egui::Event::Text("s".to_owned()),
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Text("ế".to_owned()),
                egui::Event::Text("n".to_owned()),
                egui::Event::Text("g".to_owned()),
            ],
            ..Default::default()
        };
        let _ = run_edit_pass(&ctx, unikey_input, &mut buf, &palette, false);
        assert_eq!(buf, "Tiếng");

        // Japanese / TSF IME composition (Preedit -> Commit) + IME cursor area check.
        let preedit_input = egui::RawInput {
            focused: true,
            events: vec![
                egui::Event::Text(" ".to_owned()),
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "にほん".to_owned(),
                    active_range_chars: Some(0..3),
                }),
            ],
            ..Default::default()
        };
        let (_, preedit_out) = run_edit_pass(&ctx, preedit_input, &mut buf, &palette, false);
        assert_eq!(buf, "Tiếng にほん");
        assert!(
            preedit_out.platform_output.ime.is_some(),
            "IME cursor area must be reported for candidate window placement"
        );

        let commit_input = egui::RawInput {
            focused: true,
            events: vec![egui::Event::Ime(egui::ImeEvent::Commit("日本".to_owned()))],
            ..Default::default()
        };
        let _ = run_edit_pass(&ctx, commit_input, &mut buf, &palette, false);
        assert_eq!(buf, "Tiếng 日本");
    }

    #[test]
    fn grid_50k_scroll_renders_smoothly() {
        let ctx = egui::Context::default();
        let mut grid = GridState::generate(50_000);
        let palette = ThemePalette::dark();
        let frames = 120usize;
        let warmup = 5usize;
        let mut frame_ms = Vec::with_capacity(frames);

        for frame_idx in 0..frames {
            let frame_start = Instant::now();
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 600.0))),
                events: if frame_idx > 0 {
                    vec![
                        egui::Event::PointerMoved(Pos2::new(300.0, 300.0)),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: Vec2::new(0.0, -180.0),
                            phase: egui::TouchPhase::Move,
                            source: egui::MouseWheelSource::Unknown,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]
                } else {
                    Vec::new()
                },
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                hlouis_sub_edit::render_grid_table(
                    ui,
                    &mut grid,
                    &palette,
                    ["#", "Start", "End", "CPS", "Style", "Text"],
                );
            });
            out.textures_delta.clear();
            if frame_idx >= warmup {
                frame_ms.push(frame_start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        // Frames 0..warmup build the font atlas and table layout once; they are
        // excluded so the check measures steady-state scrolling. The median is
        // used instead of the mean so one preempted frame on a shared CI runner
        // cannot fail the test; the release-mode Criterion bench gates regressions.
        frame_ms.sort_by(f64::total_cmp);
        let median_ms = frame_ms[frame_ms.len() / 2];
        println!(
            "[bench] 50k virtualized grid median frame time over {} scroll frames: {median_ms:.3} ms",
            frame_ms.len()
        );
        let ceiling_ms = if cfg!(debug_assertions) { 60.0 } else { 8.0 };
        assert!(
            median_ms < ceiling_ms,
            "50k grid median frame time too high: {median_ms:.2} ms (limit {ceiling_ms:.1} ms)"
        );
    }

    fn make_ready_frame(frame_index: usize, pts_ms: i64) -> ReadyFrame {
        ReadyFrame {
            generation: 1,
            frame_index,
            pts_ms,
            width: 2,
            height: 2,
            rgba: vec![0; 16],
            decode_ms: 1.0,
            render_ms: 0.5,
        }
    }

    #[test]
    fn playback_queue_anchors_lazily_and_handles_loop_wraparound() {
        let t0 = Instant::now();
        let interval = 1000.0 / (24_000.0 / 1001.0);
        let mut clock: Option<PlaybackClock> = None;
        let mut queue = VecDeque::from([
            make_ready_frame(1558, 64_981),
            make_ready_frame(0, 0),
            make_ready_frame(1, 42),
        ]);

        // 1. Lazy Play anchor: first frame anchors the clock to 64_981 ms (not stale 0 ms).
        let step1 = pop_due_playback_frame(&mut queue, &mut clock, 100, 1, interval, t0);
        assert_eq!(step1.due_frame.as_ref().map(|f| f.frame_index), Some(1558));
        assert_eq!(step1.dropped_count, 0);
        assert_eq!(step1.popped_count, 1);
        assert!(clock.is_some());

        // 2. Wrap-around when frame 0 is at front: re-anchors clock to 0 ms without consuming frame 1.
        let t1 = t0 + Duration::from_millis(42);
        let step2 = pop_due_playback_frame(&mut queue, &mut clock, 1558, 2, interval, t1);
        assert_eq!(step2.due_frame.as_ref().map(|f| f.frame_index), Some(0));
        assert_eq!(step2.dropped_count, 0);
        assert_eq!(step2.popped_count, 1);
        assert_eq!(queue.len(), 1);

        // 3. Frame 1 is not due immediately at t1, and becomes due 42 ms later with 0 drops.
        let step3_early = pop_due_playback_frame(&mut queue, &mut clock, 0, 3, interval, t1);
        assert!(step3_early.due_frame.is_none());
        let t2 = t1 + Duration::from_millis(42);
        let step3 = pop_due_playback_frame(&mut queue, &mut clock, 0, 3, interval, t2);
        assert_eq!(step3.due_frame.as_ref().map(|f| f.frame_index), Some(1));
        assert_eq!(step3.dropped_count, 0);
        assert!(queue.is_empty());
    }

    #[test]
    fn playback_queue_does_not_swallow_frame_zero_behind_end_of_file() {
        let t0 = Instant::now();
        let interval = 1000.0 / (24_000.0 / 1001.0);
        let mut clock = Some(PlaybackClock {
            start_instant: t0,
            start_pts_ms: 64_940.0,
        });
        let mut queue = VecDeque::from([
            make_ready_frame(1558, 64_981),
            make_ready_frame(0, 0),
            make_ready_frame(1, 42),
        ]);

        // At t0 + 50 ms (target = 64_990 ms), frame 1558 is due; frame 0 must NOT be popped in the same tick.
        let t1 = t0 + Duration::from_millis(50);
        let step1 = pop_due_playback_frame(&mut queue, &mut clock, 1557, 1558, interval, t1);
        assert_eq!(step1.due_frame.as_ref().map(|f| f.frame_index), Some(1558));
        assert_eq!(step1.dropped_count, 0);
        assert_eq!(queue.front().map(|f| f.frame_index), Some(0));

        // Next tick presents frame 0 and re-anchors the clock.
        let t2 = t1 + Duration::from_millis(42);
        let step2 = pop_due_playback_frame(&mut queue, &mut clock, 1558, 1559, interval, t2);
        assert_eq!(step2.due_frame.as_ref().map(|f| f.frame_index), Some(0));
        assert_eq!(step2.dropped_count, 0);
        assert_eq!(queue.front().map(|f| f.frame_index), Some(1));
    }
}
