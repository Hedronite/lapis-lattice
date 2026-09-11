//! `lapis tui`: the human surface. Sidebar tree | tabs | Vim editor | rendered
//! preview, leader + which-key, palette, help, mouse, tasks/kanban/calendar,
//! periodic notes, templates, neighbors, HAL inspector, trash + restore.
//!
//! Lattice work runs on the tokio runtime and posts [`app::Msg`]s; the UI loop is
//! `block_in_place`. The watcher is non-recursive and capped (no EMFILE).

mod app;
mod draw;
mod keys;

mod hal_view;
mod help;
mod leader;
mod mouse;
mod neighbors_view;
mod omarchy;
mod palette;
mod preview;
mod tags_view;
mod tasks_view;
mod theme;
mod tree;
mod vim;

use std::time::{Duration, Instant};

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::DefaultTerminal;

use crate::error::{LapisError, Result};
use crate::ops::Ctx;
use app::{App, resolve_palette};

pub async fn run(ctx: Ctx) -> Result<()> {
    tokio::task::block_in_place(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
            ratatui::restore();
            default_hook(info);
        }));
        let mut term = ratatui::init();
        let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
        let (palette, omarchy_theme) = resolve_palette(&ctx.cfg.theme);
        theme::install(palette);
        let mut app = App::new(ctx);
        app.omarchy_theme = omarchy_theme;
        let result = ui_loop(&mut app, &mut term);
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        ratatui::restore();
        result
    })
}

fn ui_loop(app: &mut App, term: &mut DefaultTerminal) -> Result<()> {
    let mut last_health = Instant::now();
    while !app.quit {
        app.drain();
        if last_health.elapsed() > Duration::from_secs(30) {
            app.poll_health();
            last_health = Instant::now();
        }
        term.draw(|f| app.draw(f)).map_err(|e| LapisError::Internal(format!("draw: {e}")))?;
        if event::poll(Duration::from_millis(60)).map_err(|e| LapisError::Internal(e.to_string()))? {
            match event::read().map_err(|e| LapisError::Internal(e.to_string()))? {
                Event::Key(k) => app.key(k, term),
                Event::Mouse(m) => app.mouse(m),
                _ => {}
            }
        }
    }
    Ok(())
}
