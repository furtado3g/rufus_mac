use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::util::format_bytes;

/// Stage of the write pipeline reported to the UI/CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Analyzing,
    Planning,
    Unmounting,
    Opening,
    Formatting,
    Copying,
    Finalizing,
    Writing,
    Syncing,
    Invalidating,
    Verifying,
    Ejecting,
    Done,
    Failed,
    Cancelled,
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Phase::Analyzing => "analyzing",
            Phase::Planning => "planning",
            Phase::Unmounting => "unmounting",
            Phase::Opening => "opening",
            Phase::Formatting => "formatting",
            Phase::Copying => "copying",
            Phase::Finalizing => "finalizing",
            Phase::Writing => "writing",
            Phase::Syncing => "syncing",
            Phase::Invalidating => "invalidating",
            Phase::Verifying => "verifying",
            Phase::Ejecting => "ejecting",
            Phase::Done => "done",
            Phase::Failed => "failed",
            Phase::Cancelled => "cancelled",
        };
        f.write_str(name)
    }
}

/// A single progress sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressEvent {
    pub phase: Phase,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub bytes_per_sec: Option<u64>,
    pub eta: Option<Duration>,
    pub message: Option<String>,
}

impl ProgressEvent {
    pub fn phase(phase: Phase) -> Self {
        Self {
            phase,
            done_bytes: 0,
            total_bytes: 0,
            bytes_per_sec: None,
            eta: None,
            message: None,
        }
    }

    pub fn message(phase: Phase, message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::phase(phase)
        }
    }

    pub fn fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.done_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0) as f32
        }
    }

    pub fn summary(&self) -> String {
        let mut out = format!("[{}]", self.phase);
        if self.total_bytes > 0 {
            out.push_str(&format!(
                " {} / {} ({:.0}%)",
                format_bytes(self.done_bytes),
                format_bytes(self.total_bytes),
                self.fraction() * 100.0
            ));
        }
        if let Some(bps) = self.bytes_per_sec
            && bps > 0
        {
            out.push_str(&format!("  {}/s", format_bytes(bps)));
        }
        if let Some(eta) = self.eta {
            out.push_str(&format!("  ETA {}", format_duration(eta)));
        }
        if let Some(msg) = &self.message {
            out.push_str("  ");
            out.push_str(msg);
        }
        out
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        format!(
            "{:02}:{:02}:{:02}",
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60
        )
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

/// Receives progress samples. Implementations must be cheap and non-blocking.
pub trait ProgressSink: Send + Sync {
    fn report(&self, event: ProgressEvent);
}

/// Discards every event.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSink;

impl ProgressSink for NoopSink {
    fn report(&self, _event: ProgressEvent) {}
}

/// Adapts a closure into a [`ProgressSink`].
pub struct CallbackSink<F>(pub F);

impl<F> ProgressSink for CallbackSink<F>
where
    F: Fn(ProgressEvent) + Send + Sync,
{
    fn report(&self, event: ProgressEvent) {
        (self.0)(event);
    }
}

/// Forwards events to a crossbeam channel, for GUI worker threads.
#[derive(Debug, Clone)]
pub struct ChannelSink(pub crossbeam_channel::Sender<ProgressEvent>);

impl ProgressSink for ChannelSink {
    fn report(&self, event: ProgressEvent) {
        // A disconnected receiver simply means nobody is listening anymore.
        let _ = self.0.try_send(event);
    }
}
