#!/usr/bin/env python3
"""Missing-index first run through the TUI: files stay usable, setup is explicit and
builds in the background, and health afterwards is built rather than silently empty.

Uses a disposable synthetic vault (or a caller-supplied copy with no index) and an
isolated XDG_CONFIG_HOME. Never runs `lapis init` and never edits notes on its own;
`--save-during-build` edits exactly one note through the editor.

Optional routes (both opt-in, used for evidence runs on the standard fixture):
  --fault              make `.lapis` unwritable before the first Space i, expect an
                       explicit failure with a retry hint, restore, retry.
  --save-during-build  insert a marker into the open note and Ctrl+S while the build
                       runs; expect the note to be re-indexed after the build.
"""
import argparse, fcntl, hashlib, json, os, pty, re, select, stat, struct, subprocess, termios, time
from pathlib import Path
from ux_terminal import screen_text

WIDTH, HEIGHT = 120, 40
MARKER = 's1bsavemarker'
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--bin', required=True)
ap.add_argument('--out', required=True)
ap.add_argument('--vault', help='existing Collection-NNN/Note-NNNNN.md vault without .lapis (default: synthetic)')
ap.add_argument('--notes', type=int, default=1500)
ap.add_argument('--timeout', type=float, default=600, help='seconds allowed for the build itself')
ap.add_argument('--fault', action='store_true', help='inject an unwritable .lapis for the first build')
ap.add_argument('--save-during-build', action='store_true', help='save the open note while the build runs')
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
first_note = vault / 'Collection-000' / 'Note-00000.md'
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
marks, failures, progress, notes = {}, [], [], {}


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


def send(data, delay=0.1):
    os.write(fd, data)
    drain(delay)


def record_progress(text):
    line = status_line(text)
    if re.search(r'indexing \d+/\d+', line) and (not progress or progress[-1]['status'] != line.strip()):
        progress.append({'ms': (time.monotonic() - started) * 1000, 'status': line.strip()})


def build_done(text):
    line = status_line(text)
    return 'search is ready' in line or ': indexed (' in line


lapis_dir = vault / '.lapis'
locked = []


def lock_index():
    # The engine opened the (empty) database at startup; take away write access to the
    # directory and every file in it so the build's DELETE/INSERT and WAL fail.
    for path in [lapis_dir, *lapis_dir.iterdir()]:
        mode = stat.S_IMODE(path.stat().st_mode)
        locked.append((path, mode))
        path.chmod(0o500 if path.is_dir() else 0o400)


def unlock_index():
    for path, mode in reversed(locked):
        path.chmod(mode)
    locked.clear()


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
    if args.fault:
        lock_index()
        try:
            send(b' ')
            send(b'i')
            text = wait('build-failed', lambda t: 'build failed' in status_line(t) or build_done(t), timeout=60)
            if build_done(text):
                notes['fault'] = 'ineffective: the build succeeded with .lapis read-only (root or permissive filesystem)'
            else:
                notes['fault'] = status_line(text).strip()
                if 'Space i retries' not in status_line(text):
                    failures.append('failed build did not show the retry hint')
                # The hint persists after the transient status expires, until the retry.
                wait('retry-hint', lambda t: 'index failed · Space i retries' in status_line(t), timeout=15)
        finally:
            unlock_index()
    # Explicit action; the build runs in the background.
    send(b' ')
    send(b'i')
    wait('indexing-shown', lambda t: 'indexing' in status_line(t), watch=record_progress)
    if args.save_during_build:
        # Insert a marker at the top of the open note and save while the build runs.
        stat_before = first_note.stat()
        send(b'i')
        send(MARKER.encode() + b' ', 0.3)
        send(b'\x1b')
        at_save = status_line(screen()).strip()
        notes['status_at_save'] = at_save
        if not re.search(r'indexing', at_save):
            failures.append(f'save was not sent during the build: {at_save!r}')
        send(b'\x13', 0)
        saved = time.monotonic()
        while time.monotonic() - saved < 5:
            current = first_note.stat()
            if (current.st_ino, current.st_mtime_ns) != (stat_before.st_ino, stat_before.st_mtime_ns):
                marks['saved-during-build'] = (time.monotonic() - started) * 1000
                break
            drain(0.02)
        else:
            failures.append('note was not saved during the build')
    else:
        # Input is still served during the build: the cursor moves down a line.
        send(b'j')
    wait('ready', build_done, timeout=args.timeout, watch=record_progress)
    if args.save_during_build:
        # The path saved mid-build is re-indexed once the build lands.
        wait('queued-reindexed', lambda t: 'Note-00000.md: indexed (' in status_line(t), timeout=60)
    after = screen()
    if 'no search index' in status_line(after) or 'index failed' in status_line(after):
        failures.append('setup indicator still shown after the build')
    send(b'\x1b')
    send(b'\x11')
except TimeoutError as error:
    failures.append(str(error))
finally:
    unlock_index()
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
    failures.append('no files/total indexing progress was displayed')
marker_hit = None
if args.save_during_build:
    search = subprocess.run([args.bin, '--vault', str(vault), '--json', 'search', MARKER, '--embedder', 'none'],
                            env=env, text=True, capture_output=True)
    (out / 'marker-search.json').write_text(search.stdout + search.stderr)
    paths = re.findall(r'"path":\s*"([^"]+)"', search.stdout)
    marker_hit = 'Collection-000/Note-00000.md' in paths
    if not marker_hit:
        failures.append(f'marker saved during the build is not indexed afterwards (hits: {paths[:3]})')
    if MARKER not in first_note.read_text():
        failures.append('marker missing from the saved note')
after_files = corpus(vault)
changed = sorted(set(before) ^ set(after_files) | {k for k in before if after_files.get(k) != before[k]})
expected_changed = ['Collection-000/Note-00000.md'] if args.save_during_build else []
if changed != expected_changed:
    failures.append(f'vault files changed outside .lapis: {changed[:5]} (expected {expected_changed})')

result = {'binary_sha256': digest(Path(args.bin)), 'vault': 'synthetic' if not args.vault else str(vault),
          'notes': markdown, 'terminal': f'PTY xterm-256color {WIDTH}x{HEIGHT}; no terminal renderer',
          'routes': {'fault': args.fault, 'save_during_build': args.save_during_build},
          'pty_ms': marks, 'progress_seen': progress, 'notes': notes, 'health_after': health,
          'marker_indexed_after_build': marker_hit, 'files_changed': changed,
          'files_unchanged': not changed, 'init_run': False, 'failures': failures}
(out / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps({k: v for k, v in result.items() if k != 'progress_seen'} | {'progress_updates': len(progress)}, indent=2))
if args.check and failures:
    raise SystemExit('missing-index TUI regression failed: ' + '; '.join(failures))
