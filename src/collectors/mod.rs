//! Each collector owns a thread, samples once a second and overwrites its part of the snapshot.

use std::sync::{Arc, RwLock};

use crate::snapshot::{Shared, Snapshot};

#[cfg(target_os = "macos")]
pub mod hw_macos;

pub fn spawn(_vllm_url: &str) -> Shared {
    let state: Shared = Arc::new(RwLock::new(Snapshot::default()));
    // shortcut: no collectors yet, add vllm/system/hw threads here as each lands
    state
}
