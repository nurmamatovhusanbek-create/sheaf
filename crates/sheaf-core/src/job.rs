//! DATA-JOB: identifiers, priorities and limits shared by MOD-JOBS and its callers.

use crate::consts::{DEFAULT_JOB_TIMEOUT_MS, DEFAULT_WORKER_RSS_MB};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct JobId(pub u64);

/// Scheduling order, highest first: visible page, then selection, then pages
/// arriving from the scanner, then background (MOD-JOBS, FLOW-SCAN-LIVE).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    Background,
    Scan,
    Selection,
    Visible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub timeout_ms: u64,
    pub rss_mb: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_JOB_TIMEOUT_MS,
            rss_mb: DEFAULT_WORKER_RSS_MB,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}
