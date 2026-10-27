pub mod app;
pub mod format;
pub mod overview;

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};

use crate::config::SharedConfig;
use crate::snapshot::Shared;
use app::App;

pub fn run(state: Shared, cfg: SharedConfig) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::default();
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    loop {
        let due = last_draw.elapsed() >= cfg.interval().min(Duration::from_secs(1));
        if due {
            if !app.frozen {
                let snap = state.read().unwrap().clone();
                app.ingest(snap);
            }
            terminal.draw(|f| overview::draw(f, &app, &cfg))?;
            last_draw = Instant::now();
        }
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(k) = event::read()?
            && k.kind != KeyEventKind::Release
        {
            if app.on_key(k, &cfg) {
                break;
            }
            terminal.draw(|f| overview::draw(f, &app, &cfg))?;
        }
    }
    ratatui::restore();
    Ok(())
}
