//! Progress reporting from the installer thread to whoever is watching (the UI or the CLI).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail};

#[derive(Debug, Clone)]
pub enum Event {
    /// Started step `index` (0-based) of `total`.
    Step {
        index: usize,
        total: usize,
        title: String,
    },
    /// Progress within the current step: bytes done out of `total`.
    Bytes {
        done: u64,
        total: Option<u64>,
    },
    Log(String),
}

#[derive(Clone)]
pub struct Progress {
    sink: Arc<dyn Fn(Event) + Send + Sync>,
    cancel: Arc<AtomicBool>,
}

impl Progress {
    pub fn new(sink: impl Fn(Event) + Send + Sync + 'static) -> Self {
        Self { sink: Arc::new(sink), cancel: Arc::new(AtomicBool::new(false)) }
    }

    /// A reporter that drops every event.
    pub fn silent() -> Self {
        Self::new(|_| {})
    }

    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub fn step(&self, index: usize, total: usize, title: impl Into<String>) {
        let title = title.into();
        log::info!("[{}/{total}] {title}", index + 1);
        (self.sink)(Event::Step { index, total, title });
    }

    pub fn bytes(&self, done: u64, total: Option<u64>) {
        (self.sink)(Event::Bytes { done, total });
    }

    pub fn log(&self, msg: impl Into<String>) {
        let msg = msg.into();
        log::info!("{msg}");
        (self.sink)(Event::Log(msg));
    }

    pub fn check_cancelled(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        Ok(())
    }
}
