//! Structured `tracing` ring-buffer logger + egui Event Log window with search, per-module
//! filtering, copy, save-to-file, and privacy-respecting problem report bundles.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use egui::{ScrollArea, Ui};
use i18n_catalog::Catalog;
use settings::AppSettings;
use std::collections::VecDeque;
use std::fmt::{self, Write as _};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

/// Numeric log severity matching `tracing::Level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

impl LogLevel {
    /// Converts from `tracing::Level`.
    #[must_use]
    pub const fn from_tracing(level: &Level) -> Self {
        match *level {
            Level::TRACE => Self::Trace,
            Level::DEBUG => Self::Debug,
            Level::INFO => Self::Info,
            Level::WARN => Self::Warn,
            Level::ERROR => Self::Error,
        }
    }

    /// Short uppercase tag for display and file export.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

/// One structured log entry stored in the bounded ring buffer.
#[derive(Debug, Clone)]
pub struct LogRecord {
    pub timestamp_ms: u64,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
}

/// Thread-safe bounded ring buffer backing both [`TracingRingLayer`] and [`show_log_window`].
#[derive(Debug, Clone)]
pub struct RingLogBuffer {
    inner: Arc<Mutex<RingInner>>,
}

#[derive(Debug)]
struct RingInner {
    capacity: usize,
    entries: VecDeque<LogRecord>,
}

impl RingLogBuffer {
    /// Creates a ring buffer retaining at most `capacity` recent entries.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            inner: Arc::new(Mutex::new(RingInner {
                capacity: cap,
                entries: VecDeque::with_capacity(cap),
            })),
        }
    }

    /// Appends one entry, evicting the oldest entry if full.
    pub fn push(&self, record: LogRecord) {
        if let Ok(mut guard) = self.inner.lock() {
            if guard.entries.len() == guard.capacity {
                guard.entries.pop_front();
            }
            guard.entries.push_back(record);
        }
    }

    /// Appends a pre-allocated entry without growing the ring once at capacity.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().map_or(0, |g| g.entries.len())
    }

    /// Returns `true` when the ring buffer holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copies filtered entries matching `min_level`, `module_filter`, and `search`.
    #[must_use]
    pub fn snapshot_filtered(
        &self,
        min_level: LogLevel,
        module_filter: &str,
        search: &str,
    ) -> Vec<LogRecord> {
        let m_low = module_filter.to_ascii_lowercase();
        let s_low = search.to_ascii_lowercase();
        let Ok(guard) = self.inner.lock() else {
            return Vec::new();
        };
        guard
            .entries
            .iter()
            .filter(|r| {
                r.level >= min_level
                    && (m_low.is_empty() || r.target.to_ascii_lowercase().contains(&m_low))
                    && (s_low.is_empty()
                        || r.message.to_ascii_lowercase().contains(&s_low)
                        || r.target.to_ascii_lowercase().contains(&s_low))
            })
            .cloned()
            .collect()
    }

    /// Formats all matching entries as plain text lines.
    #[must_use]
    pub fn format_text(&self, min_level: LogLevel, module_filter: &str, search: &str) -> String {
        let rows = self.snapshot_filtered(min_level, module_filter, search);
        let mut out = String::with_capacity(rows.len() * 64);
        for r in rows {
            let _ = writeln!(
                out,
                "[{}] {:5} [{}] {}",
                r.timestamp_ms,
                r.level.tag(),
                r.target,
                r.message
            );
        }
        out
    }

    /// Clears all buffered entries.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.entries.clear();
        }
    }
}

/// `tracing-subscriber` layer capturing structured events directly into [`RingLogBuffer`].
pub struct TracingRingLayer {
    buffer: RingLogBuffer,
}

impl TracingRingLayer {
    /// Creates a layer writing to `buffer`.
    #[must_use]
    pub const fn new(buffer: RingLogBuffer) -> Self {
        Self { buffer }
    }
}

struct MessageVisitor {
    out: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if !self.out.is_empty() {
            self.out.push(' ');
        }
        if field.name() == "message" {
            let _ = write!(self.out, "{value:?}");
        } else {
            let _ = write!(self.out, "{}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if !self.out.is_empty() {
            self.out.push(' ');
        }
        if field.name() == "message" {
            self.out.push_str(value);
        } else {
            let _ = write!(self.out, "{}={value}", field.name());
        }
    }
}

impl<S: Subscriber> Layer<S> for TracingRingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut visitor = MessageVisitor { out: String::new() };
        event.record(&mut visitor);
        #[allow(clippy::cast_possible_truncation)]
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        self.buffer.push(LogRecord {
            timestamp_ms,
            level: LogLevel::from_tracing(meta.level()),
            target: meta.target().to_owned(),
            message: visitor.out,
        });
    }
}

/// Redacts common Windows (`C:\Users\<name>\...`) and Unix (`/home/<name>/...`, `/Users/<name>/...`)
/// personal directory paths when `include_personal_paths` is `false`.
#[must_use]
pub fn redact_personal_paths(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        let mut cur = line.to_owned();
        for prefix in ["C:\\Users\\", "c:\\Users\\", "/home/", "/Users/"] {
            while let Some(pos) = cur.find(prefix) {
                let after = &cur[pos + prefix.len()..];
                let end_rel = after
                    .find(['\\', '/', ' ', '"', '\'', ',', ';'])
                    .unwrap_or(after.len());
                let mut next = String::with_capacity(cur.len());
                next.push_str(&cur[..pos]);
                next.push_str("<REDACTED_USER>");
                next.push_str(&after[end_rel..]);
                cur = next;
            }
        }
        out.push_str(&cur);
        out.push('\n');
    }
    out
}

/// Builds a "Report a Problem" diagnostics bundle containing app version, OS/arch, sanitized
/// settings JSON, and recent log entries. Personal paths are redacted unless `include_personal_paths`
/// is explicitly `true`.
#[must_use]
pub fn build_problem_report_bundle(
    app_version: &str,
    settings: &AppSettings,
    buffer: &RingLogBuffer,
    include_personal_paths: bool,
) -> String {
    let settings_json = settings.save_json().unwrap_or_else(|_| "{}".to_owned());
    let logs = buffer.format_text(LogLevel::Trace, "", "");
    let raw = format!(
        "=== HLouis Sub Edit Problem Report ===\n\
         Version: {app_version}\n\
         OS: {} ({})\n\
         Personal paths included: {include_personal_paths}\n\n\
         --- Settings ---\n{settings_json}\n\n\
         --- Event Log ---\n{logs}",
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    if include_personal_paths {
        raw
    } else {
        redact_personal_paths(&raw)
    }
}

/// Writes `content` to `path`.
///
/// # Errors
/// Returns I/O error if writing fails.
pub fn save_text_file(path: &Path, content: &str) -> std::io::Result<()> {
    fs::write(path, content)
}

/// Persistent UI state for the egui Log window.
#[derive(Debug, Clone)]
pub struct LogWindowState {
    pub min_level: LogLevel,
    pub module_filter: String,
    pub search: String,
    pub include_personal_paths: bool,
    pub last_copied: Option<String>,
    pub last_bundle: Option<String>,
}

impl Default for LogWindowState {
    fn default() -> Self {
        Self {
            min_level: LogLevel::Info,
            module_filter: String::new(),
            search: String::new(),
            include_personal_paths: false,
            last_copied: None,
            last_bundle: None,
        }
    }
}

/// Renders the egui Event Log window with search, level/module filters, Copy, and Problem Report.
pub fn show_log_window(
    ui: &mut Ui,
    state: &mut LogWindowState,
    buffer: &RingLogBuffer,
    settings: &AppSettings,
    app_version: &str,
    catalog: &Catalog,
) {
    ui.horizontal(|ui| {
        ui.label("Level:");
        for lvl in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ] {
            if ui
                .selectable_label(state.min_level == lvl, lvl.tag())
                .clicked()
            {
                state.min_level = lvl;
            }
        }
        ui.separator();
        ui.label("Module:");
        ui.text_edit_singleline(&mut state.module_filter);
        ui.label("Search:");
        ui.text_edit_singleline(&mut state.search);
    });

    ui.horizontal(|ui| {
        if ui
            .button(catalog.get("log-copy").unwrap_or("Copy"))
            .clicked()
        {
            let txt = buffer.format_text(state.min_level, &state.module_filter, &state.search);
            ui.ctx().copy_text(txt.clone());
            state.last_copied = Some(txt);
        }
        if ui
            .button(
                catalog
                    .get("log-report-bundle")
                    .unwrap_or("Export Problem Report..."),
            )
            .clicked()
        {
            state.last_bundle = Some(build_problem_report_bundle(
                app_version,
                settings,
                buffer,
                state.include_personal_paths,
            ));
        }
        ui.checkbox(
            &mut state.include_personal_paths,
            "Include personal file paths in report",
        );
    });

    ui.separator();
    let rows = buffer.snapshot_filtered(state.min_level, &state.module_filter, &state.search);
    ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
        for r in &rows {
            ui.monospace(format!("[{}] [{}] {}", r.level.tag(), r.target, r.message));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use i18n_catalog::Locale;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn tracing_layer_captures_events_redacts_paths_and_renders_window() {
        let buf = RingLogBuffer::new(4);
        let subscriber = tracing_subscriber::registry().with(TracingRingLayer::new(buf.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "ass_core", "Loaded C:\\Users\\Alice\\subs\\ep01.ass");
            tracing::warn!(target: "mkv_subs", "Track 12 is PGS bitmap");
            tracing::error!(target: "audio", "Decode hiccup");
        });

        assert_eq!(buf.len(), 3);
        let mkv_only = buf.snapshot_filtered(LogLevel::Info, "mkv_subs", "");
        assert_eq!(mkv_only.len(), 1);

        let settings = AppSettings::default();
        let redacted = build_problem_report_bundle("0.1.0", &settings, &buf, false);
        assert!(!redacted.contains("Alice"));
        assert!(redacted.contains("<REDACTED_USER>"));

        let unredacted = build_problem_report_bundle("0.1.0", &settings, &buf, true);
        assert!(unredacted.contains("Alice"));

        // Test save_text_file
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("report.txt");
        save_text_file(&p, &redacted).expect("save");
        assert!(
            fs::read_to_string(&p)
                .expect("read")
                .contains("HLouis Sub Edit")
        );

        // Render egui Log window
        let mut state = LogWindowState::default();
        let cat = Catalog::for_locale(Locale::En);
        let builder = Harness::builder().with_accessibility_check(false);
        let mut harness = builder.build_ui(|ui| {
            show_log_window(ui, &mut state, &buf, &settings, "0.1.0", &cat);
        });
        harness.run();
    }
}
