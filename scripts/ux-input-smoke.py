#!/usr/bin/env python3
"""Replay literal terminal input against a disposable vault; retain bytes and file results.

This supplements native clipboard smoke, not a replacement for it. No real vault is used.
"""
import argparse, fcntl, hashlib, json, os, pty, select, struct, termios, time
from pathlib import Path

ap = argparse.ArgumentParser()
ap.add_argument('--bin', required=True)
ap.add_argument('--out', required=True)
args = ap.parse_args()
out = Path(args.out).resolve()
out.mkdir(parents=True, exist_ok=True)
initial = '---\ntitle: Clipboard fixture\ncustom: preserve-me\n---\nanchor\nsecond line\n'
payload = 'first line\n    indented line\n\nlast line\n'

class Session:
    def __init__(self, name):
        self.root = out / name
        self.root.mkdir(exist_ok=True)
        self.vault = self.root / 'vault'
        self.vault.mkdir(exist_ok=True)
        self.note = self.vault / 'Clipboard.md'
        self.note.write_text(initial)
        config = self.root / 'config' / 'lapis'
        config.mkdir(parents=True, exist_ok=True)
        (config / 'config.toml').write_text('[lattice]\nmode = "embedded"\n')
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ['XDG_CONFIG_HOME'] = str(config.parent)
            os.environ['TERM'] = 'xterm-256color'
            os.environ.pop('LAPIS_VAULT', None)
            os.execv(args.bin, [args.bin, '--vault', str(self.vault), 'tui'])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack('HHHH', 32, 120, 0, 0))
        self.raw = bytearray()
        self.drain(0.6)
        self.send(b'\r')

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
        os.write(self.fd, data)
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
else:
    conflict_retained = False
results.append({'case':'dirty-conflict', 'dirty_quit_guard':stayed, 'external_save_guard':conflict_retained})
if stayed: s.close()

report = {'binary':str(Path(args.bin).resolve()), 'binary_sha256':hashlib.sha256(Path(args.bin).read_bytes()).hexdigest(),
          'initial_sha256':hashlib.sha256(initial.encode()).hexdigest(), 'results':results}
(out/'results.json').write_text(json.dumps(report, indent=2)+'\n')
print(json.dumps(report, indent=2))
