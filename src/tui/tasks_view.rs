//! Task views over the same scan and ids as `lapis task`: list, Kanban by
//! status column, calendar by due date.

use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use super::theme;
use crate::tasks::Task;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    List,
    Kanban,
    Calendar,
}

pub const COLUMNS: [&str; 4] = ["open", "in-progress", "waiting", "done"];

pub struct TasksView {
    pub view: View,
    pub tasks: Vec<Task>,
    pub loading: bool,
    /// list: row; kanban: card within column; calendar: task within day
    pub sel: usize,
    pub col: usize,
    /// calendar: first day of the shown month and the selected day (1-based)
    pub month: jiff::civil::Date,
    pub day: u8,
}

impl TasksView {
    pub fn new(view: View) -> Self {
        let today = jiff::Zoned::now().date();
        Self {
            view,
            tasks: vec![],
            loading: true,
            sel: 0,
            col: 0,
            month: today.first_of_month(),
            day: today.day() as u8,
        }
    }

    pub fn set_tasks(&mut self, tasks: Vec<Task>) {
        self.tasks = tasks;
        self.loading = false;
        self.sel = 0;
    }

    pub fn column_tasks(&self, col: usize) -> Vec<&Task> {
        let status = COLUMNS[col.min(COLUMNS.len() - 1)];
        self.tasks.iter().filter(|t| t.status == status).collect()
    }

    /// Tasks grouped by due date for the shown month.
    pub fn by_day(&self) -> BTreeMap<u8, Vec<&Task>> {
        let prefix = self.month.strftime("%Y-%m-").to_string();
        let mut m: BTreeMap<u8, Vec<&Task>> = BTreeMap::new();
        for t in &self.tasks {
            if let Some(d) = &t.due
                && let Some(day) = d.strip_prefix(&prefix)
                && let Ok(n) = day.parse::<u8>()
            {
                m.entry(n).or_default().push(t);
            }
        }
        m
    }

    /// The task under the cursor, whichever view is active.
    pub fn current(&self) -> Option<&Task> {
        match self.view {
            View::List => self.tasks.get(self.sel),
            View::Kanban => self.column_tasks(self.col).get(self.sel).copied(),
            View::Calendar => self.by_day().get(&self.day).and_then(|v| v.get(self.sel).copied()),
        }
    }

    pub fn current_len(&self) -> usize {
        match self.view {
            View::List => self.tasks.len(),
            View::Kanban => self.column_tasks(self.col).len(),
            View::Calendar => self.by_day().get(&self.day).map_or(0, Vec::len),
        }
    }

    pub fn down(&mut self) {
        self.sel = (self.sel + 1).min(self.current_len().saturating_sub(1));
    }
    pub fn up(&mut self) {
        self.sel = self.sel.saturating_sub(1);
    }
    pub fn left(&mut self) {
        match self.view {
            View::Kanban => {
                self.col = self.col.saturating_sub(1);
                self.sel = 0;
            }
            View::Calendar => {
                if self.day > 1 {
                    self.day -= 1;
                } else {
                    self.prev_month();
                    self.day = self.month.days_in_month() as u8;
                }
                self.sel = 0;
            }
            View::List => {}
        }
    }
    pub fn right(&mut self) {
        match self.view {
            View::Kanban => {
                self.col = (self.col + 1).min(COLUMNS.len() - 1);
                self.sel = 0;
            }
            View::Calendar => {
                if (self.day as i8) < self.month.days_in_month() {
                    self.day += 1;
                } else {
                    self.next_month();
                    self.day = 1;
                }
                self.sel = 0;
            }
            View::List => {}
        }
    }
    pub fn prev_month(&mut self) {
        self.month = self.month.saturating_sub(jiff::Span::new().months(1)).first_of_month();
        self.day = self.day.min(self.month.days_in_month() as u8);
    }
    pub fn next_month(&mut self) {
        self.month = self.month.saturating_add(jiff::Span::new().months(1)).first_of_month();
        self.day = self.day.min(self.month.days_in_month() as u8);
    }

    // ------------------------------------------------------------------ draw

    fn card_line(t: &Task, width: usize) -> Line<'static> {
        let mark = match t.status.as_str() {
            "done" => "✓",
            "in-progress" => "◐",
            "cancelled" => "✕",
            "forwarded" => "→",
            "waiting" => "…",
            _ => "○",
        };
        let mut meta = Vec::new();
        if let Some(d) = &t.due {
            meta.push(format!("due:{d}"));
        }
        if let Some(p) = &t.priority {
            meta.push(format!("!{p}"));
        }
        let content: String = t.content.chars().take(width.saturating_sub(4)).collect();
        Line::from(vec![
            Span::styled(format!("{mark} "), Style::default().fg(theme::GOLD)),
            Span::raw(content),
            Span::styled(
                if meta.is_empty() { String::new() } else { format!("  {}", meta.join(" ")) },
                theme::dim(),
            ),
        ])
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, focused: bool) {
        let border = if focused { theme::focused() } else { theme::chrome() };
        let title = match self.view {
            View::List => " tasks ",
            View::Kanban => " kanban ",
            View::Calendar => " calendar ",
        };
        let outer = Block::default().borders(Borders::ALL).title(title).border_style(border);
        let inner = outer.inner(area);
        f.render_widget(outer, area);
        if self.loading {
            f.render_widget(Paragraph::new("scanning vault…").style(theme::dim()), inner);
            return;
        }
        match self.view {
            View::List => self.draw_list(f, inner),
            View::Kanban => self.draw_kanban(f, inner),
            View::Calendar => self.draw_calendar(f, inner),
        }
    }

    fn draw_list(&self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .tasks
            .iter()
            .map(|t| {
                let mut l = Self::card_line(t, area.width as usize - 40);
                l.spans.push(Span::styled(format!("   {}", t.id), theme::dim()));
                ListItem::new(l)
            })
            .collect();
        let mut st = ListState::default().with_selected(Some(self.sel));
        f.render_stateful_widget(List::new(items).highlight_style(theme::selected()), area, &mut st);
    }

    fn draw_kanban(&self, f: &mut Frame, area: Rect) {
        let cols = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(area);
        for (i, col_area) in cols.iter().enumerate() {
            let tasks = self.column_tasks(i);
            let title = format!(" {} ({}) ", COLUMNS[i], tasks.len());
            let style = if i == self.col { theme::focused() } else { theme::chrome() };
            let block =
                Block::default().borders(Borders::LEFT | Borders::TOP).title(title).border_style(style);
            let inner = block.inner(*col_area);
            f.render_widget(block, *col_area);
            let items: Vec<ListItem> =
                tasks.iter().map(|t| ListItem::new(Self::card_line(t, inner.width as usize))).collect();
            let mut st = ListState::default();
            if i == self.col {
                st.select(Some(self.sel));
            }
            f.render_stateful_widget(List::new(items).highlight_style(theme::selected()), inner, &mut st);
        }
    }

    fn draw_calendar(&self, f: &mut Frame, area: Rect) {
        let [grid, list] = Layout::horizontal([Constraint::Length(36), Constraint::Fill(1)]).areas(area);
        let by_day = self.by_day();
        let mut lines: Vec<Line> = vec![
            Line::from(Span::styled(self.month.strftime("%B %Y").to_string(), theme::accent())),
            Line::from(Span::styled(" Mo  Tu  We  Th  Fr  Sa  Su", theme::dim())),
        ];
        let first_wd = self.month.weekday().to_monday_zero_offset() as usize;
        let days = self.month.days_in_month() as usize;
        let mut spans: Vec<Span> = vec![Span::raw("    ".repeat(first_wd))];
        let today = jiff::Zoned::now().date();
        for d in 1..=days {
            let n = by_day.get(&(d as u8)).map_or(0, Vec::len);
            let cell =
                if n > 0 { format!("{d:>2}{}", if n > 9 { "+" } else { "·" }) } else { format!("{d:>2} ") };
            let mut style =
                if n > 0 { Style::default().fg(theme::GOLD) } else { Style::default().fg(theme::CREAM) };
            if d as u8 == self.day {
                style = theme::selected();
            }
            if self.month.year() == today.year()
                && self.month.month() == today.month()
                && d as i8 == today.day()
            {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            spans.push(Span::styled(cell, style));
            spans.push(Span::raw(" "));
            if (first_wd + d).is_multiple_of(7) {
                lines.push(Line::from(std::mem::take(&mut spans)));
            }
        }
        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("h/l day · H/L month · x toggle · Enter open", theme::dim())));
        f.render_widget(Paragraph::new(lines), grid);

        let day_tasks = by_day.get(&self.day).cloned().unwrap_or_default();
        let title = format!(" due {} ({}) ", self.month.strftime("%Y-%m"), self.day);
        let items: Vec<ListItem> =
            day_tasks.iter().map(|t| ListItem::new(Self::card_line(t, list.width as usize))).collect();
        let mut st = ListState::default().with_selected(Some(self.sel));
        f.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::LEFT).title(title).border_style(theme::chrome()))
                .highlight_style(theme::selected()),
            list,
            &mut st,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks;

    fn sample() -> Vec<Task> {
        tasks::parse(
            "p.md",
            "- [ ] a due:2026-09-10\n- [x] b\n- [/] c\n- [ ] d @waiting due:2026-09-10\n- [ ] e due:2026-10-01\n",
        )
    }

    #[test]
    fn kanban_columns_and_navigation() {
        let mut v = TasksView::new(View::Kanban);
        v.set_tasks(sample());
        assert_eq!(v.column_tasks(0).len(), 2);
        assert_eq!(v.column_tasks(1).len(), 1);
        assert_eq!(v.column_tasks(2).len(), 1);
        assert_eq!(v.column_tasks(3).len(), 1);
        assert_eq!(v.current().unwrap().id, "p.md#0");
        v.down();
        assert_eq!(v.current().unwrap().id, "p.md#4");
        v.right();
        assert_eq!(v.current().unwrap().id, "p.md#2");
        v.right();
        v.right();
        v.right();
        assert_eq!(v.col, 3);
    }

    #[test]
    fn calendar_groups_by_due_in_month() {
        let mut v = TasksView::new(View::Calendar);
        v.set_tasks(sample());
        v.month = jiff::civil::date(2026, 9, 1);
        v.day = 10;
        let by = v.by_day();
        assert_eq!(by.get(&10).map(Vec::len), Some(2));
        assert!(!by.contains_key(&1));
        assert_eq!(v.current().unwrap().id, "p.md#0");
        v.next_month();
        assert_eq!(v.month, jiff::civil::date(2026, 10, 1));
        assert_eq!(v.by_day().get(&1).map(Vec::len), Some(1));
        v.prev_month();
        v.day = 30;
        v.right();
        assert_eq!((v.month.month(), v.day), (10, 1));
        v.left();
        assert_eq!((v.month.month(), v.day), (9, 30));
    }
}
