//! The graph's frame: the board, a floating toolbar over it, a legend and a preview
//! card. Labels stay elements so they use the theme's text stack; they are placed
//! from the same paint list as the discs, so they cannot drift.
use super::board::{Board, folder_hsla};
use super::*;
use gpui_omarchy::{IconName, icon};

/// A floating panel over the board.
fn panel(theme: &gpui_omarchy::Theme) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap_0p5()
        .p_1()
        .rounded(px(10.))
        .bg(theme.surface.opacity(0.9))
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
}

/// A toolbar control: an optional icon and a label, gold while `on`.
fn chip(
    id: &'static str,
    glyph: Option<IconName>,
    text: String,
    on: bool,
    theme: &gpui_omarchy::Theme,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let mut c = div()
        .id(id)
        .role(gpui_kit::Role::Button)
        .aria_label(text.clone())
        .flex()
        .items_center()
        .gap_1p5()
        .h(px(28.))
        .px_2p5()
        .rounded(px(7.))
        .cursor_pointer()
        .text_sm()
        .whitespace_nowrap();
    c = if on {
        c.bg(theme.accent.opacity(0.14)).text_color(theme.accent)
    } else {
        c.text_color(theme.foreground).hover(|s| s.bg(theme.hover_fill()))
    };
    if let Some(glyph) = glyph {
        c = c.child(icon(glyph).size(px(14.)).text_color(if on { theme.accent } else { theme.secondary }));
    }
    c.child(text)
}

fn rule(theme: &gpui_omarchy::Theme) -> gpui_kit::Div {
    div().w(px(1.)).h(px(16.)).mx_1().bg(theme.border)
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.omarchy().clone();
        // The first frame lays out against the fallback size; once the
        // canvas reports its real bounds, frame the graph for those.
        let board_now = self.board();
        if self.fitted_for != (board_now[0], board_now[1]) {
            self.camera.fit(&self.scene, board_now);
            self.fit_zoom = self.camera.zoom;
            self.fitted_for = (board_now[0], board_now[1]);
        }
        // The graph keeps moving until it settles; each frame asks for the
        // next one, and a graph at rest stops asking.
        let moving = self.visible && !self.loading && self.advance();
        if moving {
            self.meter.frame();
            window.request_animation_frame();
        } else {
            self.meter.idle();
        }
        let seen = self.visible();
        let focus = self.hover.as_deref().or(self.selected.as_deref()).and_then(|id| seen.index_of(id));
        let hi = highlight(&seen, focus);
        let pins = self.pins.clone();
        let prims =
            paint(&seen, &self.camera, self.board(), &hi, &move |id| pins.contains_key(id), &self.groups);
        let labels: Vec<Prim> = prims.iter().filter(|p| matches!(p, Prim::Label { .. })).cloned().collect();
        let label_count = labels.len();
        let focus_label = focus.map(|i| crate::view::label_stem(&seen.nodes[i].label));
        let folder: Vec<Option<usize>> = seen
            .nodes
            .iter()
            .map(|n| {
                crate::graph_style::folder_of(&n.id)
                    .and_then(|f| self.folders.iter().position(|(top, _)| top == f))
            })
            .collect();
        let draw_last = self.draw_ms.get();
        let painter = Board {
            prims,
            focus,
            folder,
            pan: [self.camera.pan_x, self.camera.pan_y],
            theme: theme.clone(),
            show_boxes: debug_overlay_on(),
            origin: self.origin.clone(),
            board_px: self.board_px.clone(),
            draw_ms: self.draw_ms.clone(),
        };
        // Strokes are real hairlines built with PathBuilder-style triangles; see board.rs.
        let painted = canvas(move |_, _, _| {}, move |bounds, _, window, _| painter.paint(bounds, window))
            .absolute()
            .size_full();

        let mut board = div()
            .id("graph-board")
            .track_focus(&self.focus)
            .relative()
            // The board is the whole graph view: it fills whatever the workspace gives it.
            .w_full()
            .h_full()
            .min_h(px(120.))
            .overflow_hidden()
            .bg(theme.background)
            .child(painted)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev, w, cx| {
                    this.focus.focus(w, cx);
                    this.on_down(ev, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev, w, cx| {
                    this.focus.focus(w, cx);
                    this.on_down(ev, cx)
                }),
            )
            .on_mouse_move(cx.listener(|this, ev, w, cx| this.on_move(ev, w, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, ev, w, cx| this.on_up(ev, w, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, ev, w, cx| this.on_up(ev, w, cx)))
            .on_scroll_wheel(cx.listener(|this, ev, _, cx| this.on_wheel(ev, cx)))
            .on_key_down(cx.listener(|this, ev, w, cx| this.on_key(ev, w, cx)));

        // Each label is drawn in exactly the rectangle the placer reserved for it:
        // same width, same height, clipped, with the line height pinned. gpui's
        // default line box is over two ems tall; nothing here may grow past its box.
        for l in labels {
            let Prim::Label { x, y, text, alpha } = l else { continue };
            let w = crate::view::label_width_px(&text);
            if x + w < 0.0 || y + LABEL_H < 0.0 || x - w > board_now[0] || y > board_now[1] {
                continue;
            }
            let lit = focus_label.as_deref() == Some(text.as_str());
            board = board.child(
                div()
                    .absolute()
                    .left(px(x - w / 2.0))
                    .top(px(y))
                    .w(px(w))
                    .h(px(LABEL_H))
                    .overflow_hidden()
                    .flex()
                    .justify_center()
                    .when_lit(lit, &theme)
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(LABEL_H))
                            .whitespace_nowrap()
                            // A label outside the lit neighbourhood fades with its disc.
                            .text_color(if lit {
                                theme.bright
                            } else if alpha < FULL_ALPHA {
                                with_alpha(theme.secondary, 0.3)
                            } else {
                                with_alpha(theme.secondary, 0.85)
                            })
                            .child(text),
                    ),
            );
        }

        if debug_overlay_on() {
            let (lo, hi) = self.sim.extent();
            let overlay = format!(
                "{:.0} fps · worst {:.1} ms · tick {:.2} · build {:.2} · draw {:.2} ms · \
                 {} nodes / {} edges / {} labels · energy {:.4} · {}",
                self.meter.fps(),
                self.meter.worst_ms(),
                self.meter.tick_ms,
                self.meter.build_ms,
                draw_last,
                self.scene.nodes.len(),
                self.scene.edges.len(),
                label_count,
                self.sim.energy(),
                if moving {
                    format!("settling, span {:.0}", (hi[0] - lo[0]).max(hi[1] - lo[1]))
                } else {
                    "at rest".to_string()
                },
            );
            board = board.child(
                div().absolute().left(px(14.)).top(px(56.)).text_xs().text_color(theme.accent).child(overlay),
            );
        }

        let summary = if self.loading {
            "Loading graph… Notes remain available.".to_string()
        } else if let Some(error) = &self.error {
            format!("Graph unavailable: {error}")
        } else if self.scene.nodes.is_empty() {
            "No indexed notes. Build the index from Find, then Refresh graph.".into()
        } else if seen.nodes.is_empty() {
            "No nodes match these filters. Reset filters to show the graph.".into()
        } else {
            format!(
                "{} of {} notes · {} links{}",
                seen.nodes.len(),
                self.scene.nodes.len(),
                seen.edges.len(),
                if self.scene.truncated { " · partial snapshot" } else { "" },
            )
        };
        let quiet = self.loading || self.error.is_some() || seen.nodes.is_empty();
        drop(seen);
        if quiet {
            // The centred card carries the message; the legend stays only for groups.
            board = board.child(self.status_card(&summary, &theme));
        }
        if !quiet || !self.groups.is_empty() {
            let line = if quiet { String::new() } else { summary };
            board = board.child(self.legend(line, quiet, &theme, cx));
        }
        board.child(self.toolbar(&theme, cx)).child(self.preview_card(&theme))
    }
}

/// The focused label sits on a small pill so it reads over busy edges.
trait Lit {
    fn when_lit(self, lit: bool, theme: &gpui_omarchy::Theme) -> Self;
}
impl Lit for gpui_kit::Div {
    fn when_lit(self, lit: bool, theme: &gpui_omarchy::Theme) -> Self {
        if lit { self.bg(theme.surface.opacity(0.92)).rounded(px(5.)) } else { self }
    }
}

impl Root {
    fn toolbar(&self, theme: &gpui_omarchy::Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let layout = match self.layout {
            Layout::Global => "Whole vault".to_string(),
            Layout::Local => "Local".to_string(),
            other => other.label().to_string(),
        };
        let query = if self.typing {
            format!("{}▏", self.filters.query)
        } else if self.filters.query.is_empty() {
            "Filter notes  /".to_string()
        } else {
            self.filters.query.clone()
        };
        let domain = format!("Domain: {}", self.filters.domain.clone().unwrap_or_else(|| "all".into()));
        let zoom = format!("{:.0}%", self.camera.zoom / self.fit_zoom.max(f32::EPSILON) * 100.0);

        let mut left = panel(theme)
            .child(
                chip("f-query", Some(IconName::Search), query, self.typing, theme)
                    .min_w(px(150.))
                    .border_1()
                    .border_color(if self.typing { theme.accent.opacity(0.6) } else { theme.border })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.focus.focus(window, cx);
                        this.typing = !this.typing;
                        cx.notify();
                    })),
            )
            .child(rule(theme))
            .child(chip("f-layout", Some(IconName::Network), layout, false, theme).on_click(cx.listener(
                |this, _, window, cx| {
                    this.layout = this.next_layout();
                    this.reload(window, cx);
                    cx.notify();
                },
            )));
        if self.layout == Layout::Local {
            left = left.child(chip("f-depth", None, format!("Depth {}", self.depth), false, theme).on_click(
                cx.listener(|this, _, window, cx| {
                    this.depth = if this.depth >= graph_data::MAX_DEPTH {
                        graph_data::MIN_DEPTH
                    } else {
                        this.depth + 1
                    };
                    this.reload(window, cx);
                    cx.notify();
                }),
            ));
        }
        left = left
            .child(rule(theme))
            .child(chip("f-orphans", None, "Orphans".into(), self.filters.show_orphans, theme).on_click(
                cx.listener(|this, _, _, cx| {
                    this.filters.show_orphans = !this.filters.show_orphans;
                    cx.notify();
                }),
            ))
            .child(chip("f-existing", None, "Unresolved".into(), self.filters.show_dangling, theme).on_click(
                cx.listener(|this, _, _, cx| {
                    this.filters.show_dangling = !this.filters.show_dangling;
                    cx.notify();
                }),
            ))
            .child(
                chip("f-domain", Some(IconName::ChevronDown), domain, self.filters.domain.is_some(), theme)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cycle_domain();
                        cx.notify();
                    })),
            )
            .child(
                chip("f-group", Some(IconName::Plus), "Colour group".into(), false, theme)
                    .aria_label("Colour the notes matching the filter")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_group();
                        cx.notify();
                    })),
            );

        let mut right = panel(theme).child(
            chip("f-zoom", Some(IconName::Maximize), zoom, false, theme)
                .aria_label("Fit the graph")
                .on_click(cx.listener(|this, _, _, cx| {
                    let board = this.board();
                    this.camera.fit(&this.scene, board);
                    this.fit_zoom = this.camera.zoom;
                    cx.notify();
                })),
        );
        if !self.pins.is_empty() {
            right = right.child(
                chip("f-unpin", None, format!("Unpin {}", self.pins.len()), true, theme).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pins.clear();
                        this.sim.unpin_all();
                        cx.notify();
                    }),
                ),
            );
        }
        right = right
            .child(chip("graph-refresh", Some(IconName::RotateCw), "Refresh".into(), false, theme).on_click(
                cx.listener(|this, _, w, cx| {
                    this.snapshot = None;
                    this.reload(w, cx);
                }),
            ))
            .child(
                chip("graph-reset", None, "Reset".into(), false, theme).aria_label("Reset filters").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.filters = Filters::default();
                        this.typing = false;
                        cx.notify();
                    }),
                ),
            );
        div()
            .absolute()
            .top(px(12.))
            .left(px(12.))
            .right(px(12.))
            .flex()
            .items_start()
            .justify_between()
            .gap_2()
            .child(left.flex_wrap())
            .child(right)
    }

    /// Folder colours, colour groups and the counts, in the lower left.
    fn legend(
        &self,
        summary: String,
        quiet: bool,
        theme: &gpui_omarchy::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let swatch = |colour: Hsla| div().flex_none().size(px(8.)).rounded(px(4.)).bg(colour);
        let mut card = div()
            .absolute()
            .left(px(12.))
            .bottom(px(12.))
            .max_w(px(280.))
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded(px(10.))
            .bg(theme.surface.opacity(0.88))
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .text_xs()
            .child(
                div()
                    .text_sm()
                    .text_color(if self.error.is_some() { theme.danger } else { theme.foreground })
                    .child(summary),
            );
        if !quiet {
            for (slot, (name, count)) in self.folders.iter().enumerate() {
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(theme.secondary)
                        .child(swatch(folder_hsla(theme, slot)))
                        .child(div().flex_1().truncate().child(name.clone()))
                        .child(format!("{count}")),
                );
            }
        }
        // Each group shows in its own colour; clicking one removes it.
        for (n, g) in self.groups.iter().enumerate() {
            card = card.child(
                div()
                    .id(gpui_kit::SharedString::from(format!("group-{n}")))
                    .role(gpui_kit::Role::Button)
                    .aria_label(format!("Remove group {}", g.query))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .text_color(theme.foreground)
                    .hover(|s| s.text_color(theme.bright))
                    .child(swatch(group_hsla(g.colour, theme)))
                    .child(div().flex_1().truncate().child(g.query.clone()))
                    .child(icon(IconName::Close).size(px(10.)).text_color(theme.secondary))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if n < this.groups.len() {
                            this.groups.remove(n);
                        }
                        cx.notify();
                    })),
            );
        }
        // The live theme by name, so a swap is visible in a screenshot and not just a claim.
        card.child(
            div()
                .id("f-theme")
                .mt_1()
                .text_color(theme.secondary.opacity(0.7))
                .child(format!("{} theme", theme.name)),
        )
    }

    /// Loading, empty and failed boards say so in the middle, where the graph would be.
    fn status_card(&self, summary: &str, theme: &gpui_omarchy::Theme) -> impl IntoElement + use<> {
        let (title, tone) = if self.loading {
            ("Laying out the graph", theme.accent)
        } else if self.error.is_some() {
            ("The graph is unavailable", theme.danger)
        } else {
            ("Nothing to show", theme.secondary)
        };
        div().absolute().inset_0().flex().items_center().justify_center().child(
            div()
                .max_w(px(440.))
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .px_6()
                .py_5()
                .rounded(px(12.))
                .bg(theme.surface.opacity(0.85))
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .child(
                    div()
                        .font_family(crate::theme::SERIF)
                        .text_size(px(22.))
                        .text_color(theme.bright)
                        .child(title),
                )
                .child(div().size(px(6.)).rounded(px(3.)).bg(tone))
                .child(div().text_sm().text_center().text_color(theme.secondary).child(summary.to_string())),
        )
    }

    /// The hovered or selected note, or how to drive the board, in the lower right.
    fn preview_card(&self, theme: &gpui_omarchy::Theme) -> impl IntoElement + use<> {
        let card = div()
            .id("graph-preview")
            .absolute()
            .right(px(12.))
            .bottom(px(12.))
            .w(px(360.))
            .max_h(px(160.))
            .overflow_y_scroll()
            .p_3()
            .rounded(px(10.))
            .text_xs();
        let Some(peek) = self.peek.clone() else {
            return card.text_color(theme.secondary.opacity(0.8)).child(
                "Click a note to open · Tab selects · Enter opens · / filters · arrows pan · 0 fits · \
                 wheel zooms · drag a note to pin it",
            );
        };
        let (path, text) = (peek.0.as_str(), peek.1.as_str());
        card.bg(theme.surface.opacity(0.92))
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.bright)
                    .truncate()
                    .child(crate::view::label_stem(path.trim_start_matches("dangling:"))),
            )
            .child(div().text_color(theme.secondary).truncate().child(path.to_string()))
            .child(div().mt_1().line_clamp(5).text_color(theme.foreground).child(text.to_string()))
    }
}
