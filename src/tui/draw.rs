//! Frame drawing: sidebar, tabs, editor, preview, overlays.

use std::collections::HashSet;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use super::app::{App, Focus, Overlay, Split, tab_label};
use super::hal_view;
use super::help;
use super::index::IndexState;
use super::leader;
use super::mouse::Regions;
use super::neighbors_view;
use super::palette::Item;
use super::theme;

impl App {
    pub(crate) fn draw(&mut self, f: &mut Frame) {
        if self.tabs.is_empty() && self.tasks.is_none() {
            self.show_sidebar = true;
            self.focus = Focus::Sidebar;
        } else if !self.show_sidebar && self.focus == Focus::Sidebar {
            self.focus = if self.tasks.is_some() {
                Focus::Tasks
            } else if self.split == Split::PreviewOnly {
                Focus::Preview
            } else {
                Focus::Editor
            };
        }
        if self.tasks.is_none() {
            if self.focus == Focus::Preview && self.split == Split::EditorOnly {
                self.focus = Focus::Editor;
            }
            if self.focus == Focus::Editor && self.split == Split::PreviewOnly {
                self.focus = Focus::Preview;
            }
        }
        let area = f.area();
        f.render_widget(Block::default().style(theme::base()), area);
        self.too_small = area.width < 45 || area.height < 12;
        if self.too_small {
            self.regions = Regions::default();
            self.pointer = Default::default();
            let dirty = self.tabs.iter().filter(|t| t.dirty).count();
            f.render_widget(Paragraph::new(format!("Lapis\nEnlarge to at least 45 × 12 to edit.\n{dirty} unsaved buffers retained.\nCtrl+Q quits when all buffers are saved.")).wrap(Wrap { trim: true }), area);
            return;
        }
        let [main, status] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        let (side, right) = if self.show_sidebar {
            let [s, r] = Layout::horizontal([Constraint::Percentage(self.sidebar_pct), Constraint::Fill(1)])
                .areas(main);
            (Some(s), r)
        } else {
            (None, main)
        };
        self.regions = Regions::default();
        if let Some(s) = side {
            self.regions.sidebar = s;
            self.draw_sidebar(f, s);
        }
        let [tabs, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(right);
        self.regions.tabs = tabs;
        self.draw_tabs(f, tabs);

        let (body, hal_area) = if self.show_hal {
            let [a, b] = Layout::horizontal([Constraint::Fill(1), Constraint::Percentage(32)]).areas(body);
            (a, Some(b))
        } else {
            (body, None)
        };
        let (body, nb_area) = if self.show_neighbors {
            let [a, b] = Layout::vertical([Constraint::Fill(1), Constraint::Length(9)]).areas(body);
            (a, Some(b))
        } else {
            (body, None)
        };

        if let Some(tv) = &self.tasks {
            self.regions.editor = body;
            tv.draw(f, body, self.focus == Focus::Tasks);
        } else {
            let split = if self.tabs.is_empty() {
                Split::EditorOnly
            } else if area.width < 100 && self.split == Split::Both {
                if self.focus == Focus::Preview { Split::PreviewOnly } else { Split::EditorOnly }
            } else {
                self.split
            };
            match split {
                Split::Both => {
                    let [e, p] =
                        Layout::horizontal([Constraint::Fill(1), Constraint::Percentage(self.preview_pct)])
                            .areas(body);
                    self.regions.editor = e;
                    self.regions.preview = p;
                    self.draw_editor(f, e);
                    self.draw_preview(f, p);
                }
                Split::EditorOnly => {
                    self.regions.editor = body;
                    self.draw_editor(f, body);
                }
                Split::PreviewOnly => {
                    self.regions.preview = body;
                    self.draw_preview(f, body);
                }
            }
        }
        if let Some(a) = hal_area {
            self.draw_hal(f, a);
        }
        if let Some(a) = nb_area {
            self.regions.bottom = a;
            self.draw_neighbors(f, a);
        }
        self.draw_status(f, status);
        self.draw_overlay(f, area);
    }

    pub(crate) fn draw_sidebar(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.tree.visible();
        let inner_h = area.height.saturating_sub(2) as usize;
        if self.sel < self.sidebar_scroll {
            self.sidebar_scroll = self.sel;
        } else if inner_h > 0 && self.sel >= self.sidebar_scroll + inner_h {
            self.sidebar_scroll = self.sel + 1 - inner_h;
        }
        let open: HashSet<&str> = self.tabs.iter().map(|t| t.rel.as_str()).collect();
        let items: Vec<ListItem> = rows
            .iter()
            .map(|(depth, e)| {
                let pad = "  ".repeat(*depth);
                let glyph = if e.is_dir {
                    if self.tree.expanded.contains(&e.rel) { "▾ " } else { "▸ " }
                } else if open.contains(e.rel.as_str()) {
                    "● "
                } else {
                    "  "
                };
                let style = if e.is_dir {
                    Style::default().fg(theme::current().regent)
                } else if open.contains(e.rel.as_str()) {
                    Style::default().fg(theme::gold())
                } else {
                    Style::default().fg(theme::cream())
                };
                ListItem::new(Line::from(Span::styled(format!("{pad}{glyph}{}", e.name), style)))
            })
            .collect();
        let focused = self.focus == Focus::Sidebar;
        let name =
            self.ctx.vault.root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {name} "))
            .border_style(if focused { theme::focused() } else { theme::chrome() });
        let mut state = ListState::default()
            .with_selected(Some(self.sel.min(rows.len().saturating_sub(1))))
            .with_offset(self.sidebar_scroll);
        f.render_stateful_widget(
            List::new(items).block(block).highlight_style(theme::selected()),
            area,
            &mut state,
        );
        self.sidebar_scroll = state.offset();
    }

    pub(crate) fn draw_tabs(&mut self, f: &mut Frame, area: Rect) {
        let paths: Vec<_> = self.tabs.iter().map(|t| (t.rel.as_str(), t.dirty)).collect();
        self.tab_slots = super::tabs::layout(&paths, self.active, area.width);
        for slot in &self.tab_slots {
            let style = if slot.index == self.active { theme::selected() } else { theme::dim() };
            f.render_widget(
                Paragraph::new(slot.label.as_str()).style(style),
                Rect::new(area.x + slot.cells.start, area.y, slot.cells.end - slot.cells.start, area.height),
            );
        }
    }

    pub(crate) fn draw_editor(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Editor;
        let border = if focused { theme::focused() } else { theme::chrome() };
        match self.tabs.get_mut(self.active) {
            Some(t) => {
                let title = format!(" {}{} ", t.rel, if t.dirty { " *" } else { "" });
                t.text.set_block(Block::default().borders(Borders::ALL).title(title).border_style(border));
                t.text.set_cursor_style(if focused { t.vim.cursor_style() } else { Style::default() });
                t.text.set_style(theme::base());
                t.text.set_selection_style(theme::selected());
                let wrap = if self.wrap {
                    ratatui_textarea::WrapMode::WordOrGlyph
                } else {
                    ratatui_textarea::WrapMode::None
                };
                if t.text.wrap_mode() != wrap {
                    t.text.set_wrap_mode(wrap);
                }
                f.render_widget(&t.text, area);
            }
            None => {
                super::welcome::draw(f, area, self.index.needs_setup());
            }
        }
    }

    pub(crate) fn draw_preview(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Preview;
        let border = if focused { theme::focused() } else { theme::chrome() };
        let wrap = self.wrap;
        let Some(t) = self.tabs.get_mut(self.active) else {
            f.render_widget(
                Block::default().borders(Borders::ALL).title(" preview ").border_style(border),
                area,
            );
            return;
        };
        t.refresh_preview();
        let block = Block::default().borders(Borders::ALL).title(" preview ").border_style(border);
        let content = block.inner(area);
        f.render_widget(block, area);
        t.reader.reflow(content.width, wrap);
        t.reader.draw(content, f.buffer_mut(), focused, theme::selected());
    }

    pub(crate) fn draw_hal(&self, f: &mut Frame, area: Rect) {
        let lines: Vec<Line> = match self.tab() {
            Some(t) => hal_view::lines(&t.hal, t.hal_valid),
            None => vec![],
        };
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .block(Block::default().borders(Borders::ALL).title(" HAL ").border_style(theme::chrome()))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    pub(crate) fn draw_neighbors(&self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = if self.neighbors.is_empty() {
            vec![ListItem::new(Span::styled("(no hop-1 neighbors, or still loading)", theme::dim()))]
        } else {
            neighbors_view::rows(&self.neighbors)
                .iter()
                .map(|r| ListItem::new(neighbors_view::line(r)))
                .collect()
        };
        let title = format!(" hop-1 neighbors · {} ", self.neighbors_for);
        f.render_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(title).border_style(theme::chrome())),
            area,
        );
    }

    pub(crate) fn draw_status(&self, f: &mut Frame, area: Rect) {
        let mut spans = Vec::new();
        let (mode, prompt) = match (self.focus, self.tab()) {
            (Focus::Editor, Some(t)) => (t.vim.mode_label(), t.vim.prompt.clone()),
            (Focus::Sidebar, _) => ("FILES".into(), None),
            (Focus::Preview, _) => ("PREVIEW".into(), None),
            (Focus::Tasks, _) => ("TASKS".into(), None),
            _ => ("LAPIS".into(), None),
        };
        spans.push(Span::styled(format!(" {mode} "), theme::mode(&mode)));
        if let Some(p) = prompt {
            spans.push(Span::styled(format!(" {}{}▏", p.kind, p.text), Style::default().fg(theme::gold())));
        } else {
            if let Some(t) = self.tab() {
                spans.push(Span::styled(format!(" {}", t.rel), Style::default().fg(theme::cream())));
                if t.dirty {
                    spans.push(Span::styled(" ●", Style::default().fg(theme::gold())));
                }
                if t.readonly {
                    spans.push(Span::styled(" [ro]", theme::dim()));
                }
                let c = t.text.cursor();
                spans.push(Span::styled(format!("  {}:{}", c.0 + 1, c.1 + 1), theme::dim()));
            }
            if self.status_at.elapsed() < Duration::from_secs(8) && !self.status.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", self.status),
                    Style::default().fg(theme::current().regent),
                ));
            }
        }
        let index = match &self.index {
            IndexState::Missing => {
                Some(Span::styled(" no search index · Space i builds ", Style::default().fg(theme::warn())))
            }
            IndexState::Failed(_) => {
                Some(Span::styled(" index failed · Space i retries ", Style::default().fg(theme::warn())))
            }
            IndexState::Indexing { done, total } if *total > 0 => {
                Some(Span::styled(format!(" indexing {done}/{total} "), Style::default().fg(theme::gold())))
            }
            IndexState::Indexing { .. } => {
                Some(Span::styled(" indexing… ", Style::default().fg(theme::gold())))
            }
            _ => None,
        };
        let lattice = match self.lattice_ok {
            Some(true) => Span::styled(" lattice ✓ ", Style::default().fg(theme::current().ok)),
            Some(false) => Span::styled(" lattice ✗ ", Style::default().fg(theme::warn())),
            None => Span::styled(" lattice … ", theme::dim()),
        };
        let tabs = if self.tabs.is_empty() {
            String::new()
        } else {
            format!(" {}/{} ", self.active + 1, self.tabs.len())
        };
        let left = Line::from(spans);
        let mut right = vec![Span::styled(tabs, theme::dim()), lattice];
        // The index hint yields to transient status and guard messages; it returns
        // once they expire.
        if let Some(index) = index
            && left.width() + index.width() + right.iter().map(Span::width).sum::<usize>()
                <= area.width as usize
        {
            right.insert(0, index);
        }
        let right = Line::from(right);
        let right_w = right.width() as u16;
        let [l, r] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(area);
        f.render_widget(Paragraph::new(left).style(theme::statusline()), l);
        f.render_widget(Paragraph::new(right).style(theme::statusline()), r);
    }

    pub(crate) fn draw_overlay(&self, f: &mut Frame, area: Rect) {
        let Some(ov) = &self.overlay else { return };
        match ov {
            Overlay::Leader(keys) => {
                let table = leader::table();
                let nodes = leader::resolve(&table, keys).unwrap_or_default();
                let mut lines: Vec<Line> = Vec::new();
                let title = if keys.is_empty() {
                    " Space ".to_string()
                } else {
                    format!(" Space {} ", keys.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" "))
                };
                let cols = 3usize;
                let per_col = nodes.len().div_ceil(cols).max(1);
                for row in 0..per_col {
                    let mut spans = Vec::new();
                    for c in 0..cols {
                        if let Some(n) = nodes.get(c * per_col + row) {
                            spans.push(Span::styled(format!(" {} ", n.key()), theme::accent()));
                            spans.push(Span::styled(
                                format!("{:<26}", n.label()),
                                Style::default().fg(theme::cream()),
                            ));
                        }
                    }
                    lines.push(Line::from(spans));
                }
                let h = (lines.len() as u16 + 2).min(area.height);
                let w = area.width.min(96);
                let popup = Rect {
                    x: (area.width - w) / 2,
                    y: area.height.saturating_sub(h + 1),
                    width: w,
                    height: h,
                };
                f.render_widget(Clear, popup);
                f.render_widget(
                    Paragraph::new(Text::from(lines)).style(theme::overlay()).block(
                        Block::default().borders(Borders::ALL).title(title).border_style(theme::focused()),
                    ),
                    popup,
                );
            }
            Overlay::Pane => {
                let popup = Rect {
                    x: area.width.saturating_sub(48),
                    y: area.height.saturating_sub(4),
                    width: 46.min(area.width),
                    height: 3,
                };
                f.render_widget(Clear, popup);
                f.render_widget(
                    Paragraph::new("h/l focus · < > sidebar · - + preview · v split · o one · q close")
                        .style(theme::overlay())
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(" Ctrl+W ")
                                .border_style(theme::focused()),
                        ),
                    popup,
                );
            }
            Overlay::Palette(p) => {
                let w = area.width * 3 / 4;
                let h = (area.height * 2 / 3).max(8);
                let popup = Rect { x: (area.width - w) / 2, y: (area.height - h) / 2, width: w, height: h };
                f.render_widget(Clear, popup);
                let [input, list] =
                    Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(popup);
                let title = if p.is_commands() {
                    " commands "
                } else if p.pending {
                    " lattice search… "
                } else if !p.asked.is_empty()
                    && self.index.needs_setup()
                    && p.items.iter().all(|i| matches!(i, Item::Command { .. }))
                {
                    " search index not built · Enter builds it in the background "
                } else if matches!(self.index, IndexState::Indexing { .. }) {
                    " lattice search · indexing, results incomplete "
                } else if !p.asked.is_empty() && p.items.is_empty() {
                    " lattice search · no matches "
                } else {
                    " lattice search  (type `>` for commands) "
                };
                f.render_widget(
                    Paragraph::new(format!("> {}▏", p.input)).style(theme::overlay()).block(
                        Block::default().borders(Borders::ALL).title(title).border_style(theme::focused()),
                    ),
                    input,
                );
                let items: Vec<ListItem> = p
                    .items
                    .iter()
                    .map(|it| match it {
                        Item::Note { path, title, snippet } => ListItem::new(vec![
                            Line::from(vec![
                                Span::styled(
                                    title.clone(),
                                    Style::default().fg(theme::cream()).add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(format!("  {path}"), theme::dim()),
                            ]),
                            Line::from(Span::styled(
                                snippet
                                    .clone()
                                    .unwrap_or_default()
                                    .chars()
                                    .take(list.width as usize - 4)
                                    .collect::<String>(),
                                theme::chrome(),
                            )),
                        ]),
                        Item::Command { keys, cmd } => ListItem::new(Line::from(vec![
                            Span::styled(format!("{keys:<14}"), theme::accent()),
                            Span::raw(cmd.label()),
                        ])),
                    })
                    .collect();
                let mut st = ListState::default().with_selected(Some(p.sel));
                f.render_stateful_widget(
                    List::new(items)
                        .style(theme::overlay())
                        .block(Block::default().borders(Borders::ALL).border_style(theme::chrome()))
                        .highlight_style(theme::selected()),
                    list,
                    &mut st,
                );
            }
            Overlay::Help(scroll) => {
                let popup = centered(area, 90, 90);
                f.render_widget(Clear, popup);
                let mut lines: Vec<Line> = Vec::new();
                for s in help::sections() {
                    lines.push(Line::from(Span::styled(s.title, theme::accent())));
                    for (k, v) in s.rows {
                        lines.push(Line::from(vec![
                            Span::styled(format!("  {k:<34}"), Style::default().fg(theme::gold())),
                            Span::raw(v),
                        ]));
                    }
                    lines.push(Line::default());
                }
                let max = lines.len().saturating_sub(popup.height.saturating_sub(2) as usize) as u16;
                let scroll = (*scroll as u16).min(max);
                f.render_widget(
                    Paragraph::new(Text::from(lines)).style(theme::overlay()).scroll((scroll, 0)).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" help  (j/k scroll · Esc close) ")
                            .border_style(theme::focused()),
                    ),
                    popup,
                );
            }
            Overlay::Outline(sel, items) => {
                self.draw_picker(
                    f,
                    area,
                    " outline ",
                    *sel,
                    items
                        .iter()
                        .map(|(_, lvl, t)| format!("{}{t}", "  ".repeat((*lvl as usize).saturating_sub(1))))
                        .collect(),
                );
            }
            Overlay::Buffers(sel) => {
                self.draw_picker(
                    f,
                    area,
                    " buffers  (Enter switch · x close) ",
                    *sel,
                    self.tabs.iter().map(tab_label).collect(),
                );
            }
            Overlay::Restore(sel, items) => {
                self.draw_picker(f, area, " trash  (Enter restores) ", *sel, items.clone());
            }
            Overlay::Tags(b) => {
                let mut rows = b.rows();
                if rows.is_empty() && !b.loading {
                    rows.push("(no tags)".into());
                }
                self.draw_picker(f, area, &b.title(), b.selected_index(), rows);
            }
            Overlay::Templates(sel, items) => {
                self.draw_picker(
                    f,
                    area,
                    " new from template ",
                    *sel,
                    items.iter().map(|t| format!("{:<24} {}   {}", t.id, t.name, t.target)).collect(),
                );
            }
            Overlay::Prompt(title, input, _) => {
                let w = area.width.min(70);
                let popup = Rect { x: (area.width - w) / 2, y: area.height / 3, width: w, height: 3 };
                f.render_widget(Clear, popup);
                f.render_widget(
                    Paragraph::new(format!("{input}▏")).style(theme::overlay()).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(format!(" {title} "))
                            .border_style(theme::focused()),
                    ),
                    popup,
                );
            }
        }
    }

    pub(crate) fn draw_picker(&self, f: &mut Frame, area: Rect, title: &str, sel: usize, rows: Vec<String>) {
        let popup = centered(area, 70, 60);
        f.render_widget(Clear, popup);
        let items: Vec<ListItem> = rows.into_iter().map(ListItem::new).collect();
        let mut st = ListState::default().with_selected(Some(sel));
        f.render_stateful_widget(
            List::new(items)
                .style(theme::overlay())
                .block(Block::default().borders(Borders::ALL).title(title).border_style(theme::focused()))
                .highlight_style(theme::selected()),
            popup,
            &mut st,
        );
    }
}

fn centered(area: Rect, pct_w: u16, pct_h: u16) -> Rect {
    let w = area.width * pct_w / 100;
    let h = area.height * pct_h / 100;
    Rect { x: (area.width - w) / 2, y: (area.height - h) / 2, width: w, height: h }
}
