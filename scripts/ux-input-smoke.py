#!/usr/bin/env python3
"""Replay literal terminal input against a disposable vault; retain bytes and file results.

This supplements native clipboard smoke, not a replacement for it. No real vault is used.
"""
import argparse, base64, fcntl, hashlib, json, os, pty, re, select, struct, termios, time
from pathlib import Path

ap = argparse.ArgumentParser()
ap.add_argument('--bin', required=True)
ap.add_argument('--out', required=True)
ap.add_argument('--large', action='store_true', help='measure an exact 1 MiB bracketed paste and undo/redo')
ap.add_argument('--check', action='store_true', help='fail if a required supported input route fails')
args = ap.parse_args()
out = Path(args.out).resolve()
out.mkdir(parents=True, exist_ok=True)
initial = '---\ntitle: Clipboard fixture\ncustom: preserve-me\n---\nanchor\nsecond line\n'
payload = 'first line\n    indented line\n\nlast line\n'

class Session:
    def __init__(self, name, remote=False, editor=False):
        self.root = out / name
        self.root.mkdir(exist_ok=True)
        self.vault = self.root / 'vault'
        self.vault.mkdir(exist_ok=True)
        self.note = self.vault / 'Clipboard.md'
        self.note.write_text(initial)
        config = self.root / 'config' / 'lapis'
        config.mkdir(parents=True, exist_ok=True)
        (config / 'config.toml').write_text('[lattice]\nmode = "embedded"\n')
        self.editor_marker = self.root / 'editor-called'
        editor_program = self.root / 'fixture-editor.py'
        if editor:
            editor_program.write_text('#!/usr/bin/env python3\nfrom pathlib import Path\nimport sys\np=Path(sys.argv[1])\np.write_text(p.read_text()+"external editor\\n")\nPath('+repr(str(self.editor_marker))+').write_text("called")\n')
            editor_program.chmod(0o755)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ['XDG_CONFIG_HOME'] = str(config.parent)
            os.environ['TERM'] = 'xterm-256color'
            os.environ.pop('LAPIS_VAULT', None)
            if remote: os.environ['SSH_CONNECTION'] = 'fixture'
            if editor: os.environ['EDITOR'] = str(editor_program)
            os.execv(args.bin, [args.bin, '--vault', str(self.vault), 'tui'])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack('HHHH', 32, 120, 0, 0))
        os.set_blocking(self.fd, False)
        self.raw = bytearray()
        self.wait_for(b'Space leader')
        self.send(b'\r')
        self.wait_for(b'NORMAL')

    def wait_for(self, marker, timeout=15):
        deadline = time.monotonic() + timeout
        while marker not in self.raw:
            if time.monotonic() >= deadline:
                (self.root / 'startup-timeout.ansi').write_bytes(self.raw)
                self.close()
                raise RuntimeError(f'{self.root.name}: terminal never reached {marker!r}')
            self.drain(0.05)

    def drain(self, seconds=0.12):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            ready, _, _ = select.select([self.fd], [], [], max(0, end-time.monotonic()))
            if ready:
                try: data = os.read(self.fd, 65536)
                except OSError: return
                if not data: return
                self.raw.extend(data)

    def send(self, data, delay=0.12):
        pending = memoryview(data)
        deadline = time.monotonic() + 30
        while pending:
            if time.monotonic() > deadline: raise RuntimeError('PTY input timed out')
            readable, writable, _ = select.select([self.fd], [self.fd], [], 0.1)
            if readable:
                try: self.raw.extend(os.read(self.fd, 65536))
                except BlockingIOError: pass
            if writable:
                try: pending = pending[os.write(self.fd, pending):]
                except BlockingIOError: pass
        self.drain(delay)

    def saved(self):
        self.send(b'\x13', 0.25)
        return self.note.read_text()

    def close(self):
        self.send(b'\x1b')
        self.send(b'\x11')
        pid, _ = os.waitpid(self.pid, os.WNOHANG)
        if not pid:
            # Disposable process only; guards in a fixed build may keep dirty data open.
            os.kill(self.pid, 15)
            os.waitpid(self.pid, 0)
        os.close(self.fd)
        (self.root / 'terminal.ansi').write_bytes(self.raw)

results = []
for name, mode, data in [
    ('typed-save', b'i', b'typed '),
    ('paste-lf-insert', b'i', payload.encode()),
    ('paste-crlf-insert', b'i', payload.replace('\n', '\r\n').encode()),
    ('bracketed-insert', b'i', b'\x1b[200~'+payload.encode()+b'\x1b[201~'),
    ('bracketed-normal', b'', b'\x1b[200~'+payload.encode()+b'\x1b[201~'),
]:
    s = Session(name)
    if mode: s.send(mode)
    s.send(data, 0.4)
    saved = s.saved()
    body = saved.split('---\n', 2)[-1]
    expected = ('typed ' if name == 'typed-save' else payload) + 'anchor\nsecond line\n'
    result = {'case':name, 'body':body, 'expected':expected, 'content_matches':body==expected,
              'custom_preserved':'custom: preserve-me' in saved,
              'bracketed_enabled':b'\x1b[?2004h' in s.raw}
    s.close()
    results.append(result)
# A selection replacement must undo as a single transaction, including after
# an explicit save and the resulting filesystem watcher notification.
for name, selection in [('paste-undo', b''), ('visual-paste-undo', b'vlll')]:
    s = Session(name)
    if selection: s.send(selection)
    s.send(b'\x1b[200~'+payload.encode()+b'\x1b[201~', 0.4)
    after = s.saved().split('---\n', 2)[-1]
    expected = payload + ('hor\nsecond line\n' if selection else 'anchor\nsecond line\n')
    s.send(b'u')
    undone = s.saved().split('---\n', 2)[-1]
    s.send(b'\x12')
    redone = s.saved().split('---\n', 2)[-1]
    results.append({'case':name, 'content_matches':after==expected,
                    'undo_matches':undone=='anchor\nsecond line\n', 'redo_matches':redone==expected})
    s.close()

# Real crossterm mouse packets through a PTY, measured against the rendered
# 120x32 layout. This checks app dispatch, not the user's native pointer route.
s = Session('mouse-replace-undo')
s.send(b'\x1b[<0;34;3M')
s.send(b'\x1b[<32;40;3M')
s.send(b'\x1b[<0;40;3m')
s.send(b'\x1b[200~'+payload.encode()+b'\x1b[201~', 0.4)
after = s.saved().split('---\n', 2)[-1]
s.send(b'u')
undone = s.saved().split('---\n', 2)[-1]
results.append({'case':'mouse-replace-undo', 'body':after,
                'content_matches':after==payload+'\nsecond line\n',
                'undo_matches':undone=='anchor\nsecond line\n'})
s.close()

# Preview text is copied through OSC 52 in a simulated SSH environment;
# this verifies dispatch/content without overwriting the runner's OS clipboard.
s = Session('preview-copy', remote=True)
s.send(b'\x1b[<0;77;3M')
s.send(b'\x1b[<32;83;3M')
s.send(b'\x1b[<0;83;3m')
s.send(b'y')
copies = re.findall(rb'\x1b\]52;c;([^\x07]*)\x07', s.raw)
results.append({'case':'preview-copy', 'content_matches':bool(copies) and base64.b64decode(copies[-1]) == b'anchor'})
s.close()

if args.large:
    s = Session('large-paste')
    unit = 'line\t漢字 ' + 'x' * (64 - len('line\t漢字 \n'.encode())) + '\n'
    large = unit * (1024 * 1024 // 64)
    assert len(large.encode()) == 1024 * 1024
    expected = large + 'anchor\nsecond line\n'
    body_of = lambda text: text.split('---\n', 2)[-1]
    start = time.monotonic()
    s.send(b'\x1b[200~' + large.encode() + b'\x1b[201~', 0)
    delivered = time.monotonic()
    s.send(b'\x13', 0)
    deadline = time.monotonic() + 30
    while body_of(s.note.read_text()) != expected and time.monotonic() < deadline: s.drain(0.005)
    integrated_saved = time.monotonic()
    after = s.note.read_text()
    s.send(b'u', 0.1)
    undone = s.saved()
    s.send(b'\x12', 0.1)
    redone = s.saved()
    results.append({'case':'large-paste', 'bytes':len(large.encode()),
                    'payload_sha256':hashlib.sha256(large.encode()).hexdigest(),
                    'delivery_ms':1000*(delivered-start),
                    'paste_and_save_upper_bound_ms':1000*(integrated_saved-start),
                    'content_matches':body_of(after)==expected,
                    'custom_preserved':all('custom: preserve-me' in text for text in (after, undone, redone)),
                    'undo_matches':body_of(undone)=='anchor\nsecond line\n',
                    'redo_matches':body_of(redone)==expected})
    s.close()

# Tiny windows keep buffers intact and never accept invisible text edits.
s = Session('undersized-input')
fcntl.ioctl(s.fd, termios.TIOCSWINSZ, struct.pack('HHHH',10,40,0,0))
s.drain(0.2)
s.send(b'iHIDDEN')
s.send(b'\x1b[200~invisible paste\x1b[201~')
fcntl.ioctl(s.fd, termios.TIOCSWINSZ, struct.pack('HHHH',32,120,0,0))
s.drain(0.2)
s.send(b'\x1b')
after = s.saved()
results.append({'case':'undersized-input', 'content_matches':after.split('---\n',2)[-1]=='anchor\nsecond line\n'})
s.close()

# Destructive navigation must not discard an unsaved tab.
s = Session('dirty-trash-close')
s.send(b'iunsaved')
s.send(b'\x1b')
s.send(b' x')
trash_guard = s.note.exists() and b'before trashing' in s.raw
s.send(b':q\r')
saved = s.saved()
results.append({'case':'dirty-trash-close', 'dirty_trash_guard':trash_guard,
                'dirty_close_guard':saved.split('---\n',2)[-1]=='unsavedanchor\nsecond line\n'})
s.close()

# The external editor is an isolated fixture program. Verify the dirty guard,
# successful return/reload, and restored terminal protocols.
s = Session('external-editor', editor=True)
s.send(b'iunsaved')
s.send(b'\x1b')
s.send(b' le')
blocked_dirty = not s.editor_marker.exists()
s.saved()
s.send(b' le', 0.6)
after_external = s.note.read_text()
s.send(b'iX')
s.send(b'\x1b')
after_local = s.saved()
results.append({'case':'external-editor', 'dirty_guard':blocked_dirty,
                'editor_ran':s.editor_marker.exists(),
                'reload_preserved_external_edit':'external editor\n' in after_local,
                'local_edit_retained':'X' in after_local and after_local.replace('X','',1)==after_external,
                'bracketed_restored':s.raw.count(b'\x1b[?2004h')>=2,
                'mouse_restored':s.raw.count(b'\x1b[?1000h')>=2})
s.close()

# Quit and save must retain dirty content when an agent changes the same file.
s = Session('dirty-conflict')
s.send(b'iunsaved')
s.send(b'\x1b')
s.send(b'\x11')
stayed = os.waitpid(s.pid, os.WNOHANG)[0] == 0
if stayed:
    external = initial.replace('anchor', 'external')
    s.note.write_text(external)
    s.drain(0.2)
    s.saved()
    conflict_retained = s.note.read_text() == external
    s.send(b' lS', 0.4)
    copies = list(s.vault.glob('* (Lapis copy *).md'))
    copy_retained = bool(copies) and 'unsavedanchor' in copies[0].read_text() and 'custom: preserve-me' in copies[0].read_text()
    conflict_retained = conflict_retained and s.note.read_text() == external
else:
    conflict_retained = False
    copy_retained = False
results.append({'case':'dirty-conflict', 'dirty_quit_guard':stayed, 'external_save_guard':conflict_retained, 'conflict_copy_preserved':copy_retained})
if stayed: s.close()

report = {'binary':str(Path(args.bin).resolve()), 'binary_sha256':hashlib.sha256(Path(args.bin).read_bytes()).hexdigest(),
          'initial_sha256':hashlib.sha256(initial.encode()).hexdigest(), 'results':results}
(out/'results.json').write_text(json.dumps(report, indent=2)+'\n')
print(json.dumps(report, indent=2))
if args.check:
    required = [r for r in results if r['case'] not in ('paste-lf-insert', 'paste-crlf-insert')]
    if any(value is False for result in required for value in result.values()):
        raise SystemExit('required TUI input regression failed; see retained results')
