//! Cooperative cancellation for downloads.
//!
//! Async steps poll [`CancelFlag::cancelled`] so an in-flight HTTP request is
//! dropped instead of waiting out its timeout. Blocking loops call
//! [`CancelFlag::is_cancelled`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

use crate::error::{Error, Result};

/// Shared flag a UI can raise to stop a download.
#[derive(Clone, Debug)]
pub struct CancelFlag {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancelFlag {
    /// Flag that starts clear.
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Ask every waiter to stop.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Whether [`cancel`](Self::cancel) has been called.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Error when the flag is set.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::message("cancelled"))
        } else {
            Ok(())
        }
    }

    /// Resolves when [`cancel`](Self::cancel) is called.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}
