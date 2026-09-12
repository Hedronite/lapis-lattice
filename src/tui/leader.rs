//! Space leader with a which-key overlay. The table is data: every node is a
//! key, a label, and either an [`Cmd`] or a nested group.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    FindNote,
    SearchText,
    Commands,
    ToggleSidebar,
    Outline,
    TogglePreview,
    ToggleSplit,
    EditorOnly,
    PreviewOnly,
    ToggleWrap,
    ToggleLineNumbers,
    Kanban,
    TaskList,
    Calendar,
    Daily,
    Weekly,
    Monthly,
    NewNote,
    NewFromTemplate,
    QuickCapture,
    ExternalEditor,
    TrashNote,
    RestorePicker,
    CopyPath,
    CopySelection,
    CutSelection,
    PasteClipboard,
    ToggleMouse,
    Neighbors,
    Hal,
    Tags,
    Theme,
    FollowTheme,
    Buffers,
    TabNext,
    TabPrev,
    TabClose,
    Save,
    SaveCopy,
    Help,
    Quit,
    Refresh,
    /// Return to the notes view from tasks/kanban/calendar.
    NotesView,
    /// Build (or rebuild) the embedded search index in the background.
    BuildIndex,
}

impl Cmd {
    pub fn label(self) -> &'static str {
        match self {
            Cmd::FindNote => "find note (lattice)",
            Cmd::SearchText => "search text (bm25)",
            Cmd::Commands => "command palette",
            Cmd::ToggleSidebar => "toggle sidebar",
            Cmd::Outline => "note outline",
            Cmd::TogglePreview => "toggle preview",
            Cmd::ToggleSplit => "toggle split",
            Cmd::EditorOnly => "editor only",
            Cmd::PreviewOnly => "preview only",
            Cmd::ToggleWrap => "toggle wrap",
            Cmd::ToggleLineNumbers => "toggle line numbers",
            Cmd::Kanban => "kanban board",
            Cmd::TaskList => "task list",
            Cmd::Calendar => "calendar (by due)",
            Cmd::Daily => "today's daily note",
            Cmd::Weekly => "this week's note",
            Cmd::Monthly => "this month's note",
            Cmd::NewNote => "new note",
            Cmd::NewFromTemplate => "new from template",
            Cmd::QuickCapture => "quick capture",
            Cmd::ExternalEditor => "open in $EDITOR",
            Cmd::TrashNote => "trash note",
            Cmd::RestorePicker => "restore from trash",
            Cmd::CopyPath => "copy file path",
            Cmd::CopySelection => "copy selection to clipboard",
            Cmd::CutSelection => "cut selection to clipboard",
            Cmd::PasteClipboard => "paste system clipboard",
            Cmd::ToggleMouse => "toggle mouse capture (terminal selection)",
            Cmd::Neighbors => "neighbors (hop-1)",
            Cmd::Hal => "HAL inspector",
            Cmd::Tags => "tags browser",
            Cmd::Theme => "next theme (lapis · parchment · obsidian)",
            Cmd::FollowTheme => "follow host theme",
            Cmd::Buffers => "open buffers",
            Cmd::TabNext => "next tab",
            Cmd::TabPrev => "previous tab",
            Cmd::TabClose => "close tab",
            Cmd::Save => "save note",
            Cmd::SaveCopy => "save a copy (retain both versions)",
            Cmd::Help => "help",
            Cmd::Quit => "quit",
            Cmd::Refresh => "refresh tasks / tree",
            Cmd::NotesView => "notes view",
            Cmd::BuildIndex => "build search index (background)",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Node {
    Leaf(char, Cmd),
    Group(char, &'static str, Vec<Node>),
}

impl Node {
    pub fn key(&self) -> char {
        match self {
            Node::Leaf(k, _) | Node::Group(k, _, _) => *k,
        }
    }
    pub fn label(&self) -> String {
        match self {
            Node::Leaf(_, c) => c.label().to_string(),
            Node::Group(_, l, _) => format!("{l} ▸"),
        }
    }
}

pub fn table() -> Vec<Node> {
    use Node::*;
    vec![
        Leaf('f', Cmd::FindNote),
        Leaf('i', Cmd::BuildIndex),
        Group(
            's',
            "search",
            vec![
                Leaf('f', Cmd::FindNote),
                Leaf('t', Cmd::SearchText),
                Leaf('#', Cmd::Tags),
                Leaf('c', Cmd::Commands),
                Leaf('i', Cmd::BuildIndex),
            ],
        ),
        Leaf('e', Cmd::ToggleSidebar),
        Leaf('p', Cmd::Outline),
        Group(
            'z',
            "view",
            vec![
                Leaf('p', Cmd::TogglePreview),
                Leaf('s', Cmd::ToggleSplit),
                Leaf('e', Cmd::EditorOnly),
                Leaf('v', Cmd::PreviewOnly),
                Leaf('w', Cmd::ToggleWrap),
                Leaf('n', Cmd::ToggleLineNumbers),
                Leaf('b', Cmd::ToggleSidebar),
                Leaf('t', Cmd::Theme),
                Leaf('T', Cmd::FollowTheme),
                Leaf('m', Cmd::ToggleMouse),
            ],
        ),
        Leaf('k', Cmd::Kanban),
        Group(
            't',
            "tasks",
            vec![
                Leaf('l', Cmd::TaskList),
                Leaf('k', Cmd::Kanban),
                Leaf('c', Cmd::Calendar),
                Leaf('n', Cmd::NotesView),
            ],
        ),
        Leaf('c', Cmd::Calendar),
        Leaf('d', Cmd::Daily),
        Leaf('w', Cmd::Weekly),
        Leaf('m', Cmd::Monthly),
        Group(
            'n',
            "new",
            vec![Leaf('n', Cmd::NewNote), Leaf('t', Cmd::NewFromTemplate), Leaf('q', Cmd::QuickCapture)],
        ),
        Leaf('q', Cmd::QuickCapture),
        Group(
            'l',
            "note",
            vec![
                Leaf('e', Cmd::ExternalEditor),
                Leaf('x', Cmd::TrashNote),
                Leaf('r', Cmd::RestorePicker),
                Leaf('y', Cmd::CopyPath),
                Leaf('c', Cmd::CopySelection),
                Leaf('X', Cmd::CutSelection),
                Leaf('p', Cmd::PasteClipboard),
                Leaf('s', Cmd::Save),
                Leaf('S', Cmd::SaveCopy),
                Leaf('g', Cmd::Neighbors),
                Leaf('h', Cmd::Hal),
            ],
        ),
        Leaf('g', Cmd::Neighbors),
        Leaf('y', Cmd::Hal),
        Leaf('#', Cmd::Tags),
        Leaf('o', Cmd::Buffers),
        Group('b', "tabs", vec![Leaf('n', Cmd::TabNext), Leaf('p', Cmd::TabPrev), Leaf('x', Cmd::TabClose)]),
        Leaf('r', Cmd::Refresh),
        Leaf('h', Cmd::Help),
        Leaf('?', Cmd::Help),
        Leaf('x', Cmd::TrashNote),
        Leaf('Q', Cmd::Quit),
    ]
}

/// Walk the table by the keys pressed so far.
pub fn resolve<'a>(nodes: &'a [Node], keys: &[char]) -> Option<&'a [Node]> {
    let mut cur = nodes;
    for k in keys {
        let n = cur.iter().find(|n| n.key() == *k)?;
        match n {
            Node::Group(_, _, kids) => cur = kids,
            Node::Leaf(..) => return None,
        }
    }
    Some(cur)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Show these children next.
    Pending,
    Run(Cmd),
    Unknown,
}

pub fn step(keys: &[char]) -> Step {
    let t = table();
    let Some(parent) = resolve(&t, &keys[..keys.len().saturating_sub(1)]) else { return Step::Unknown };
    let Some(last) = keys.last() else { return Step::Pending };
    match parent.iter().find(|n| n.key() == *last) {
        Some(Node::Leaf(_, c)) => Step::Run(*c),
        Some(Node::Group(..)) => Step::Pending,
        None => Step::Unknown,
    }
}

/// Every reachable command with its key path, for the palette and help.
pub fn all_commands() -> Vec<(String, Cmd)> {
    fn walk(nodes: &[Node], prefix: &str, out: &mut Vec<(String, Cmd)>) {
        for n in nodes {
            match n {
                Node::Leaf(k, c) => {
                    if !out.iter().any(|(_, x)| x == c) {
                        out.push((format!("{prefix}{k}"), *c));
                    }
                }
                Node::Group(k, _, kids) => walk(kids, &format!("{prefix}{k} "), out),
            }
        }
    }
    let mut out = Vec::new();
    walk(&table(), "Space ", &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_through_groups() {
        assert_eq!(step(&['d']), Step::Run(Cmd::Daily));
        assert_eq!(step(&['t']), Step::Pending);
        assert_eq!(step(&['t', 'k']), Step::Run(Cmd::Kanban));
        assert_eq!(step(&['l', 'e']), Step::Run(Cmd::ExternalEditor));
        assert_eq!(step(&['z', 'p']), Step::Run(Cmd::TogglePreview));
        assert_eq!(step(&['#']), Step::Run(Cmd::Tags));
        assert_eq!(step(&['s', '#']), Step::Run(Cmd::Tags));
        assert_eq!(step(&['%']), Step::Unknown);
        assert_eq!(step(&['t', '#']), Step::Unknown);
        assert_eq!(step(&['i']), Step::Run(Cmd::BuildIndex));
        assert_eq!(step(&['s', 'i']), Step::Run(Cmd::BuildIndex));
    }

    #[test]
    fn which_key_children_and_flattening() {
        let t = table();
        let kids = resolve(&t, &['t']).unwrap();
        assert_eq!(kids.iter().map(Node::key).collect::<Vec<_>>(), ['l', 'k', 'c', 'n']);
        let all = all_commands();
        assert!(all.iter().any(|(k, c)| k == "Space t c" && *c == Cmd::Calendar));
        assert!(all.iter().any(|(k, c)| k == "Space d" && *c == Cmd::Daily));
        assert!(all.iter().any(|(k, c)| k == "Space i" && *c == Cmd::BuildIndex));
        // top-level keys are unique
        let mut keys: Vec<char> = t.iter().map(Node::key).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate leader keys");
    }
}
