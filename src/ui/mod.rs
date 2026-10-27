use std::time::Duration;

use crossterm::event::{self, Event, KeyCode};
use ratatui::widgets::Paragraph;

use crate::config::SharedConfig;
use crate::snapshot::Shared;

pub fn run(state: Shared, _cfg: SharedConfig) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    loop {
        let snap = state.read().unwrap().clone();
        terminal.draw(|f| {
            let text = format!("vtop  vLLM reachable: {}  (q quits)", snap.vllm.reachable);
            f.render_widget(Paragraph::new(text), f.area());
        })?;
        if event::poll(Duration::from_secs(1))?
            && let Event::Key(k) = event::read()?
            && k.code == KeyCode::Char('q')
        {
            break;
        }
    }
    ratatui::restore();
    Ok(())
}
