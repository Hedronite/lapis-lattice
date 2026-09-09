#!/usr/bin/env python3
"""Scripted `lapis tui` walk in a pty. Records a screen after each step and
checks the markers the Ship-1 TUI review asked for.

    python3 scripts/tui-demo.py [--bin target/release/lapis] [--vault ~/Obsidian/Atrium/Atrium] [--out /tmp/lapis-demo]

Steps (keys sent → what must be on screen):
  1. start                    sidebar with `foundry`, statusline `FILES`, `lattice`
  2. Ctrl+P, type "Hedronite" palette title `lattice search`, at least one hit
  3. Enter                    a tab opens; statusline `NORMAL`; preview pane present
  4. Space                    which-key overlay with `d` daily / `k` kanban / `?` help
  5. Space t k                kanban columns `open`, `in-progress`, `waiting`, `done`
  6. Esc, Space d             `Daily/<today>.md` tab
  7. ?                        help overlay `Editor (Vim)`
  8. Esc, Ctrl+Q              clean exit, no EMFILE / panic anywhere
"""
import argparse, fcntl, os, pty, re, select, signal, struct, sys, termios, time, datetime

ap = argparse.ArgumentParser()
ap.add_argument("--bin", default=os.path.expanduser("~/Developer/lapis/target/release/lapis"))
ap.add_argument("--vault", default=os.path.expanduser("~/Obsidian/Atrium/Atrium"))
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

    def feed(self, data: str):
        i = 0
        while i < len(data):
            ch = data[i]
            if ch == "\x1b":
                m = re.match(r"\x1b\[([0-9;?]*)([A-Za-z])", data[i:])
                if m:
                    self.csi(m.group(1), m.group(2))
                    i += len(m.group(0))
                    continue
                m = re.match(r"\x1b\][^\x07]*\x07|\x1b[=>]|\x1b\(B|\x1b\)B", data[i:])
                i += len(m.group(0)) if m else 1
                continue
            if ch == "\r":
                self.c = 0
            elif ch == "\n":
                self.r = min(self.r + 1, self.rows - 1)
            elif ch == "\x08":
                self.c = max(0, self.c - 1)
            elif ch >= " ":
                if self.c < self.cols and self.r < self.rows:
                    self.buf[self.r][self.c] = ch
                self.c += 1
                if self.c >= self.cols:
                    self.c = self.cols - 1
            i += 1

    def csi(self, p, cmd):
        nums = [int(x) if x else 0 for x in p.replace("?", "").split(";")] if p else []
        n = nums[0] if nums else 0
        if cmd == "H" or cmd == "f":
            self.r = max(0, (nums[0] if nums else 1) - 1)
            self.c = max(0, (nums[1] if len(nums) > 1 else 1) - 1)
        elif cmd == "A":
            self.r = max(0, self.r - max(1, n))
        elif cmd == "B":
            self.r = min(self.rows - 1, self.r + max(1, n))
        elif cmd == "C":
            self.c = min(self.cols - 1, self.c + max(1, n))
        elif cmd == "D":
            self.c = max(0, self.c - max(1, n))
        elif cmd == "G":
            self.c = max(0, n - 1)
        elif cmd == "d":
            self.r = max(0, n - 1)
        elif cmd == "J":
            if n == 2:
                self.buf = [[" "] * self.cols for _ in range(self.rows)]
        elif cmd == "K":
            for x in range(self.c, self.cols):
                self.buf[self.r][x] = " "

    def text(self):
        return "\n".join("".join(row).rstrip() for row in self.buf)


screen = Screen(args.rows, args.cols)
raw_log = b""


def pump(seconds):
    global raw_log
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if fd in r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return
            raw_log += data
            screen.feed(data.decode("utf-8", "replace"))


def send(keys, wait=0.5):
    os.write(fd, keys.encode() if isinstance(keys, str) else keys)
    pump(wait)


failures = []


def snap(name, *must):
    txt = screen.text()
    path = os.path.join(args.out, f"{name}.txt")
    with open(path, "w") as f:
        f.write(txt)
    missing = [m for m in must if m not in txt]
    status = "ok" if not missing else f"MISSING {missing}"
    print(f"[{name}] {status}  -> {path}")
    if missing:
        failures.append((name, missing))


today = datetime.date.today().isoformat()
pump(2.0)
snap("01-start", "foundry", "FILES", "lattice")
send("\x10", 0.3)  # Ctrl+P
send("Hedronite", 2.5)
snap("02-palette", "lattice search", "Hedronite")
send("\r", 1.0)
snap("03-open-note", "NORMAL", "preview")
send(" ", 0.4)
snap("04-leader", "daily", "kanban", "help")
send("t", 0.3)
snap("05-leader-tasks", "task list", "kanban", "calendar")
send("k", 3.0)
snap("06-kanban", "open", "in-progress", "waiting", "done")
send("\x1b", 0.3)
send(" d", 1.5)
snap("07-daily", f"Daily/{today}.md")
send("\x1b", 0.2)
send("?", 0.5)
snap("08-help", "Editor (Vim)", "Global")
send("\x1b", 0.3)
send("\x11", 1.0)  # Ctrl+Q

try:
    wpid, status = os.waitpid(pid, os.WNOHANG)
    if wpid == 0:
        os.kill(pid, signal.SIGTERM)
        _, status = os.waitpid(pid, 0)
except ChildProcessError:
    status = -1

plain = ANSI.sub("", raw_log.decode("utf-8", "replace"))
bad = [m for m in ("too many open files", "EMFILE", "panicked", "thread 'main'") if m in plain]
print(f"exit status {status}; bad markers: {bad or 'none'}; {len(raw_log)} bytes captured")
if bad:
    failures.append(("exit", bad))
sys.exit(1 if failures else 0)
