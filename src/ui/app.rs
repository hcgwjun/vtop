//! UI state: the latest snapshot, sparkline history and what the keys toggle.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{Config, INTERVALS_MS};
use crate::history::History;
use crate::requests::{Record, SharedLog};
use crate::snapshot::Snapshot;

pub const WINDOWS_MIN: [u8; 3] = [1, 5, 30];

pub struct App {
    pub snapshot: Snapshot,
    pub history: History,
    pub window_min: u8,
    pub frozen: bool,
    pub help: bool,
    pub tab: u8,
    /// Newest first; refilled from the span log every tick.
    pub requests: Vec<Record>,
    pub log: SharedLog,
    /// Index into `visible_requests()`.
    pub selected: usize,
    pub filter: String,
    pub filter_editing: bool,
    /// Shown in the header for a few seconds (export result).
    pub notice: Option<(String, Instant)>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            snapshot: Snapshot::default(),
            history: History::new(),
            window_min: 1,
            frozen: false,
            help: false,
            tab: 0,
            requests: Vec::new(),
            log: SharedLog::default(),
            selected: 0,
            filter: String::new(),
            filter_editing: false,
            notice: None,
        }
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
        self.history.push("disk_r", f(snap.system.disk_read_bps));
        self.history.push("net_rx", f(snap.system.net_rx_bps));
        self.history.push("kv", f(v.kv_cache_usage));
        self.history.push("prefix_hit", f(v.prefix_cache_hit_rate));
        self.history.push("gpu_util", f(gpu_avg(|g| g.utilization)));
        self.history.push("gpu_mem", f(gpu_sum(|g| g.mem_used.map(|b| b as f64))));
        self.history.push("power", f(gpu_sum(|g| g.power_w)));
        self.history.push("gpu_temp", f(gpu_avg(|g| g.temp_c)));
        self.snapshot = snap;
    }

    pub fn ingest_log(&mut self) {
        // the cursor follows its row by id as newer rows are inserted above; row 0 keeps tailing the newest
        let followed = (self.selected > 0).then(|| self.visible_requests().get(self.selected).map(|r| r.id)).flatten();
        self.requests = self.log.lock().unwrap().latest(crate::requests::CAPACITY);
        if let Some(id) = followed {
            let visible = self.visible_requests();
            self.selected = visible.iter().position(|r| r.id == id).unwrap_or(visible.len().saturating_sub(1));
        }
    }

    /// Every whitespace token must match: `>N` means output_tokens >= N, anything else is a
    /// case-insensitive substring of the request id.
    pub fn visible_requests(&self) -> Vec<&Record> {
        let tokens: Vec<String> = self.filter.split_whitespace().map(str::to_lowercase).collect();
        self.requests
            .iter()
            .filter(|r| {
                tokens.iter().all(|t| match t.strip_prefix('>').and_then(|n| n.parse::<u64>().ok()) {
                    Some(n) => r.output_tokens.is_some_and(|o| o >= n),
                    None => r.request_id.to_lowercase().contains(t),
                })
            })
            .collect()
    }

    /// Writes `vtop-<YYYYmmdd-HHMMSS>.json` (local time) with the current snapshot and every recorded request.
    pub fn export_to(&self, dir: &Path) -> anyhow::Result<PathBuf> {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let path = dir.join(format!("vtop-{}.json", crate::requests::stamp_local(ms)));
        let doc = serde_json::json!({ "snapshot": self.snapshot, "requests": self.requests });
        std::fs::write(&path, serde_json::to_vec_pretty(&doc)?)?;
        Ok(path)
    }

    fn export(&mut self) {
        let msg = match self.export_to(Path::new(".")) {
            Ok(p) => format!("saved {}", p.display()),
            Err(e) => format!("export failed: {e}"),
        };
        self.notice = Some((msg, Instant::now()));
    }

    fn clear_requests(&mut self) {
        self.log.lock().unwrap().clear();
        self.requests.clear();
        self.selected = 0;
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
        if self.filter_editing {
            // every key is text here, including q: only Enter/Esc leave the mode
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filter_editing = false;
                }
                KeyCode::Enter => self.filter_editing = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => self.filter.push(c),
                KeyCode::Char('c') => return true,
                _ => {}
            }
            self.selected = self.selected.min(self.visible_requests().len().saturating_sub(1));
            return false;
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Esc if self.help => self.help = false,
            KeyCode::Esc if !self.filter.is_empty() => self.filter.clear(),
            KeyCode::Esc => return true,
            KeyCode::Char(c @ '1'..='4') => self.tab = c as u8 - b'1',
            KeyCode::Char('+') | KeyCode::Char('=') => step(1),
            KeyCode::Char('-') => step(-1),
            KeyCode::Char(']') => self.window_min = window(self.window_min, 1),
            KeyCode::Char('[') => self.window_min = window(self.window_min, -1),
            KeyCode::Char(' ') => self.frozen = !self.frozen,
            KeyCode::Char('r') => self.history.clear(),
            KeyCode::Char('?') => self.help = !self.help,
            KeyCode::Char('j') if self.tab == 2 => self.selected += 1,
            KeyCode::Char('k') if self.tab == 2 => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('c') if self.tab == 2 => self.clear_requests(),
            KeyCode::Char('/') if self.tab == 2 => self.filter_editing = true,
            KeyCode::Char('e') => self.export(),
            _ => {}
        }
        self.selected = self.selected.min(self.visible_requests().len().saturating_sub(1));
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::sample;

    fn app3() -> App {
        let mut app = App::default();
        app.requests = vec![sample("chatcmpl-aaa", 10), sample("chatcmpl-bbb", 500), sample("cmpl-ccc", 10)];
        app
    }

    #[test]
    fn filter_tokens_all_match() {
        let mut app = app3();
        assert_eq!(app.visible_requests().len(), 3);
        app.filter = "aaa >100".into();
        assert_eq!(app.visible_requests().len(), 0);
        app.filter = ">100".into();
        assert_eq!(app.visible_requests().len(), 1);
        app.filter = "CMPL-C".into();
        let v = app.visible_requests();
        assert!(v.len() == 1 && v[0].request_id == "cmpl-ccc");
    }

    #[test]
    fn j_k_clamp_selection() {
        let cfg = Config::new("http://x".into(), false);
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        let mut app = app3();
        app.tab = 2;
        app.on_key(key('k'), &cfg);
        assert_eq!(app.selected, 0);
        for _ in 0..5 {
            app.on_key(key('j'), &cfg);
        }
        assert_eq!(app.selected, 2);
        app.filter = ">100".into();
        app.on_key(key('j'), &cfg);
        assert_eq!(app.selected, 0, "clamped to the filtered list");
    }

    #[test]
    fn typing_in_filter_does_not_quit() {
        let cfg = Config::new("http://x".into(), false);
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let mut app = app3();
        app.tab = 2;
        app.on_key(key(KeyCode::Char('/')), &cfg);
        assert!(!app.on_key(key(KeyCode::Char('q')), &cfg));
        assert!(!app.on_key(key(KeyCode::Char('c')), &cfg), "ctrl-less c types a char");
        assert_eq!(app.filter, "qc");
        assert_eq!(app.requests.len(), 3, "c must not clear while typing");
        app.on_key(key(KeyCode::Backspace), &cfg);
        assert_eq!(app.filter, "q");
        app.on_key(key(KeyCode::Enter), &cfg);
        assert!(!app.filter_editing && app.filter == "q");
        app.on_key(key(KeyCode::Char('/')), &cfg);
        assert!(!app.on_key(key(KeyCode::Esc), &cfg));
        assert!(!app.filter_editing && app.filter.is_empty());
    }

    #[test]
    fn esc_clears_an_applied_filter_instead_of_quitting() {
        let cfg = Config::new("http://x".into(), false);
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let mut app = app3();
        app.tab = 2;
        app.filter = "b".into();
        assert!(!app.on_key(key(KeyCode::Esc), &cfg));
        assert!(app.filter.is_empty());
        assert!(app.on_key(key(KeyCode::Esc), &cfg), "second Esc quits");
    }

    #[test]
    fn c_clears_the_shared_log() {
        let cfg = Config::new("http://x".into(), false);
        let mut app = App::default();
        app.log.lock().unwrap().push(sample("a", 1));
        app.ingest_log();
        app.tab = 2;
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE), &cfg);
        assert!(app.requests.is_empty() && app.log.lock().unwrap().latest(1).is_empty());
    }

    #[test]
    fn selection_follows_its_row_under_a_filter_and_at_capacity() {
        let mut app = App::default();
        for i in 0..3 {
            app.log.lock().unwrap().push(sample(&format!("r{i}"), 500));
        }
        app.ingest_log();
        app.filter = ">100".into();
        app.selected = 2; // r0, the oldest visible row
        app.log.lock().unwrap().push(sample("short", 1)); // filtered out: must not shift the cursor
        app.ingest_log();
        assert_eq!(app.visible_requests()[app.selected].request_id, "r0");
        for _ in 0..crate::requests::CAPACITY {
            app.log.lock().unwrap().push(sample("fill", 500));
        }
        app.ingest_log();
        assert!(app.selected < app.visible_requests().len(), "r0 rotated out: cursor clamped, not lost");
    }

    #[test]
    fn selection_follows_its_row_when_new_requests_arrive() {
        let mut app = App::default();
        for _ in 0..2 {
            app.log.lock().unwrap().push(sample("a", 1));
        }
        app.ingest_log();
        app.selected = 1; // the oldest row, id 1
        app.log.lock().unwrap().push(sample("a", 1));
        app.ingest_log();
        assert_eq!(app.visible_requests()[app.selected].id, 1);
        app.selected = 0;
        app.log.lock().unwrap().push(sample("a", 1));
        app.ingest_log();
        assert_eq!(app.selected, 0, "the top row keeps tailing the newest request");
    }

    #[test]
    fn export_writes_json() {
        let dir = std::env::temp_dir().join(format!("vtop-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = app3().export_to(&dir).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("vtop-") && name.ends_with(".json"), "{name}");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["requests"].as_array().unwrap().len(), 3);
        assert!(v["snapshot"]["vllm"].is_object());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
