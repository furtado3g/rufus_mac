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
    Preparing,
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

impl Phase {
    /// Phases that carry a meaningful byte counter for the main progress bar.
    pub const fn is_data(self) -> bool {
        matches!(self, Phase::Writing | Phase::Copying | Phase::Verifying)
    }

    /// Terminal states. `Ejecting` is deliberately not terminal: ejection
    /// warnings are reported separately from the write result.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Phase::Done | Phase::Failed | Phase::Cancelled)
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Phase::Analyzing => "analyzing",
            Phase::Planning => "planning",
            Phase::Preparing => "preparing",
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
    /// Set for `Failed`/`Cancelled` only after the first target write has
    /// happened. In that state the target may contain a partial image and must
    /// not be treated as untouched.
    #[serde(default)]
    pub target_incomplete: bool,
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
            target_incomplete: false,
        }
    }

    pub fn message(phase: Phase, message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::phase(phase)
        }
    }

    /// Build a failure event before any target write occurred.
    pub fn failed_before_write(message: impl Into<String>) -> Self {
        Self::terminal(Phase::Failed, message, false)
    }

    /// Build a failure event after at least one target write occurred.
    pub fn failed_after_write(message: impl Into<String>) -> Self {
        Self::terminal(Phase::Failed, message, true)
    }

    /// Build a cancellation event before any target write occurred.
    pub fn cancelled_before_write() -> Self {
        Self::terminal(Phase::Cancelled, "operation cancelled", false)
    }

    /// Build a cancellation event after at least one target write occurred.
    pub fn cancelled_after_write() -> Self {
        Self::terminal(Phase::Cancelled, "operation cancelled", true)
    }

    fn terminal(phase: Phase, message: impl Into<String>, target_incomplete: bool) -> Self {
        debug_assert!(phase.is_terminal());
        let mut event = Self::message(phase, message);
        event.target_incomplete = target_incomplete;
        event
    }

    /// Build the successful terminal event.
    ///
    /// Callers must emit this only after the target has been flushed with
    /// `RawIo::sync`. Ejection problems are separate `Ejecting` events and do
    /// not turn a flushed, complete write into a failure.
    pub fn done_after_flush() -> Self {
        Self::message(Phase::Done, "write complete")
    }

    /// Build a non-terminal ejection warning.
    pub fn ejecting(message: impl Into<String>) -> Self {
        Self::message(Phase::Ejecting, message)
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
        if self.target_incomplete {
            out.push_str("  [target incomplete]");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparing_phase_serializes_and_is_not_a_data_phase() {
        assert_eq!(Phase::Preparing.to_string(), "preparing");
        assert_eq!(
            serde_json::to_string(&Phase::Preparing).unwrap(),
            "\"preparing\""
        );
        assert_eq!(
            serde_json::from_str::<Phase>("\"preparing\"").unwrap(),
            Phase::Preparing
        );
        assert!(!Phase::Preparing.is_data());
        assert!(!Phase::Preparing.is_terminal());
        assert!(Phase::Writing.is_data());
        assert!(Phase::Copying.is_data());
        assert!(Phase::Verifying.is_data());
    }

    #[test]
    fn failure_and_cancellation_after_a_write_mark_the_target_incomplete() {
        let failed = ProgressEvent::failed_after_write("copy failed");
        assert_eq!(failed.phase, Phase::Failed);
        assert!(failed.target_incomplete);
        assert!(failed.phase.is_terminal());
        assert!(failed.summary().contains("[target incomplete]"));

        let cancelled = ProgressEvent::cancelled_after_write();
        assert_eq!(cancelled.phase, Phase::Cancelled);
        assert!(cancelled.target_incomplete);
        assert!(cancelled.phase.is_terminal());

        let failed_before_write = ProgressEvent::failed_before_write("planning failed");
        assert!(!failed_before_write.target_incomplete);
        let cancelled_before_write = ProgressEvent::cancelled_before_write();
        assert!(!cancelled_before_write.target_incomplete);
    }

    #[test]
    fn done_is_complete_and_ejection_is_a_separate_warning() {
        let done = ProgressEvent::done_after_flush();
        assert_eq!(done.phase, Phase::Done);
        assert!(!done.target_incomplete);
        assert!(done.phase.is_terminal());
        assert!(!done.summary().contains("incomplete"));

        let ejection = ProgressEvent::ejecting("could not eject; remove manually");
        assert_eq!(ejection.phase, Phase::Ejecting);
        assert!(!ejection.target_incomplete);
        assert!(!ejection.phase.is_terminal());
        assert!(ejection.summary().contains("remove manually"));
    }

    #[test]
    fn missing_target_incomplete_field_defaults_to_false() {
        let event: ProgressEvent = serde_json::from_str(
            r#"{
                "phase":"failed",
                "done_bytes":0,
                "total_bytes":0,
                "bytes_per_sec":null,
                "eta":null,
                "message":"failed"
            }"#,
        )
        .unwrap();
        assert!(!event.target_incomplete);

        let event: ProgressEvent = serde_json::from_str(
            r#"{
                "phase":"failed",
                "done_bytes":4096,
                "total_bytes":8192,
                "bytes_per_sec":null,
                "eta":null,
                "message":"failed",
                "target_incomplete":true
            }"#,
        )
        .unwrap();
        assert!(event.target_incomplete);
        assert_eq!(
            serde_json::from_str::<ProgressEvent>(&serde_json::to_string(&event).unwrap()).unwrap(),
            event
        );
    }
}
