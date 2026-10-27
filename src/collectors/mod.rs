//! Each collector owns a thread, samples once a second and overwrites its part of the snapshot.

use std::sync::{Arc, RwLock};

use crate::config::SharedConfig;
use crate::snapshot::{Shared, Snapshot};

pub mod system;
pub mod vllm;

#[cfg(target_os = "macos")]
pub mod hw_macos;
#[cfg(target_os = "linux")]
pub mod hw_linux;

pub fn spawn(cfg: SharedConfig) -> Shared {
    let state: Shared = Arc::new(RwLock::new(Snapshot::default()));
    vllm::spawn(state.clone(), cfg.clone());
    system::spawn(state.clone(), cfg.clone());
    #[cfg(target_os = "macos")]
    hw_macos::spawn(state.clone(), cfg.clone());
    #[cfg(target_os = "linux")]
    hw_linux::spawn(state.clone(), cfg.clone());
    state
}
