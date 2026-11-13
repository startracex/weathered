//! Progress events for downloads.
//!
//! The library never prints. A CLI or GUI supplies a [`ProgressSink`].

use std::sync::Arc;

/// One progress notification.
#[derive(Debug, Clone)]
pub struct ProgressEvent {
    /// Short stage name such as `assets` or `libraries`.
    pub stage: String,
    /// Units finished in this stage.
    pub completed: u64,
    /// Units expected in this stage.
    pub total: u64,
    /// File path or other detail. May be empty.
    pub detail: String,
}

/// Callback invoked as files are downloaded.
#[derive(Clone)]
pub struct ProgressSink {
    callback: Arc<dyn Fn(ProgressEvent) + Send + Sync>,
}

impl ProgressSink {
    /// Build a sink from a callback. The callback may be invoked from worker tasks.
    pub fn new(callback: impl Fn(ProgressEvent) + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }

    /// Sink that discards every event.
    pub fn noop() -> Self {
        Self::new(|_| {})
    }

    /// Deliver one event.
    pub fn emit(&self, event: ProgressEvent) {
        (self.callback)(event);
    }
}
