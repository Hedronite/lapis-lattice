//! Small shell pieces shared by the sidebar, toolbars and dialogs.
use gpui_kit::{
    Div, ElementId, InteractiveElement, IntoElement, ParentElement, Role, Stateful,
    StatefulInteractiveElement, Styled, div, px,
};
use gpui_omarchy::{IconName, Theme, icon};

/// A key hint such as `⌘P`, drawn as a quiet chip.
pub(super) fn kbd(text: impl Into<gpui_kit::SharedString>, theme: &Theme) -> Div {
    div()
        .flex_none()
        .px_1p5()
        .rounded(px(4.))
        .border_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.secondary)
        .child(text.into())
}

/// A muted, small heading over a group of rows.
pub(super) fn section(text: impl Into<gpui_kit::SharedString>, theme: &Theme) -> Div {
    div().text_xs().text_color(theme.secondary.opacity(0.8)).child(text.into())
}

/// A square icon control with a hover wash; `on` marks a toggled state in gold.
pub(super) fn icon_button(
    id: impl Into<ElementId>,
    name: IconName,
    label: impl Into<gpui_kit::SharedString>,
    on: bool,
    enabled: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let colour = if on {
        theme.accent
    } else if enabled {
        theme.foreground
    } else {
        theme.secondary.opacity(0.45)
    };
    let mut button = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.into())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(6.))
        .text_color(colour)
        .child(icon(name).size(px(15.)));
    if on {
        button = button.bg(theme.accent.opacity(0.12));
    }
    if enabled {
        button = button.cursor_pointer().hover(|s| s.bg(theme.hover_fill()));
    }
    button
}

/// A text control with an optional leading icon.
pub(super) fn text_button(
    id: impl Into<ElementId>,
    name: Option<IconName>,
    label: impl Into<gpui_kit::SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    let label = label.into();
    let mut button = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .flex()
        .flex_none()
        .items_center()
        .gap_1p5()
        .px_2()
        .h(px(28.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_color(theme.foreground)
        .hover(|s| s.bg(theme.hover_fill()));
    if let Some(name) = name {
        button = button.child(icon(name).size(px(14.)).text_color(theme.secondary));
    }
    button.child(label)
}

/// A one-pixel vertical rule between toolbar groups.
pub(super) fn rule(theme: &Theme) -> Div {
    div().flex_none().w(px(1.)).h(px(16.)).mx_1().bg(theme.border)
}

/// The poster's hexagon chain: a few gold hairline cells, each with a centre dot.
pub(super) fn hex_chain(theme: &Theme) -> impl IntoElement + use<> {
    let line = theme.accent.opacity(0.55);
    let dot = theme.accent;
    // Pointy-top cells, r = 20, spaced 1.3× the tiling offset so hairlines never touch.
    const CELLS: [(f32, f32); 6] =
        [(60., 24.), (37.5, 63.), (82.5, 63.), (60., 102.), (37.5, 141.), (82.5, 141.)];
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let r = 20.0f32;
            let mut segs = Vec::new();
            for (cx, cy) in CELLS {
                let corner = |k: usize| {
                    let a = std::f32::consts::FRAC_PI_3 * k as f32 + std::f32::consts::FRAC_PI_6;
                    (cx + r * a.cos(), cy + r * a.sin())
                };
                for k in 0..6 {
                    let (x0, y0) = corner(k);
                    let (x1, y1) = corner(k + 1);
                    segs.push([x0, y0, x1, y1]);
                }
            }
            if let Some(path) = crate::window::segments_path(&segs, 1.0, bounds.origin) {
                window.paint_path(path, line);
            }
            for (cx, cy) in CELLS {
                let at = bounds.origin + gpui_kit::point(px(cx - 2.), px(cy - 2.));
                let square = gpui_kit::Bounds::new(at, gpui_kit::size(px(4.), px(4.)));
                window.paint_quad(gpui_kit::fill(square, dot).corner_radii(px(2.)));
            }
        },
    )
    .w(px(120.))
    .h(px(166.))
}

/// The floating panel behind search and the command menu.
pub(super) fn dialog(
    id: &'static str,
    label: impl Into<gpui_kit::SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Dialog)
        .aria_label(label.into())
        .w(px(640.))
        .max_w_full()
        .h_auto()
        .self_start()
        .rounded(px(12.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface)
        .shadow_lg()
        .overflow_hidden()
        .flex()
        .flex_col()
}

/// One gold hexagon outline, the brand mark at text size.
pub(super) fn hex_mark(theme: &Theme) -> impl IntoElement + use<> {
    let colour = theme.accent;
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (cx, cy, r) = (8.0f32, 8.0f32, 7.0f32);
            let corner = |k: usize| {
                let a = std::f32::consts::FRAC_PI_3 * k as f32 + std::f32::consts::FRAC_PI_6;
                (cx + r * a.cos(), cy + r * a.sin())
            };
            let segs: Vec<[f32; 4]> = (0..6)
                .map(|k| {
                    let ((x0, y0), (x1, y1)) = (corner(k), corner(k + 1));
                    [x0, y0, x1, y1]
                })
                .collect();
            if let Some(path) = crate::window::segments_path(&segs, 1.4, bounds.origin) {
                window.paint_path(path, colour);
            }
            let dot = gpui_kit::Bounds::new(
                bounds.origin + gpui_kit::point(px(cx - 1.5), px(cy - 1.5)),
                gpui_kit::size(px(3.), px(3.)),
            );
            window.paint_quad(gpui_kit::fill(dot, colour).corner_radii(px(1.5)));
        },
    )
    .flex_none()
    .size(px(16.))
}
