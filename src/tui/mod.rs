//! `lapis tui`: the human surface. Sidebar tree | tabs | Vim editor | rendered
//! preview, leader + which-key, palette, help, mouse, tasks/kanban/calendar,
//! periodic notes, templates, neighbors, HAL inspector, trash + restore.
//!
//! Lattice work runs on the tokio runtime and posts [`app::Msg`]s; the UI loop is
//! `block_in_place`. The watcher is non-recursive and capped (no EMFILE).

mod app;
mod clipboard;
mod draw;
mod keys;

mod hal_view;
mod help;
mod history;
mod index;
mod leader;
mod mouse;
mod neighbors_view;
mod omarchy;
mod palette;
mod paste;
mod pointer;
mod preview;
mod reader;
mod save;
mod tabs;
mod tags_view;
mod tasks_view;
mod theme;
mod tree;
mod vim;
mod welcome;

use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
};
use ratatui::DefaultTerminal;

use crate::error::{LapisError, Result};
use crate::ops::Ctx;
use app::{App, resolve_palette};

pub async fn run(ctx: Ctx) -> Result<()> {
    tokio::task::block_in_place(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste, DisableMouseCapture);
            ratatui::restore();
            default_hook(info);
        }));
        let mut term = ratatui::init();
        let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
        let (palette, omarchy_theme) = resolve_palette(&ctx.cfg.theme);
        theme::install(palette);
        let mut app = App::new(ctx);
        app.omarchy_theme = omarchy_theme;
        let result = ui_loop(&mut app, &mut term);
        let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste, DisableMouseCapture);
        ratatui::restore();
        result
    })
}

fn ui_loop(app: &mut App, term: &mut DefaultTerminal) -> Result<()> {
    let mut last_health = Instant::now();
    let mut redraw = true;
    let mut status_visible = false;
    while !app.quit {
        redraw |= app.drain();
        redraw |= app.pointer_tick();
        if last_health.elapsed() > Duration::from_secs(30) {
            app.poll_health();
            last_health = Instant::now();
        }
        // Keep idle notes quiet. Input, asynchronous results and drag scrolling
        // invalidate the frame; the transient status has its own expiry.
        let show_status = !app.status.is_empty() && app.status_at.elapsed() < Duration::from_secs(8);
        redraw |= show_status != status_visible;
        status_visible = show_status;
        if redraw {
            term.draw(|f| app.draw(f)).map_err(|e| LapisError::Internal(format!("draw: {e}")))?;
            redraw = false;
        }
        if event::poll(Duration::from_millis(60)).map_err(|e| LapisError::Internal(e.to_string()))? {
            match event::read().map_err(|e| LapisError::Internal(e.to_string()))? {
                Event::Key(k) => app.key(k, term),
                Event::Mouse(m) => app.mouse(m),
                Event::Paste(text) => app.paste(text),
                _ => {}
            }
            redraw = true;
        }
    }
    Ok(())
}
