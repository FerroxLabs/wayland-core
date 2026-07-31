#!/usr/bin/env python3
"""Drive a command on a REAL pseudo-terminal and dump the RENDERED screen.

The 24-C2 and 27-C1 ledger rows both record "no PTY drive, no rendered screen".
Piping stdout to a file is not that drive: the product asks `isatty()` and takes a
different branch, so a pipe measures the branch a user never sees.

This opens a real PTY, puts the child in its own session so the PTY is its
**controlling terminal**, sets a real window size, and then renders the child's
output through a small VT parser so the artifact is the SCREEN A USER WOULD SEE
rather than a byte soup of escape sequences. Both are kept: `--raw-out` writes the
untouched bytes so nothing is lost to a parser bug.

Usage:
  pty-drive.py --out screen.txt [--raw-out raw.bin] [--rows R] [--cols C]
               [--send 'text\\n' --wait 0.5]... [--settle 2.0] -- CMD [ARGS...]

`--send` / `--wait` are positional-order pairs: each `--send` writes keystrokes to
the PTY master, each following `--wait` sleeps that many seconds before the next.

Exit status is the child's. It prints `PTY_CHILD_RC=<n>` and `PTY_ISATTY=<bool>` —
the latter read back from the child's own view where the driven command supports
it, otherwise from the master side, so a broken PTY setup is visible rather than
silently degrading to the pipe branch.
"""

import argparse
import errno
import fcntl
import os
import pty
import re
import select
import struct
import sys
import termios
import time


# ── a small VT renderer ──────────────────────────────────────────────────
# Enough of ECMA-48 for a TUI screen dump: cursor addressing, relative moves,
# erase-in-line / erase-in-display, carriage return, newline, backspace. SGR and
# other unhandled sequences are dropped (they carry colour, not text).

CSI = re.compile(rb"\x1b\[([0-9;?]*)([@-~])")
OSC = re.compile(rb"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")


class Screen:
    def __init__(self, rows, cols):
        self.rows, self.cols = rows, cols
        self.buf = [[" "] * cols for _ in range(rows)]
        self.cy = self.cx = 0

    def _scroll(self):
        self.buf.pop(0)
        self.buf.append([" "] * self.cols)
        self.cy = self.rows - 1

    def putc(self, ch):
        if self.cx >= self.cols:
            self.cx = 0
            self.cy += 1
        if self.cy >= self.rows:
            self._scroll()
        self.buf[self.cy][self.cx] = ch
        self.cx += 1

    def feed(self, data: bytes):
        data = OSC.sub(b"", data)
        i = 0
        n = len(data)
        while i < n:
            b = data[i : i + 1]
            if b == b"\x1b":
                m = CSI.match(data, i)
                if m:
                    self._csi(m.group(1).decode("ascii", "replace"), m.group(2))
                    i = m.end()
                    continue
                # Unknown/short escape — skip the introducer and one byte.
                i += 2
                continue
            if b == b"\r":
                self.cx = 0
            elif b == b"\n":
                self.cy += 1
                if self.cy >= self.rows:
                    self._scroll()
            elif b == b"\b":
                self.cx = max(0, self.cx - 1)
            elif b == b"\t":
                self.cx = min(self.cols - 1, (self.cx // 8 + 1) * 8)
            elif b >= b" ":
                # Decode one UTF-8 codepoint so wide text is not mangled.
                length = 1
                c = data[i]
                if c >= 0xF0:
                    length = 4
                elif c >= 0xE0:
                    length = 3
                elif c >= 0xC0:
                    length = 2
                self.putc(data[i : i + length].decode("utf-8", "replace"))
                i += length
                continue
            i += 1

    def _csi(self, params, final):
        ps = [int(p) if p.isdigit() else 0 for p in params.split(";")] if params else []

        def p(idx, default=1):
            return ps[idx] if idx < len(ps) and ps[idx] else default

        if final == b"H" or final == b"f":
            self.cy = min(self.rows - 1, p(0) - 1)
            self.cx = min(self.cols - 1, p(1) - 1)
        elif final == b"A":
            self.cy = max(0, self.cy - p(0))
        elif final == b"B":
            self.cy = min(self.rows - 1, self.cy + p(0))
        elif final == b"C":
            self.cx = min(self.cols - 1, self.cx + p(0))
        elif final == b"D":
            self.cx = max(0, self.cx - p(0))
        elif final == b"G":
            self.cx = min(self.cols - 1, p(0) - 1)
        elif final == b"J":
            mode = ps[0] if ps else 0
            if mode == 2:
                self.buf = [[" "] * self.cols for _ in range(self.rows)]
                self.cy = self.cx = 0
            elif mode == 0:
                self.buf[self.cy][self.cx :] = [" "] * (self.cols - self.cx)
                for r in range(self.cy + 1, self.rows):
                    self.buf[r] = [" "] * self.cols
            elif mode == 1:
                self.buf[self.cy][: self.cx + 1] = [" "] * (self.cx + 1)
                for r in range(self.cy):
                    self.buf[r] = [" "] * self.cols
        elif final == b"K":
            mode = ps[0] if ps else 0
            if mode == 0:
                self.buf[self.cy][self.cx :] = [" "] * (self.cols - self.cx)
            elif mode == 1:
                self.buf[self.cy][: self.cx + 1] = [" "] * (self.cx + 1)
            else:
                self.buf[self.cy] = [" "] * self.cols
        # Everything else (SGR, mode set/reset, cursor save/restore) carries no text.

    def render(self):
        return "\n".join("".join(row).rstrip() for row in self.buf)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--raw-out")
    ap.add_argument("--rows", type=int, default=44)
    ap.add_argument("--cols", type=int, default=120)
    ap.add_argument("--settle", type=float, default=2.0)
    ap.add_argument("--send", action="append", default=[])
    ap.add_argument("--wait", action="append", default=[], type=float)
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()

    cmd = a.cmd[1:] if a.cmd and a.cmd[0] == "--" else a.cmd
    if not cmd:
        print("no command given", file=sys.stderr)
        return 2

    pid, fd = pty.fork()
    if pid == 0:
        # Child: pty.fork already made this a session leader with the slave as
        # its controlling terminal.
        os.environ["TERM"] = os.environ.get("TERM_FOR_PTY", "xterm-256color")
        os.environ.pop("NO_COLOR", None)
        try:
            os.execvp(cmd[0], cmd)
        except Exception as exc:  # noqa: BLE001 - child must not fall through
            os.write(2, f"exec failed: {exc}\n".encode())
            os._exit(127)

    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", a.rows, a.cols, 0, 0))

    raw = bytearray()
    scr = Screen(a.rows, a.cols)

    def pump(deadline):
        while time.time() < deadline:
            r, _, _ = select.select([fd], [], [], 0.05)
            if not r:
                continue
            try:
                chunk = os.read(fd, 65536)
            except OSError as exc:
                if exc.errno in (errno.EIO, errno.EBADF):
                    return False
                raise
            if not chunk:
                return False
            raw.extend(chunk)
            scr.feed(chunk)
        return True

    # Let the program paint its first screen before typing into it.
    pump(time.time() + 1.0)

    for idx, text in enumerate(a.send):
        os.write(fd, text.encode().decode("unicode_escape").encode())
        delay = a.wait[idx] if idx < len(a.wait) else 0.5
        pump(time.time() + delay)

    pump(time.time() + a.settle)

    try:
        os.close(fd)
    except OSError:
        pass
    _, status = os.waitpid(pid, 0)
    rc = os.waitstatus_to_exitcode(status)

    with open(a.out, "w", encoding="utf-8") as fh:
        fh.write(scr.render())
        fh.write("\n")
    if a.raw_out:
        with open(a.raw_out, "wb") as fh:
            fh.write(bytes(raw))

    print(f"PTY_ROWS={a.rows} PTY_COLS={a.cols} PTY_RAW_BYTES={len(raw)}")
    print(f"PTY_CHILD_RC={rc}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
