#!/usr/bin/env python3
"""Run the TUI PTY routes against one binary and write one evidence record per
checklist ID (U1c, U2b, U2d) in the UX evidence schema shape, mapping every rubric
row of the ID to the routes/cases that cover it.

The verdict is computed from what ran here: `pass` only when every row of the ID is
covered by passing routes and the ID has no outstanding native row; otherwise
`pending` with the missing rows in `limitations`. Native rows (a person pasting in
Zed on castle, a linux-smoke-host terminal) can be supplied with --native as a JSON list of
{row, host, terminal, steps, result} entries and are merged verbatim.

This does not touch a real vault; the smokes create disposable fixtures under --out.
"""
import argparse, datetime, hashlib, json, os, platform, subprocess, sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--bin', required=True)
ap.add_argument('--out', required=True)
ap.add_argument('--code-sha', required=True, help='full git SHA the binary was built from')
ap.add_argument('--host', required=True, help='e.g. "castle; release built on citadel"')
ap.add_argument('--ids', default='U1c,U2b,U2d')
ap.add_argument('--native', help='JSON list of native rows done by a person, merged into the record')
ap.add_argument('--schema', help='path to evidence.schema.json to validate against (needs jsonschema)')
ap.add_argument('--artifact-prefix', default='', help='prefix for artifact paths, e.g. runtime/tip-<sha7>-tui/ when the records live in evidence/ux/')
args = ap.parse_args()
out = Path(args.out).resolve()
out.mkdir(parents=True, exist_ok=True)
ids = [i.strip() for i in args.ids.split(',') if i.strip()]
binary = Path(args.bin).resolve()
binary_sha = hashlib.sha256(binary.read_bytes()).hexdigest()
now = datetime.datetime.now(datetime.timezone.utc).isoformat()
uname = platform.uname()
os_text = f'{uname.system} {uname.release} {uname.machine}'
if uname.system == 'Darwin':
    ver = subprocess.run(['sw_vers', '-productVersion'], capture_output=True, text=True).stdout.strip()
    cpu = subprocess.run(['sysctl', '-n', 'machdep.cpu.brand_string'], capture_output=True, text=True).stdout.strip()
    os_text = f'macOS {ver} {uname.machine}, {cpu}'
native = json.loads(Path(args.native).read_text()) if args.native else []


def run(script, extra):
    log = out / f'{script}.log'
    proc = subprocess.run([sys.executable, str(HERE / f'{script}.py'), '--bin', str(binary), '--out', str(out / script), *extra],
                          capture_output=True, text=True)
    log.write_text(proc.stdout + proc.stderr)
    return proc.returncode


results = {}
if any(i in ids for i in ('U2b', 'U2d')):
    rc = run('ux-workflow-smoke', [])
    results['workflow'] = json.loads((out / 'ux-workflow-smoke' / 'results.json').read_text())
    results['workflow']['exit'] = rc
if 'U1c' in ids:
    rc = run('ux-input-smoke', ['--large'])
    text = (out / 'ux-input-smoke.log').read_text()
    results['input'] = json.loads(text[text.index('{'):text.rindex('}') + 1])
    results['input']['exit'] = rc


def route_ok(name):
    return next((r for r in results.get('workflow', {}).get('routes', []) if r['route'] == name), {}).get('ok', False)


def route_ms(name):
    return next((r for r in results.get('workflow', {}).get('routes', []) if r['route'] == name), {}).get('ms')


def case_ok(name):
    rows = [r for r in results.get('input', {}).get('results', []) if r['case'] == name]
    return bool(rows) and not any(v is False for r in rows for v in r.values())


ROWS = {
    'U1c': {
        'expected': 'LF/CRLF multiline paste preserves content in normal/insert/visual modes; single undo/redo, literal command-like text, read-only handling, terminal restoration, and large payload pass.',
        'surface': 'tui', 'kind': 'acceptance',
        'rows': {
            'LF/CRLF paste in normal mode (raw and bracketed)': ('case', ['paste-lf-normal', 'paste-crlf-normal', 'bracketed-normal']),
            'LF/CRLF paste in insert mode (raw and bracketed)': ('case', ['paste-lf-insert', 'paste-crlf-insert', 'bracketed-insert']),
            'paste over a visual selection': ('case', ['visual-paste-undo']),
            'single undo/redo for a paste': ('case', ['paste-undo', 'visual-paste-undo', 'linewise-replace-undo', 'mouse-replace-undo']),
            'literal command-like text': ('case', ['command-like-normal', 'bracketed-normal']),
            'read-only handling': ('case', ['html-static-reading', 'pdf-reference.pdf', 'pdf-scanned.pdf']),
            'terminal restoration after an external editor': ('case', ['external-editor']),
            '1 MiB payload integrates with one undo/redo': ('case', ['large-paste']),
        },
        'native_rows': ['castle / Zed terminal: actual multiline paste with the terminal paste shortcut', 'linux-smoke-host / installed terminal: actual multiline paste'],
        'fixture': 'clipboard-fixture (ux-input-smoke.py)',
    },
    'U2b': {
        'expected': 'Context help, command palette, navigation history, outline, and mouse/keyboard parity pass; search pending/empty/error states are distinct.',
        'surface': 'tui', 'kind': 'acceptance',
        'rows': {
            'context help': ('route', ['help']),
            'command palette (results, commands, no matches)': ('route', ['palette-states']),
            'navigation history (Ctrl+O, Alt+arrows, Space b h/l, ends reported)': ('route', ['history']),
            'outline jump': ('route', ['outline']),
            'mouse/keyboard parity (sidebar click, tab click, pane click, Tab, Alt+digit)': ('route', ['mouse-parity']),
            'search states distinct: no matches vs lattice error vs results': ('route', ['palette-states', 'lattice-error']),
        },
        'native_rows': [],
        'fixture': 'workflow-fixture (ux-workflow-smoke.py)',
    },
    'U2d': {
        'expected': 'Tasks/Kanban/calendar, dailies, templates, tags, HAL, trash/restore, and external-editor workflow remain accessible and correct.',
        'surface': 'tui', 'kind': 'acceptance',
        'rows': {
            'tasks list (folder summary, rows), kanban, calendar': ('route', ['tasks']),
            'daily note (created on disk, opened)': ('route', ['daily']),
            'new note and new from template (files on disk)': ('route', ['new-note', 'template']),
            'quick capture (inbox file)': ('route', ['capture']),
            'tags browser (filter, drill-in)': ('route', ['tags']),
            'HAL inspector': ('route', ['hal']),
            'trash and restore (bucket path, file back)': ('route', ['trash-restore']),
            'external editor round trip with reload': ('route', ['external-editor']),
        },
        'native_rows': [],
        'fixture': 'workflow-fixture (ux-workflow-smoke.py)',
    },
}


def fixture_manifest_sha(kind):
    src = (HERE / ('ux-input-smoke.py' if kind.startswith('clipboard') else 'ux-workflow-smoke.py')).read_bytes()
    return hashlib.sha256(src).hexdigest()


written = []
for id_ in ids:
    spec = ROWS[id_]
    row_results, steps, artifacts, failed_rows = {}, [], [], []
    for row, (how, names) in spec['rows'].items():
        oks = {n: (route_ok(n) if how == 'route' else case_ok(n)) for n in names}
        row_results[row] = oks
        if not all(oks.values()):
            failed_rows.append(row)
    if spec['fixture'].startswith('workflow'):
        steps.append('scripts/ux-workflow-smoke.py on a disposable three-note vault with isolated XDG_CONFIG_HOME (embedded lattice): every route sends real keys/mouse escapes through a 120x40 PTY, asserts screen text, and checks disk state where a file is produced; a second session uses HTTP mode against a closed port for the lattice-error state.')
        artifacts += [f'ux-workflow-smoke/results.json', 'ux-workflow-smoke/workflows/terminal.ansi', 'ux-workflow-smoke/lattice-error/terminal.ansi', 'ux-workflow-smoke.log']
    else:
        steps.append('scripts/ux-input-smoke.py --large on a disposable clipboard fixture with isolated config: raw and bracketed LF/CRLF payloads in normal/insert/visual modes, undo/redo, command-like lines, read-only references, external editor with terminal-protocol restoration, and an exact 1 MiB bracketed paste; saved bytes are compared to expected content.')
        artifacts += ['ux-input-smoke.log']
    steps.append(f'Binary {binary} (sha256 {binary_sha}) built from {args.code_sha}; run on {args.host}.')
    native_done = [n for n in native if n.get('id', id_) == id_]
    for n in native_done:
        steps.append(f"Native row '{n['row']}' on {n['host']} ({n['terminal']}): {n['steps']} → {n['result']}")
    missing_native = [r for r in spec['native_rows'] if not any(n['row'].startswith(r.split(':')[0]) for n in native_done)]
    verdict = 'pass' if not failed_rows and not missing_native else 'pending'
    if failed_rows and results.get('workflow', {}).get('exit') not in (0, None) or failed_rows:
        verdict = 'fail'
    actual_rows = '; '.join(f"{row}: {'ok' if all(oks.values()) else 'FAIL ' + str([k for k, v in oks.items() if not v])}" for row, oks in row_results.items())
    limitations = [
        'PTY receipt is not native terminal rendering/display; screens are reconstructed from the terminal byte stream.',
        'Disposable synthetic fixtures; no real vault, no embedder.',
    ]
    if id_ == 'U2b':
        limitations.append('The transient "lattice search…" pending title is not asserted (it clears within the PTY poll interval); no-matches, error and results states are.')
    if missing_native:
        limitations.append('Native rows not covered by this run: ' + '; '.join(missing_native) + '. Supply them with --native from a person on that host.')
    measurements = []
    if spec['fixture'].startswith('workflow'):
        for row, (how, names) in spec['rows'].items():
            for n in names:
                ms = route_ms(n)
                if ms is not None:
                    measurements.append({'metric': f'route {n} wall time', 'value': ms, 'unit': 'ms (PTY, includes fixed key delays)', 'sample_count': 1, 'protocol': 'time from route start to its last assertion.'})
    record = {
        '$schema': '../../schema/ux/evidence.schema.json', 'id': id_, 'kind': spec['kind'], 'verdict': verdict,
        'timestamp': now, 'code_sha': args.code_sha, 'binary_sha256': binary_sha, 'host': args.host, 'os': os_text,
        'surface': spec['surface'], 'terminal': 'Automated PTY xterm-256color 120x40; no terminal renderer/display', 'backend': 'embedded',
        'fixture': {'id': spec['fixture'], 'manifest_sha256': fixture_manifest_sha(spec['fixture']), 'privacy': 'synthetic'},
        'steps': steps, 'expected': spec['expected'],
        'actual': f"{verdict.upper()}: {actual_rows}. Failed rows: {failed_rows or 'none'}." + (f" Native rows outstanding: {missing_native}." if missing_native else ''),
        'artifacts': [args.artifact_prefix + a for a in artifacts + [f'{id_}-{args.code_sha[:7]}-rows.json']],
        'limitations': limitations, 'blocker': None, 'measurements': measurements,
    }
    path = out / f'{id_}-{args.code_sha[:7]}-tui-evidence.json'
    path.write_text(json.dumps(record, indent=2, ensure_ascii=False) + '\n')
    # The row → route mapping stays next to the record as an artifact, outside the schema.
    (out / f'{id_}-{args.code_sha[:7]}-rows.json').write_text(json.dumps({'id': id_, 'rows': row_results}, indent=2) + '\n')
    written.append((id_, verdict, str(path)))

if args.schema:
    try:
        import jsonschema
        schema = json.loads(Path(args.schema).read_text())
        for _, _, p in written:
            jsonschema.validate(json.loads(Path(p).read_text()), schema)
        print('schema: all records valid')
    except ImportError:
        print('schema: jsonschema not installed; skipped')
for id_, verdict, p in written:
    print(f'{id_} {verdict} {p}')
