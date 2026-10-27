//! UI state: the latest snapshot, sparkline history and what the keys toggle.

use std::sync::atomic::Ordering;

use crossterm::event::{KeyCode, KeyEvent};

use crate::config::{Config, INTERVALS_MS};
use crate::history::History;
use crate::snapshot::Snapshot;

pub const WINDOWS_MIN: [u8; 3] = [1, 5, 30];

pub struct App {
    pub snapshot: Snapshot,
    pub history: History,
    pub window_min: u8,
    pub frozen: bool,
    pub help: bool,
    pub tab: u8,
}

impl Default for App {
    fn default() -> Self {
        Self { snapshot: Snapshot::default(), history: History::new(), window_min: 1, frozen: false, help: false, tab: 0 }
    }
}

impl App {
    pub fn ingest(&mut self, snap: Snapshot) {
        let v = &snap.vllm;
        let f = |o: Option<f64>| o.map(|x| x as f32);
        let gpu_avg = |get: fn(&crate::snapshot::Gpu) -> Option<f64>| {
            let vals: Vec<f64> = snap.gpu.iter().filter_map(get).collect();
            (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
        };
        let gpu_sum = |get: fn(&crate::snapshot::Gpu) -> Option<f64>| {
            let vals: Vec<f64> = snap.gpu.iter().filter_map(get).collect();
            (!vals.is_empty()).then(|| vals.iter().sum::<f64>())
        };
        self.history.push("gen_tok_s", f(v.generation_tok_s));
        self.history.push("prompt_tok_s", f(v.prompt_tok_s));
        self.history.push("running", v.running.map(|x| x as f32));
        self.history.push("kv", f(v.kv_cache_usage));
        self.history.push("gpu_util", f(gpu_avg(|g| g.utilization)));
        self.history.push("gpu_mem", f(gpu_sum(|g| g.mem_used.map(|b| b as f64))));
        self.history.push("power", f(gpu_sum(|g| g.power_w)));
        self.history.push("gpu_temp", f(gpu_avg(|g| g.temp_c)));
        self.snapshot = snap;
    }

    /// Returns true when the app should quit.
    pub fn on_key(&mut self, key: KeyEvent, cfg: &Config) -> bool {
        let step = |delta: i32| {
            let cur = cfg.interval_ms.load(Ordering::Relaxed);
            let i = INTERVALS_MS.iter().position(|&m| m == cur).unwrap_or(1) as i32 + delta;
            cfg.interval_ms.store(INTERVALS_MS[i.clamp(0, INTERVALS_MS.len() as i32 - 1) as usize], Ordering::Relaxed);
        };
        let window = |cur: u8, delta: i32| {
            let i = WINDOWS_MIN.iter().position(|&w| w == cur).unwrap_or(0) as i32 + delta;
            WINDOWS_MIN[i.clamp(0, WINDOWS_MIN.len() as i32 - 1) as usize]
        };
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc if self.help => self.help = false,
            KeyCode::Esc => return true,
            KeyCode::Char(c @ '1'..='4') => self.tab = c as u8 - b'1',
            KeyCode::Char('+') | KeyCode::Char('=') => step(1),
            KeyCode::Char('-') => step(-1),
            KeyCode::Char(']') => self.window_min = window(self.window_min, 1),
            KeyCode::Char('[') => self.window_min = window(self.window_min, -1),
            KeyCode::Char(' ') => self.frozen = !self.frozen,
            KeyCode::Char('r') => self.history.clear(),
            KeyCode::Char('?') => self.help = !self.help,
            _ => {}
        }
        false
    }
}
