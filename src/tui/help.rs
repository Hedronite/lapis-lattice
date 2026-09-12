//! `?` help overlay: the keymap, grouped. Scrolls with j/k.

use super::leader;

pub struct Section {
    pub title: &'static str,
    pub rows: Vec<(String, String)>,
}

pub fn sections() -> Vec<Section> {
    let s = |k: &str, v: &str| (k.to_string(), v.to_string());
    let mut out = vec![
        Section {
            title: "Global",
            rows: vec![
                s("Ctrl+P", "palette: search / commands (`>` prefix)"),
                s("Ctrl+S", "save current note"),
                s("Ctrl+Q / q", "quit (q from the sidebar or preview)"),
                s("?", "this help"),
                s("Space", "leader (which-key shows the next keys)"),
                s("Tab / Shift+Tab", "cycle focus: sidebar → editor → preview"),
                s("Alt+1..9", "go to tab"),
                s("Ctrl+W then h/l", "focus left / right pane"),
                s("Ctrl+W then < / >", "sidebar narrower / wider"),
                s("Ctrl+W then - / +", "preview narrower / wider"),
                s("Esc", "close overlay / back to sidebar"),
            ],
        },
        Section {
            title: "Sidebar",
            rows: vec![
                s("j / k", "move"),
                s("Enter / l", "open note or expand folder"),
                s("h", "collapse / go to parent"),
                s("o", "toggle folder"),
                s("/", "search"),
                s("x", "trash selected note"),
                s("n", "new note in selected folder"),
                s("r", "refresh folder"),
                s("gg / G", "top / bottom"),
                s("Mouse", "click to select/open, wheel to scroll"),
            ],
        },
        Section {
            title: "Editor (Vim)",
            rows: vec![
                s("h j k l  w b e  0 ^ $  gg G  { }", "motions"),
                s("i a I A o O", "insert"),
                s("x  dd yy cc  dw cw yw  D C  J", "edit / operators"),
                s("v V  then y d c", "visual"),
                s("p  u  Ctrl+R", "paste, undo, redo"),
                s("r R", "replace"),
                s("/pattern  n N", "search in note"),
                s(":w :q :wq :q! :<line>", "ex commands"),
                s("Mouse drag / Shift-click / double-click", "select / extend / select word"),
                s("Ctrl+C / Ctrl+X / Ctrl+V", "system copy / cut / paste (if terminal forwards them)"),
                s("\"+y / \"+yy / \"+p", "system register: copy selection / line / paste"),
                s("F6 / Space v m", "toggle capture for native terminal selection"),
                s("Terminal paste", "Cmd+V in Zed/macOS; Linux terminal's paste shortcut"),
                s("SSH clipboard", "copy via OSC 52 if allowed; paste from local terminal"),
                s("Ctrl+L", "toggle checkbox on line"),
                s("gt / gT", "next / previous tab"),
                s("Ctrl+D/U/F/B", "scroll"),
            ],
        },
        Section {
            title: "Preview",
            rows: vec![
                s("j / k / Ctrl+D / Ctrl+U", "scroll"),
                s("gg / G", "top / bottom"),
                s("Mouse wheel", "scroll"),
            ],
        },
        Section {
            title: "Tasks / Kanban / Calendar",
            rows: vec![
                s("j / k", "move card / row"),
                s("h / l", "switch column (kanban) / day (calendar)"),
                s("x", "toggle task (same id as `lapis task toggle`)"),
                s("Enter", "open the source note at that line"),
                s("r", "rescan vault"),
                s("Esc", "back to notes"),
            ],
        },
    ];
    let leader_rows = leader::all_commands().into_iter().map(|(k, c)| (k, c.label().to_string())).collect();
    out.push(Section { title: "Leader", rows: leader_rows });
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn help_has_leader_section() {
        let s = super::sections();
        assert!(s.iter().any(|x| x.title == "Leader" && x.rows.iter().any(|(k, _)| k == "Space d")));
    }
}
