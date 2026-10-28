//! Opt-in UI timing for reproducible local integration tests.
use std::{path::PathBuf, time::Duration};
pub struct FrameTimings {
    path: Option<PathBuf>,
    frames: Vec<f64>,
    logic: Vec<f64>,
    events: u64,
}
impl Default for FrameTimings {
    fn default() -> Self {
        Self {
            path: std::env::var_os("INSPECTOR_FRAME_LOG").map(PathBuf::from),
            frames: Vec::new(),
            logic: Vec::new(),
            events: 0,
        }
    }
}
impl FrameTimings {
    pub fn event(&mut self) {
        if self.path.is_some() {
            self.events += 1;
        }
    }
    pub fn record_logic(&mut self, elapsed: Duration) {
        if self.path.is_some() && self.logic.len() < 10000 {
            self.logic.push(elapsed.as_secs_f64() * 1000.0);
        }
    }
    pub fn record(&mut self, elapsed: Duration) {
        if self.path.is_some() && self.frames.len() < 10000 {
            self.frames.push(elapsed.as_secs_f64() * 1000.0);
        }
    }
}
impl Drop for FrameTimings {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            self.frames.sort_by(f64::total_cmp);
            self.logic.sort_by(f64::total_cmp);
            let count = self.frames.len();
            let p95 = self
                .frames
                .get(count.saturating_sub(1) * 95 / 100)
                .copied()
                .unwrap_or(0.0);
            let report = serde_json::json!({
                "frames": count,
                "ui_p95_ms": p95,
                "ui_max_ms": self.frames.last().copied().unwrap_or(0.0),
                "logic_calls": self.logic.len(),
                "logic_p95_ms": self.logic.get(self.logic.len().saturating_sub(1) * 95 / 100).copied().unwrap_or(0.0),
                "logic_max_ms": self.logic.last().copied().unwrap_or(0.0),
                "events_processed": self.events,
                "note": "UI and background event-processing callbacks are measured separately; excludes GPU presentation and time outside the callbacks. Logic also runs for hidden windows.",
            });
            let _ = std::fs::write(path, report.to_string());
        }
    }
}
