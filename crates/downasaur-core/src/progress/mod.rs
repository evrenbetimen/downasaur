//! Progress fan-out with 60 fps coalescing.
//!
//! Workers call [`ProgressHub::publish`] as often as they like; only the latest
//! update per `(task, channel)` is kept. A flusher task drains dirty entries
//! every [`FRAME`] and hands one batch to the sink (the Tauri window emitter),
//! so 100+ concurrent downloads cost at most 60 IPC messages per second.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::model::TaskState;

/// One frame at 60 fps.
pub const FRAME: Duration = Duration::from_micros(16_667);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
pub enum ProgressEvent {
    Download { task_id: Uuid, bytes_done: u64, bytes_total: Option<u64>, speed_bps: f64, eta_secs: Option<f64> },
    Muxing { task_id: Uuid, percent: f32, label: String },
    State { task_id: Uuid, state: TaskState, message: Option<String> },
    Organized { task_id: Uuid, destination: String },
}

impl ProgressEvent {
    fn key(&self) -> (Uuid, u8) {
        match self {
            Self::Download { task_id, .. } => (*task_id, 0),
            Self::Muxing { task_id, .. } => (*task_id, 1),
            Self::State { task_id, .. } => (*task_id, 2),
            Self::Organized { task_id, .. } => (*task_id, 3),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProgressHub {
    pending: Arc<Mutex<HashMap<(Uuid, u8), ProgressEvent>>>,
}

impl ProgressHub {
    pub fn publish(&self, event: ProgressEvent) {
        self.pending.lock().insert(event.key(), event);
    }

    /// Take everything published since the last drain.
    pub fn drain(&self) -> Vec<ProgressEvent> {
        let mut map = self.pending.lock();
        map.drain().map(|(_, v)| v).collect()
    }

    /// Spawn the frame-paced flusher. `sink` is only called with non-empty batches.
    pub fn spawn_flusher<S>(&self, sink: S, cancel: CancellationToken) -> tokio::task::JoinHandle<()>
    where
        S: Fn(Vec<ProgressEvent>) + Send + 'static,
    {
        let hub = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(FRAME);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    _ = tick.tick() => {
                        let batch = hub.drain();
                        if !batch.is_empty() {
                            sink(batch);
                        }
                    }
                }
            }
        })
    }
}

/// Exponentially smoothed throughput estimate.
#[derive(Debug, Clone)]
pub struct SpeedMeter {
    last_bytes: u64,
    last_at: Instant,
    bps: f64,
}

impl SpeedMeter {
    const ALPHA: f64 = 0.3;

    pub fn new(start_bytes: u64) -> Self {
        Self { last_bytes: start_bytes, last_at: Instant::now(), bps: 0.0 }
    }

    pub fn sample(&mut self, bytes: u64) -> f64 {
        let now = Instant::now();
        let dt = now.duration_since(self.last_at).as_secs_f64();
        if dt > 0.05 {
            let inst = bytes.saturating_sub(self.last_bytes) as f64 / dt;
            self.bps = if self.bps == 0.0 { inst } else { Self::ALPHA * inst + (1.0 - Self::ALPHA) * self.bps };
            self.last_bytes = bytes;
            self.last_at = now;
        }
        self.bps
    }

    pub fn eta(&self, done: u64, total: Option<u64>) -> Option<f64> {
        let remaining = total?.saturating_sub(done) as f64;
        (self.bps > 1.0).then(|| remaining / self.bps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_to_latest_per_task() {
        let hub = ProgressHub::default();
        let id = Uuid::new_v4();
        for b in [10, 20, 30] {
            hub.publish(ProgressEvent::Download {
                task_id: id,
                bytes_done: b,
                bytes_total: None,
                speed_bps: 0.0,
                eta_secs: None,
            });
        }
        hub.publish(ProgressEvent::State { task_id: id, state: TaskState::Downloading, message: None });
        let batch = hub.drain();
        assert_eq!(batch.len(), 2);
        assert!(batch.iter().any(|e| matches!(e, ProgressEvent::Download { bytes_done: 30, .. })));
        assert!(hub.drain().is_empty());
    }

    #[test]
    fn events_serialize_camel_case() {
        let id = Uuid::nil();
        let v = serde_json::to_value(ProgressEvent::Muxing {
            task_id: id,
            percent: 50.0,
            label: "Muxing 8K Video...".into(),
        })
        .expect("json");
        assert_eq!(v["kind"], "muxing");
        assert_eq!(v["taskId"], id.to_string());
    }
}
