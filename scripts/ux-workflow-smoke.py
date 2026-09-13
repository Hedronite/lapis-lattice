#!/usr/bin/env python3
"""Drive every TUI workflow route through a PTY on a disposable vault: outline,
palette states (results / no matches / commands / lattice error), back/forward
history, tasks/kanban/calendar, daily, new note, template, quick capture, tags,
HAL, neighbors, trash/restore, external editor and help. Each route records what
it saw; `--check` fails on any route failure. Never touches a real vault.
"""
import argparse, datetime, fcntl, json, os, pty, re, select, struct, subprocess, termios, time
from pathlib import Path
from ux_terminal import screen_text

W, H = 120, 40
ap = argparse.ArgumentParser(description=__doc__)
ap.add_argument('--bin', required=True)
ap.add_argument('--out', required=True)
ap.add_argument('--check', action='store_true')
args = ap.parse_args()
out = Path(args.out).resolve(); out.mkdir(parents=True, exist_ok=True)
ALPHA = '---\ntitle: Alpha\ntags: [alpha, shared]\n---\n# Alpha\n\nSee [[Beta]].\n\n## Alpha tasks\n\n- [ ] first task\n- [x] done task\n\n## Alpha end\n\ntext\n'
BETA = '---\ntitle: Beta\ntags: [beta, shared]\n---\n# Beta\n\n## Section two\n\n### Deep\n\nbody [[Alpha]]\n'
GAMMA = '# Gamma\n\nlinks [[Alpha]]\n'


class Session:
    def __init__(self, name, lattice='[lattice]\nmode = "embedded"\n', editor=False):
        self.name = name
        self.root = out / name; self.vault = self.root / 'vault'
        (self.vault / 'notes').mkdir(parents=True, exist_ok=True)
        (self.vault / 'notes' / 'Alpha.md').write_text(ALPHA)
        (self.vault / 'notes' / 'Beta.md').write_text(BETA)
        (self.vault / 'notes' / 'Gamma.md').write_text(GAMMA)
        config = self.root / 'config' / 'lapis'; config.mkdir(parents=True, exist_ok=True)
        (config / 'config.toml').write_text(lattice)
        self.marker = self.root / 'editor-called'
        program = self.root / 'fixture-editor.py'
        program.write_text('#!/usr/bin/env python3\nfrom pathlib import Path\nimport sys\np=Path(sys.argv[1])\np.write_text(p.read_text()+"external editor\\n")\nPath(' + repr(str(self.marker)) + ').write_text("called")\n')
        program.chmod(0o755)
        env = dict(os.environ, XDG_CONFIG_HOME=str(config.parent), TERM='xterm-256color')
        for k in ('LAPIS_VAULT', 'LAPIS_LATTICE_URL'):
            env.pop(k, None)
        if editor:
            env['EDITOR'] = str(program)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.execve(args.bin, [args.bin, '--vault', str(self.vault), 'tui'], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack('HHHH', H, W, 0, 0))
        os.set_blocking(self.fd, False)
        self.raw = bytearray(); self.routes = []; self.alive = True
        self.wait('startup', lambda t: 'Space leader' in t)

    def drain(self, seconds):
        end = time.monotonic() + seconds
        while (left := end - time.monotonic()) > 0:
            r, _, _ = select.select([self.fd], [], [], left)
            if not r:
                continue
            try:
                data = os.read(self.fd, 65536)
            except BlockingIOError:
                continue
            except OSError:
                self.alive = False; return
            if not data:
                self.alive = False; return
            self.raw.extend(data)

    def screen(self):
        return screen_text(self.raw, W, H)

    def status(self):
        lines = self.screen().splitlines()
        return lines[H - 1] if len(lines) >= H else ''

    def send(self, data, delay=0.15):
        os.write(self.fd, data); self.drain(delay)

    def keys(self, *ks):
        for k in ks:
            self.send(k)

    def wait(self, name, pred, timeout=15):
        deadline = time.monotonic() + timeout
        while True:
            t = self.screen()
            if pred(t):
                return t
            if time.monotonic() > deadline or not self.alive:
                (self.root / f'{name}-timeout.txt').write_text(t)
                raise TimeoutError(f'{name}: not seen within {timeout}s; last status {self.status().strip()!r}')
            self.drain(0.03)

    def poll(self, name, pred, timeout=5):
        deadline = time.monotonic() + timeout
        while not pred():
            if time.monotonic() > deadline:
                raise TimeoutError(f'{name}: disk state not reached within {timeout}s')
            self.drain(0.05)

    def route(self, name, fn):
        started = time.monotonic()
        try:
            detail = fn() or {}
            self.routes.append({'route': name, 'ok': True, 'ms': (time.monotonic() - started) * 1000, **detail})
        except Exception as e:  # noqa: BLE001 - each route is recorded, then the next one runs
            self.routes.append({'route': name, 'ok': False, 'error': str(e)[:300]})
            (self.root / f'{name}-screen.txt').write_text(self.screen())
            self.keys(b'\x1b', b'\x1b')

    def close(self):
        self.keys(b'\x1b', b'\x1b', b'\x11')
        self.drain(0.5)
        if 'unsaved changes' in self.status():
            # A route left a dirty buffer; discard it explicitly, then quit.
            self.routes.append({'route': 'quit', 'ok': False, 'error': 'dirty buffer at quit: ' + self.status().strip()[:120]})
            self.send(b':q!\r', 0.5); self.send(b'\x11', 0.5)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            done, _ = os.waitpid(self.pid, os.WNOHANG)
            if done:
                break
            self.drain(0.1)
        else:
            self.send(b':q!\r'); self.drain(1)
            done, _ = os.waitpid(self.pid, os.WNOHANG)
            if not done:
                os.kill(self.pid, 15); os.waitpid(self.pid, 0)
                self.routes.append({'route': 'quit', 'ok': False, 'error': 'Ctrl+Q did not exit'})
        os.close(self.fd)
        (self.root / 'terminal.ansi').write_bytes(self.raw)


def open_note(s, name):
    s.keys(b'\x10')
    s.send(name.encode(), 0.4)
    s.wait(f'palette-{name}', lambda t: f'notes/{name}.md' in t)
    s.send(b'\r', 0.3)
    s.wait(f'open-{name}', lambda t: f'notes/{name}.md' in s.status() and 'NORMAL' in s.status())


s = Session('workflows', editor=True)
s.keys(b'\r'); s.wait('folder', lambda t: 'Alpha.md' in t)
s.keys(b'j', b'\r'); s.wait('editor', lambda t: 'NORMAL' in s.status())


def r_index():
    s.keys(b' ', b'i'); s.wait('index', lambda t: 'search is ready' in s.status(), 60)


def r_outline():
    s.keys(b' ', b'p'); t = s.wait('outline', lambda t: ' outline ' in t and 'Alpha tasks' in t)
    s.keys(b'j', b'\r'); s.wait('outline-jump', lambda t: re.search(r'\s[2-9]\d*:\d+', s.status()) is not None)
    return {'cursor': re.search(r'\s(\d+:\d+)', s.status()).group(1)}


def r_history():
    open_note(s, 'Beta')
    s.keys(b' ', b'b', b'h'); s.wait('back', lambda t: 'notes/Alpha.md' in s.status())
    s.send(b'\x1b[1;3C'); s.wait('forward-alt-right', lambda t: 'notes/Beta.md' in s.status())
    s.send(b'\x0f'); s.wait('back-ctrl-o', lambda t: 'notes/Alpha.md' in s.status())
    s.keys(b' ', b'b', b'l'); s.wait('forward-leader', lambda t: 'notes/Beta.md' in s.status())
    s.keys(b' ', b'b', b'l'); s.wait('forward-end', lambda t: 'no newer visit' in s.status())


def r_palette_states():
    s.keys(b'\x10'); s.send(b'zzqqxx', 0.4); s.wait('no-matches', lambda t: 'no matches' in t); s.keys(b'\x1b')
    s.keys(b'\x10'); s.send(b'>kanban', 0.3); s.wait('commands', lambda t: ' commands ' in t and 'kanban board' in t); s.keys(b'\x1b')
    s.keys(b'\x10'); s.send(b'Alpha', 0.4); s.wait('results', lambda t: 'notes/Alpha.md' in t); s.keys(b'\x1b')


def r_buffers():
    s.keys(b' ', b'o'); s.wait('buffers', lambda t: ' buffers ' in t and 'Beta.md' in t); s.keys(b'\x1b')


def r_tasks():
    # The list opens as a per-folder summary; Enter drills into the folder rows.
    s.keys(b' ', b't', b'l'); s.wait('tasks', lambda t: 'tasks' in t and ' by folder ' in t and 'notes' in t)
    s.send(b'\r', 0.3); s.wait('tasks-rows', lambda t: 'first task' in t)
    s.keys(b' ', b'k'); s.wait('kanban', lambda t: 'kanban' in t and 'open' in t)
    s.keys(b' ', b'c'); s.wait('calendar', lambda t: 'calendar' in t and 'Mo  Tu' in t)
    s.keys(b' ', b't', b'n'); s.wait('notes-view', lambda t: 'NORMAL' in s.status() or 'FILES' in s.status())


def r_daily():
    today = datetime.date.today().isoformat()
    s.keys(b' ', b'd'); s.wait('daily', lambda t: f'Daily/{today}.md' in s.status())
    assert (s.vault / 'Daily' / f'{today}.md').exists(), 'daily file missing'
    return {'path': f'Daily/{today}.md'}


def r_new_note():
    s.keys(b' ', b'n', b'n'); s.wait('new-prompt', lambda t: 'New note title' in t)
    s.send(b'Fresh note', 0.2); s.send(b'\r', 0.4)
    s.poll('new-file', lambda: any('fresh' in p.name.lower() for p in s.vault.rglob('*.md')))
    # A new note opens ready to type (INSERT); leave insert mode before the next leader chord.
    s.wait('new-note-insert', lambda t: 'INSERT' in s.status()); s.keys(b'\x1b')
    return {'files': [str(p.relative_to(s.vault)) for p in s.vault.rglob('*.md') if 'fresh' in p.name.lower()]}


def r_template():
    s.keys(b' ', b'n', b't'); s.wait('templates', lambda t: ' new from template ' in t)
    s.send(b'\r', 0.3); s.wait('template-prompt', lambda t: 'title' in t.lower())
    s.send(b'Templated', 0.2); s.send(b'\r', 0.4)
    s.poll('template-file', lambda: any('templated' in p.name.lower() for p in s.vault.rglob('*.md')))
    s.drain(0.3)
    if 'INSERT' in s.status():
        s.keys(b'\x1b')
    return {'files': [str(p.relative_to(s.vault)) for p in s.vault.rglob('*.md') if 'templated' in p.name.lower()]}


def r_capture():
    s.keys(b' ', b'q'); s.wait('capture-prompt', lambda t: 'Quick capture' in t)
    s.send(b'captured line', 0.2); s.send(b'\r', 0.4)
    # The capture confirmation is followed within a frame by the re-index status for
    # the same inbox path; either names the new note.
    s.wait('captured', lambda t: 'captured ->' in s.status() or re.search(r'inbox/\S+: indexed', s.status()) is not None)
    s.poll('capture-file', lambda: any('captured line' in p.read_text() for p in s.vault.rglob('*.md')))
    return {'files': [str(p.relative_to(s.vault)) for p in s.vault.rglob('*.md') if 'captured line' in p.read_text()]}


def r_tags():
    s.keys(b' ', b'#'); s.wait('tags', lambda t: ' tags ' in t and 'shared' in t)
    s.send(b'sha', 0.3); s.wait('tags-filter', lambda t: ' tags  /sha ' in t)
    s.send(b'\r', 0.3); s.wait('tag-drill', lambda t: ' #shared ' in t and 'Alpha' in t)
    s.keys(b'\x1b', b'\x1b')


def r_hal():
    open_note(s, 'Alpha')
    s.keys(b' ', b'y'); s.wait('hal', lambda t: ' HAL ' in t and 'tags' in t)
    s.keys(b' ', b'y'); s.wait('hal-off', lambda t: ' HAL ' not in t)


def r_neighbors():
    s.keys(b' ', b'g'); s.wait('neighbors', lambda t: ' hop-1 neighbors' in t and 'Beta' in t)
    s.keys(b' ', b'g'); s.wait('neighbors-off', lambda t: ' hop-1 neighbors' not in t)


def r_trash_restore():
    open_note(s, 'Gamma')
    s.keys(b' ', b'x')
    s.poll('trashed', lambda: not (s.vault / 'notes' / 'Gamma.md').exists())
    trashed = [str(p.relative_to(s.vault)) for p in (s.vault / '.lapis' / 'trash').rglob('*.md')]
    assert trashed, 'trash bucket has no note'
    s.keys(b' ', b'l', b'r'); s.wait('restore-picker', lambda t: ' trash  (Enter restores) ' in t)
    s.send(b'\r', 0.4)
    s.poll('restored', lambda: (s.vault / 'notes' / 'Gamma.md').exists())
    return {'trashed_as': trashed}


def r_external_editor():
    open_note(s, 'Alpha')
    s.keys(b' ', b'l'); s.send(b'e', 1.0)
    s.poll('editor-ran', lambda: s.marker.exists())
    s.wait('reloaded', lambda t: 'external editor' in t, 10)


def r_help():
    s.send(b'?'); s.wait('help', lambda t: ' help ' in t); s.keys(b'\x1b')


def click(x, y):
    # SGR mouse press + release at 1-based terminal coordinates.
    s.send(f'\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m'.encode(), 0.3)


def r_mouse_parity():
    open_note(s, 'Alpha')
    # Keyboard: Shift+Tab cycles focus back to the sidebar; mouse reaches the same panes by click.
    s.send(b'\x1b[Z', 0.3); s.wait('backtab-to-sidebar', lambda t: s.status().lstrip().startswith('FILES'))
    # Sidebar rows (list starts under the border): notes, Alpha, Beta, Gamma once expanded.
    rows = s.screen().splitlines()
    beta_row = next(i for i, line in enumerate(rows) if line[:28].strip().endswith('Beta.md'))
    # First click selects the row, the second opens it (folders toggle the same way).
    click(6, beta_row + 1)
    s.wait('click-selects-beta', lambda t: s.status().lstrip().startswith('FILES') and 'notes/Alpha.md' in s.status())
    click(6, beta_row + 1)
    # Opening by click keeps the sidebar focused, like Enter does.
    s.wait('click-opens-beta', lambda t: 'notes/Beta.md' in s.status())
    # Tab bar: clicking the first tab switches back to Alpha; Alt+1 does the same by keyboard.
    tabs_line = s.screen().splitlines()[0]
    alpha_x = tabs_line.index('Alpha.md') + 1
    click(alpha_x, 1)
    s.wait('click-tab-alpha', lambda t: 'notes/Alpha.md' in s.status())
    s.send(b'\x1b2', 0.3); s.wait('alt-2-beta', lambda t: 'notes/Beta.md' in s.status())
    # Preview and editor panes take focus by click; Tab reaches them by keyboard.
    click(100, 10); s.wait('click-preview', lambda t: s.status().lstrip().startswith('PREVIEW'))
    click(45, 10); s.wait('click-editor', lambda t: s.status().lstrip().startswith('NORMAL'))
    return {'beta_row': beta_row, 'alpha_tab_x': alpha_x}


for name, fn in [('index', r_index), ('outline', r_outline), ('history', r_history), ('palette-states', r_palette_states),
                 ('buffers', r_buffers), ('tasks', r_tasks), ('daily', r_daily), ('new-note', r_new_note),
                 ('template', r_template), ('capture', r_capture), ('tags', r_tags), ('hal', r_hal),
                 ('neighbors', r_neighbors), ('trash-restore', r_trash_restore), ('external-editor', r_external_editor),
                 ('help', r_help), ('mouse-parity', r_mouse_parity)]:
    s.route(name, fn)
s.close()

e = Session('lattice-error', lattice='[lattice]\nmode = "http"\nurl = "http://127.0.0.1:9"\n')


def r_error():
    e.keys(b'\r'); e.keys(b'\x10'); e.send(b'Alpha', 0.4)
    e.wait('error-state', lambda t: 'search failed:' in t and 'Enter retries' in t)
    e.send(b'\r', 1.2); e.wait('error-after-retry', lambda t: 'search failed:' in t)
    # Editing the query asks again; with the service still down it fails again, but the
    # failure names the service rather than pretending the query has no matches.
    e.send(b'x', 0.6); e.wait('error-after-edit', lambda t: 'search failed:' in t and 'no matches' not in t)
    e.keys(b'\x1b'); e.wait('error-closed', lambda t: 'search failed:' not in t)


e.route('lattice-error', r_error)
e.close()

report = {'binary': args.bin, 'routes': s.routes + e.routes}
failed = [r['route'] for r in report['routes'] if not r['ok']]
report['failed'] = failed
(out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
if args.check and failed:
    raise SystemExit('workflow routes failed: ' + ', '.join(failed))
