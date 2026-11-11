"""A command on a real terminal, its screen kept as text: what `cozy run ls` and `cozy machine ls` draw.

terminal.py <screen file> <command…>: the screen goes to the file each time it changes. SIGTERM presses q, and this exits as the
command did.
"""

import codecs
import fcntl
import os
import pty
import re
import select
import signal
import struct
import sys
import termios

ROWS, COLS = 50, 200
ESCAPE = re.compile(r"\x1b(?:\[([?0-9;]*)[ -/]*([@-~])|\][^\x07\x1b]*(?:\x07|\x1b\\)|[^\[\]])")


class Screen:
    def __init__(self) -> None:
        self.clear()

    def clear(self) -> None:
        self.cells: list[list[str]] = [[" "] * COLS for _ in range(ROWS)]
        self.row, self.col = 0, 0

    def feed(self, text: str) -> str:
        """Draws `text`; answers what may be the start of an escape sequence still to come."""
        at = 0
        while at < len(text):
            if text[at] == "\x1b":
                found = ESCAPE.match(text, at)
                if not found:
                    return text[at:]
                self.escape(found.group(1), found.group(2))
                at = found.end()
                continue
            self.char(text[at])
            at += 1
        return ""

    def escape(self, numbers: str | None, final: str | None) -> None:
        if final == "H":
            row, _, col = (numbers or "").partition(";")
            self.row, self.col = int(row or 1) - 1, int(col or 1) - 1
        elif final == "G":
            self.col = int(numbers or 1) - 1
        elif final == "J" and numbers == "2":
            self.cells = [[" "] * COLS for _ in range(ROWS)]
        elif final == "K":
            self.cells[self.row][self.col :] = [" "] * (COLS - self.col)

    def char(self, char: str) -> None:
        if char == "\r":
            self.col = 0
        elif char == "\n":
            self.row = min(self.row + 1, ROWS - 1)
        elif char >= " " and self.row < ROWS and self.col < COLS:
            self.cells[self.row][self.col] = char
            self.col += 1

    def text(self) -> str:
        return "\n".join("".join(row).rstrip() for row in self.cells).rstrip() + "\n"


def main() -> None:
    out, command = sys.argv[1], sys.argv[2:]
    pid, terminal = pty.fork()
    if pid == 0:
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        os.environ["TERM"] = "xterm-256color"
        os.execvp(command[0], command)
    signal.signal(signal.SIGTERM, lambda *_: os.write(terminal, b"q"))
    shown: str | None = None
    screen, decoder, held = Screen(), codecs.getincrementaldecoder("utf-8")("replace"), ""
    while True:
        try:
            if not select.select([terminal], [], [], 0.2)[0]:
                continue
            read = os.read(terminal, 65536)
        except InterruptedError:
            continue
        except OSError:
            break
        if not read:
            break
        held = screen.feed(held + decoder.decode(read))
        if screen.text() != shown:
            shown = screen.text()
            with open(out + ".new", "w") as file:
                file.write(shown)
            os.replace(out + ".new", out)
    sys.exit(os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]))


main()
