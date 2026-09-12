#!/usr/bin/env python3
"""Measure a release TUI on the unmodified standard synthetic fixture.

PTY-output timings include process creation and output transport, but exclude
terminal drawing and display presentation. They do not close native latency gates.
No notes are written. Index preparation must be explicit and outside this script.
"""
import argparse
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import pty
import select
import statistics
import struct
import subprocess
import termios
import time

from ux_terminal import screen_text


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def summary(values):
    ordered = sorted(values)
    return {"n": len(values), "median": statistics.median(values),
            "p95": ordered[math.ceil(len(values) * .95) - 1], "worst": ordered[-1]}


def cpu_seconds(value):
    days, _, clock = value.rpartition('-')
    total = 0.0
    for field in clock.split(':'):
        total = total * 60 + float(field)
    return total + (int(days) * 86400 if days else 0)


def resources(pid):
    rows = subprocess.check_output(['ps', '-axo', 'pid=,ppid=,rss=,time='], text=True)
    parsed = []
    for row in rows.splitlines():
        p, parent, rss, cpu = row.split()
        parsed.append({'pid': int(p), 'ppid': int(parent), 'rss_kib': int(rss),
                       'cpu_seconds': cpu_seconds(cpu)})
    included = {pid}
    while True:
        added = {r['pid'] for r in parsed if r['ppid'] in included} - included
        if not added:
            break
        included.update(added)
    return {'time': time.monotonic(), 'processes': [r for r in parsed if r['pid'] in included]}


class Session:
    def __init__(self, binary, vault, config):
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
        env = os.environ.copy()
        for key in ('LAPIS_VAULT', 'LAPIS_LATTICE_URL', 'LAPIS_UI_TRACE'):
            env.pop(key, None)
        env.update(XDG_CONFIG_HOME=str(config), TERM='xterm-256color')
        self.raw = bytearray()
        self.fd = master
        self.started = time.perf_counter()
        self.process = subprocess.Popen([str(binary), '--vault', str(vault), 'tui'],
                                        stdin=slave, stdout=slave, stderr=slave,
                                        env=env, start_new_session=True)
        os.close(slave)

    def drain(self, seconds):
        until = time.perf_counter() + seconds
        while time.perf_counter() < until:
            if select.select([self.fd], [], [], max(0, until - time.perf_counter()))[0]:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    break
                if not data:
                    break
                self.raw.extend(data)

    def wait(self, expected, timeout=15):
        until = time.perf_counter() + timeout
        while time.perf_counter() < until:
            if select.select([self.fd], [], [], max(0, until - time.perf_counter()))[0]:
                data = os.read(self.fd, 65536)
                received = time.perf_counter()
                if not data:
                    raise RuntimeError('TUI exited before expected screen')
                self.raw.extend(data)
                screen = screen_text(self.raw, 120, 40)
                if all(text in screen for text in expected):
                    return received
            if self.process.poll() is not None:
                break
        raise RuntimeError(f'Timed out waiting for {expected!r}')

    def send(self, data):
        started = time.perf_counter()
        os.write(self.fd, data)
        return started

    def close(self):
        if self.process.poll() is None:
            self.send(b'\x11')  # Ctrl+Q; no edits are made by this harness.
            self.drain(.2)
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                self.process.wait(timeout=3)
                raise RuntimeError('TUI did not exit through its clean quit command')
        os.close(self.fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin', required=True, type=Path)
    parser.add_argument('--vault', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--phase', choices=['missing-index', 'indexed'], required=True)
    parser.add_argument('--idle-seconds', type=int, default=60)
    args = parser.parse_args()
    binary, vault, out = args.bin.resolve(), args.vault.resolve(), args.out.resolve()
    if out.exists():
        parser.error('Output must be new; retained evidence is never overwritten')
    manifest_path = vault / 'fixture-manifest.json'
    manifest = json.loads(manifest_path.read_text())
    if manifest['id'] != 'lapis-ux-standard-v1' or manifest['notes'] != 10000:
        parser.error('Requires the standard synthetic fixture')
    known = {entry['path'] for entry in manifest['files']}
    actual = {str(path.relative_to(vault)) for path in vault.rglob('*.md')}
    if known != actual:
        parser.error('Markdown paths differ from the frozen fixture')
    for entry in manifest['files']:
        if digest(vault / entry['path']) != entry['sha256']:
            parser.error(f"Fixture differs: {entry['path']}")
    index = vault / '.lapis' / 'lattice.sqlite'
    if index.exists() != (args.phase == 'indexed'):
        parser.error('Index presence does not match phase; prepare it explicitly')
    out.mkdir(parents=True)
    config = out / 'config'
    (config / 'lapis').mkdir(parents=True)
    (config / 'lapis' / 'config.toml').write_text('[lattice]\nmode = "embedded"\n')
    health = None
    if args.phase == 'indexed':
        env = os.environ.copy()
        env['XDG_CONFIG_HOME'] = str(config)
        for key in ('LAPIS_VAULT', 'LAPIS_LATTICE_URL'):
            env.pop(key, None)
        probe = subprocess.run([str(binary), '--vault', str(vault), '--json', 'vault', 'info'],
                               env=env, text=True, capture_output=True, check=True)
        (out / 'index-health.json').write_text(probe.stdout)
        health = json.loads(probe.stdout)['data']['lattice']['health']
        if health['documents_indexed'] != 10000 or health['embedder'] != 'none' or not health['graph']['built']:
            parser.error('Expected a built 10,000-document index with no embedder')
    result = {'binary_sha256': digest(binary), 'fixture_manifest_sha256': digest(manifest_path),
              'platform': platform.platform(), 'phase': args.phase,
              'terminal': 'PTY xterm-256color, 120x40; no terminal renderer/display',
              'os_cache': 'unflushed; fixture hash validation warms note data before measurements',
              'backend': 'embedded, default no embedder; no external model/service',
              'percentile': 'nearest rank', 'runs': [], 'idle': None,
              'index_health': health, 'load_average_before': os.getloadavg(),
              'cpu_count': os.cpu_count(),
              'limitations': ['PTY receipt is not native visible update.',
                              'Warm reopen means an immediate subsequent new process, not tab reopen.',
                              'File open uses the standard 10,486-byte note, not the 100 KiB switch fixture.']}
    runs = 1 if args.phase == 'missing-index' else 30
    try:
        for number in range(runs):
            session = Session(binary, vault, config)
            try:
                files = session.wait(['Collection-000', 'Space leader'])
                # Open the first collection, then its first note using the actual key path.
                session.send(b'\r')
                session.wait(['Note-00000.md'])
                selected = session.send(b'j\r')
                editor = session.wait(['NORMAL', 'A synthetic knowledge'])
                record = {'number': number + 1,
                          'group': args.phase if number < 10 else 'warm-reopen',
                          'files_pty_ms': (files - session.started) * 1000,
                          'editor_pty_ms': (editor - session.started) * 1000,
                          'note_open_pty_ms': (editor - selected) * 1000}
                result['runs'].append(record)
                (out / f'run-{number + 1:02d}-editor.txt').write_text(screen_text(session.raw, 120, 40))
                if args.phase == 'indexed' and number == runs - 1:
                    session.drain(5)
                    samples = [resources(session.process.pid)]
                    stop = time.monotonic() + args.idle_seconds
                    while time.monotonic() < stop:
                        session.drain(min(1, stop - time.monotonic()))
                        samples.append(resources(session.process.pid))
                    elapsed = samples[-1]['time'] - samples[0]['time']
                    cpu = lambda sample: sum(p['cpu_seconds'] for p in sample['processes'])
                    rss = [sum(p['rss_kib'] for p in s['processes']) / 1024 for s in samples]
                    result['idle'] = {'elapsed_seconds': elapsed, 'samples': samples,
                                      'cpu_percent_one_core': (cpu(samples[-1]) - cpu(samples[0])) / elapsed * 100,
                                      'rss_mib': summary(rss), 'graph': 'closed (TUI editor and preview)'}
                print(json.dumps(record), flush=True)
            finally:
                try:
                    session.close()
                finally:
                    (out / f'run-{number + 1:02d}.ansi').write_bytes(session.raw)
        for entry in manifest['files']:
            if digest(vault / entry['path']) != entry['sha256']:
                raise RuntimeError(f"Note changed: {entry['path']}")
        result['notes_unchanged'] = True
        result['summary'] = {group: {metric: summary([r[metric] for r in result['runs'] if r['group'] == group])
                                   for metric in ['files_pty_ms', 'editor_pty_ms', 'note_open_pty_ms']}
                             for group in sorted({r['group'] for r in result['runs']})}
    except Exception as error:
        result['error'] = str(error)
        raise
    finally:
        (out / 'measurements.json').write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
