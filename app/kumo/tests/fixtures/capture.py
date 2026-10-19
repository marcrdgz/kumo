#!/usr/bin/env python3
"""Capture raw PTY output of an interactive CLI at 120x40.

usage: capture.py OUT CWD SCRIPT -- CMD...
SCRIPT is a ';'-separated list of steps: `w<secs>` waits, `t<text>` types
text one char at a time, `k<hex>` sends raw bytes.
"""
import fcntl, os, pty, re, select, signal, struct, sys, termios, time

out_path, cwd, steps = sys.argv[1], sys.argv[2], sys.argv[3]
cmd = sys.argv[sys.argv.index("--") + 1:]

pid, fd = pty.fork()
if pid == 0:
    os.chdir(cwd)
    os.environ.update(TERM="xterm-256color", COLORTERM="truecolor", COLUMNS="120", LINES="40")
    os.execvp(cmd[0], cmd)

fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
os.kill(pid, signal.SIGWINCH)
captured = bytearray()

# Minimal terminal query responder so TUIs don't stall waiting for replies.
REPLIES = [
    (re.compile(rb"\x1b\[6n"), b"\x1b[1;1R"),
    (re.compile(rb"\x1b\[c|\x1b\[0c"), b"\x1b[?62;22c"),
    (re.compile(rb"\x1b\[>0?c"), b"\x1b[>1;10;0c"),
    (re.compile(rb"\x1b\[\?u"), b"\x1b[?0u"),
    (re.compile(rb"\x1b\]10;\?(\x07|\x1b\\)"), b"\x1b]10;rgb:d8d8/dede/e9e9\x1b\\"),
    (re.compile(rb"\x1b\]11;\?(\x07|\x1b\\)"), b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\"),
    (re.compile(rb"\x1b\[14t"), b"\x1b[4;800;1200t"),
    (re.compile(rb"\x1b\[16t"), b"\x1b[6;20;10t"),
    (re.compile(rb"\x1b\[18t"), b"\x1b[8;40;120t"),
    (re.compile(rb"\x1bP\+q[0-9a-fA-F;]*\x1b\\"), b"\x1bP0+r\x1b\\"),
    (re.compile(rb"\x1b_Gi=31337[^\x1b]*\x1b\\"), b"\x1b_Gi=31337;ENOTSUPPORTED\x1b\\"),
    (re.compile(rb"\x1b\[\?996n"), b"\x1b[?997;1n"),
    (re.compile(rb"\x1b\[\?2027\$p"), b"\x1b[?2027;0$y"),
]


def pump(secs):
    end = time.time() + secs
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], max(0.0, end - time.time()))
        if not r:
            continue
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            return False
        if not chunk:
            return False
        captured.extend(chunk)
        for pat, reply in REPLIES:
            for _ in pat.finditer(chunk):
                os.write(fd, reply)
    return True


alive = True
for step in steps.split(";"):
    if not alive:
        break
    kind, arg = step[0], step[1:]
    if kind == "w":
        alive = pump(float(arg))
    elif kind == "t":
        for ch in arg:
            os.write(fd, ch.encode())
            alive = pump(0.08)
    elif kind == "k":
        os.write(fd, bytes.fromhex(arg))
        alive = pump(0.3)

pump(1.5)
try:
    os.kill(pid, signal.SIGKILL)
except ProcessLookupError:
    pass
open(out_path, "wb").write(captured)
print(f"{out_path}: {len(captured)} bytes")
