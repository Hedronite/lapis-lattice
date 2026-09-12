#!/usr/bin/env python3
"""Missing-index first run through the TUI: files stay usable, setup is explicit and
builds in the background, and health afterwards is built rather than silently empty.

Uses a disposable synthetic vault (or a caller-supplied copy with no index) and an
isolated XDG_CONFIG_HOME. Never runs `lapis init` and never edits notes.
"""
import argparse, fcntl, hashlib, json, os, pty, select, struct, subprocess, termios, time
from pathlib import Path
from ux_terminal import screen_text

WIDTH, HEIGHT = 120, 40
ap = argparse.ArgumentParser(description=__doc__)
ap.add_argument('--bin', required=True)
ap.add_argument('--out', required=True)
ap.add_argument('--vault', help='existing Collection-NNN/Note-NNNNN.md vault without .lapis (default: synthetic)')
ap.add_argument('--notes', type=int, default=1500)
ap.add_argument('--timeout', type=float, default=600, help='seconds allowed for the build itself')
ap.add_argument('--check', action='store_true', help='exit non-zero when any expectation fails')
args = ap.parse_args()
out = Path(args.out).resolve()
out.mkdir(parents=True, exist_ok=True)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def corpus(root):
    return {str(p.relative_to(root)): digest(p) for p in sorted(root.rglob('*'))
            if p.is_file() and '.lapis' not in p.relative_to(root).parts}


vault = Path(args.vault).resolve() if args.vault else out / 'vault'
if not args.vault:
    for i in range(args.notes):
        note = vault / f'Collection-{i // 100:03d}' / f'Note-{i:05d}.md'
        note.parent.mkdir(parents=True, exist_ok=True)
        note.write_text(f'---\ntitle: Note {i}\n---\n# Note {i}\n\nSetup fixture body. '
                        f'See [[Note-{(i + 1) % args.notes:05d}]].\n')
if (vault / '.lapis' / 'lattice.sqlite').exists():
    ap.error('the vault already has an index; the missing-index route needs none')
before = corpus(vault)
markdown = sum(1 for rel in before if rel.endswith('.md'))
config = out / 'config'
(config / 'lapis').mkdir(parents=True, exist_ok=True)
(config / 'lapis' / 'config.toml').write_text('[lattice]\nmode = "embedded"\n')
env = dict(os.environ, XDG_CONFIG_HOME=str(config), TERM='xterm-256color')
for key in ('LAPIS_VAULT', 'LAPIS_LATTICE_URL'):
    env.pop(key, None)

pid, fd = pty.fork()
if pid == 0:
    os.execve(args.bin, [args.bin, '--vault', str(vault), 'tui'], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', HEIGHT, WIDTH, 0, 0))
os.set_blocking(fd, False)
raw = bytearray()
started = time.monotonic()
marks, failures, progress = {}, [], []


def drain(seconds):
    end = time.monotonic() + seconds
    while (left := end - time.monotonic()) > 0:
        ready, _, _ = select.select([fd], [], [], left)
        if not ready:
            continue
        try:
            data = os.read(fd, 65536)
        except BlockingIOError:
            continue
        except OSError:
            return
        if not data:
            return
        raw.extend(data)


def screen():
    return screen_text(raw, WIDTH, HEIGHT)


def status_line(text):
    return text.splitlines()[HEIGHT - 1] if len(text.splitlines()) >= HEIGHT else ''


def wait(name, predicate, timeout=20, watch=None):
    deadline = time.monotonic() + timeout
    while True:
        text = screen()
        if watch:
            watch(text)
        if predicate(text):
            marks[name] = (time.monotonic() - started) * 1000
            (out / f'{name}.txt').write_text(text)
            return text
        if time.monotonic() > deadline:
            (out / f'{name}-timeout.txt').write_text(text)
            raise TimeoutError(f'timed out waiting for {name}')
        drain(0.02)


def send(data):
    os.write(fd, data)
    drain(0.1)


def record_progress(text):
    line = status_line(text)
    if 'indexing' in line and (not progress or progress[-1]['status'] != line.strip()):
        progress.append({'ms': (time.monotonic() - started) * 1000, 'status': line.strip()})


try:
    wait('setup-offered', lambda t: 'no search index · Space i builds' in status_line(t))
    # Files remain usable before any index exists: expand the first folder, open its first note.
    send(b'\r')
    wait('folder-open', lambda t: 'Note-' in t)
    send(b'j\r')
    wait('editor-open', lambda t: 'NORMAL' in status_line(t))
    # A search with no index offers setup instead of looking like a real zero-result query.
    send(b'\x10')
    for ch in b'Note':
        send(bytes([ch]))
    wait('search-offers-setup', lambda t: 'search index not built' in t and 'Space i' in t)
    send(b'\x1b')
    # Explicit action; the build runs in the background.
    send(b' ')
    send(b'i')
    wait('indexing-shown', lambda t: 'indexing' in status_line(t), watch=record_progress)
    # Input is still served during the build: the cursor moves down a line.
    send(b'j')
    wait('ready', lambda t: 'search is ready' in status_line(t), timeout=args.timeout, watch=record_progress)
    after = screen()
    if 'no search index' in status_line(after) or 'index failed' in status_line(after):
        failures.append('setup indicator still shown after the build')
    send(b'\x1b')
    send(b'\x11')
except TimeoutError as error:
    failures.append(str(error))
finally:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        done, _ = os.waitpid(pid, os.WNOHANG)
        if done:
            break
        drain(0.1)
    else:
        os.kill(pid, 15)
        os.waitpid(pid, 0)
        failures.append('TUI did not exit after Ctrl+Q')
    os.close(fd)
    (out / 'terminal.ansi').write_bytes(raw)

probe = subprocess.run([args.bin, '--vault', str(vault), '--json', 'vault', 'info'],
                       env=env, text=True, capture_output=True)
(out / 'index-health.json').write_text(probe.stdout + probe.stderr)
health = None
try:
    health = json.loads(probe.stdout)['data']['lattice']['health']
except (ValueError, KeyError, TypeError):
    failures.append(f'vault info did not report health (exit {probe.returncode})')
if health:
    if not health['graph']['built'] or health['status'] != 'ok':
        failures.append(f"health not built/ok: {health['status']}, built={health['graph']['built']}")
    if health['documents_indexed'] != markdown:
        failures.append(f"documents_indexed {health['documents_indexed']} != {markdown} notes")
if not progress:
    failures.append('no indexing progress was displayed')
after_files = corpus(vault)
if after_files != before:
    changed = sorted(set(before) ^ set(after_files) | {k for k in before if after_files.get(k) != before[k]})
    failures.append(f'vault files changed outside .lapis: {changed[:5]}')

result = {'binary_sha256': digest(Path(args.bin)), 'vault': 'synthetic' if not args.vault else str(vault),
          'notes': markdown, 'terminal': f'PTY xterm-256color {WIDTH}x{HEIGHT}; no terminal renderer',
          'pty_ms': marks, 'progress_seen': progress, 'health_after': health,
          'files_unchanged': after_files == before, 'init_run': False, 'failures': failures}
(out / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps({k: v for k, v in result.items() if k != 'progress_seen'} | {'progress_updates': len(progress)}, indent=2))
if args.check and failures:
    raise SystemExit('missing-index TUI regression failed: ' + '; '.join(failures))
