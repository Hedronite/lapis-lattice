#!/usr/bin/env python3
"""Scripted `lapis tui` walk in a pty against testdata/vault.

    python3 scripts/tui-demo.py [--bin target/release/lapis] [--vault testdata/vault] [--out /tmp/lapis-demo]
"""
import argparse, fcntl, os, pty, re, select, signal, struct, sys, termios, time, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ap = argparse.ArgumentParser()
ap.add_argument("--bin", default=os.environ.get("CARGO_BIN_EXE_lapis", os.path.join(ROOT, "target/release/lapis")))
ap.add_argument("--vault", default=os.path.join(ROOT, "testdata/vault"))
ap.add_argument("--out", default="/tmp/lapis-demo")
ap.add_argument("--cols", type=int, default=160)
ap.add_argument("--rows", type=int, default=44)
args = ap.parse_args()
os.makedirs(args.out, exist_ok=True)

pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.execv(args.bin, [args.bin, "tui", "--vault", args.vault])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", args.rows, args.cols, 0, 0))

ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[=>]|\x1b\(B")


class Screen:
    """Tiny VT emulator: enough for cursor moves, erase, and text."""

    def __init__(self, rows, cols):
        self.rows, self.cols = rows, cols
        self.buf = [[" "] * cols for _ in range(rows)]
        self.r = self.c = 0

    def feed(self, data: bytes):
        s = data.decode("utf-8", "replace")
        i = 0
        while i < len(s):
            m = ANSI.match(s, i)
            if m:
                seq = m.group(0)
                i = m.end()
                if seq.startswith("\x1b["):
                    body, cmd = seq[2:-1], seq[-1]
                    self._csi(body, cmd)
                continue
            ch = s[i]
            i += 1
            if ch == "\r":
                self.c = 0
            elif ch == "\n":
                self.r = min(self.rows - 1, self.r + 1)
                self.c = 0
            elif ch == "\x08":
                self.c = max(0, self.c - 1)
            elif ch.isprintable() or ch == " ":
                if 0 <= self.r < self.rows and 0 <= self.c < self.cols:
                    self.buf[self.r][self.c] = ch
                self.c += 1
                if self.c >= self.cols:
                    self.c = 0
                    self.r = min(self.rows - 1, self.r + 1)

    def _csi(self, body, cmd):
        nums = [int(x) if x else 0 for x in body.split(";") if x != ""] if body else []
        if cmd == "H" or cmd == "f":
            self.r = max(0, (nums[0] - 1) if nums else 0)
            self.c = max(0, (nums[1] - 1) if len(nums) > 1 else 0)
        elif cmd == "J":
            self.buf = [[" "] * self.cols for _ in range(self.rows)]
        elif cmd == "K":
            if 0 <= self.r < self.rows:
                for c in range(self.c, self.cols):
                    self.buf[self.r][c] = " "
        elif cmd == "A":
            self.r = max(0, self.r - (nums[0] or 1))
        elif cmd == "B":
            self.r = min(self.rows - 1, self.r + (nums[0] or 1))
        elif cmd == "C":
            self.c = min(self.cols - 1, self.c + (nums[0] or 1))
        elif cmd == "D":
            self.c = max(0, self.c - (nums[0] or 1))

    def text(self) -> str:
        return "\n".join("".join(row).rstrip() for row in self.buf)


scr = Screen(args.rows, args.cols)
log = []


def drain(seconds):
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], max(0, end - time.time()))
        if not r:
            continue
        try:
            data = os.read(fd, 65536)
        except OSError:
            break
        if not data:
            break
        log.append(data)
        scr.feed(data)


def send(s, wait=0.4):
    os.write(fd, s.encode() if isinstance(s, str) else s)
    drain(wait)


def snap(name, *needles):
    path = os.path.join(args.out, name + ".txt")
    body = scr.text()
    open(path, "w").write(body)
    missing = [n for n in needles if n.lower() not in body.lower()]
    if missing:
        print("FAIL", name, "missing", missing, file=sys.stderr)
        print(body[-2000:], file=sys.stderr)
        raise SystemExit(1)
    print("ok", name)


drain(1.2)
snap("01-start", "notes", "FILES")
send("\x10", 0.4)  # Ctrl+P
send("welcome", 1.0)
snap("02-palette", "lattice search")
send("\r", 1.2)
snap("03-open", "NORMAL")
send(" ", 0.5)
snap("04-leader", "d")
send("\x1b", 0.3)
send("?", 0.5)
snap("05-help", "Editor")
send("\x1b", 0.2)
send("\x11", 0.4)  # Ctrl+Q
os.close(fd)
try:
    os.waitpid(pid, 0)
except ChildProcessError:
    pass
print("ok exit")
