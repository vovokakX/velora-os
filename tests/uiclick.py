"""Clicks every interactive element of the graphical installer in QEMU and
checks that each one reacted (used by `qemu_run.py clicks`).

The Live image is built with `velora.uitest` on the kernel command line
(`./build.sh --uitest`). The installer then prints, after every change, a
line like

    velora-start: ui location state=1a2b3c4d focus=20 popup=- hit=20003
        scroll=0,0 areas 800:58,618,72,36 20:376,231,384,297 20003:382,...

with a digest of everything the user can change, the focused widget, the
open drop-down list, the widget that last took input, the scroll offsets
of the two lists and the interactive areas (id: x, y, width, height).
The pointer is a USB tablet driven through QMP `input-send-event`, so
clicks land on exact pixels.
"""

import json
import re
import socket
import time

POPUP_ROW = 0x70000000
BACK, CONTINUE, REFRESH = 900, 901, 902

REPORT = re.compile(
    r"velora-start: ui (\w+) state=([0-9a-f]+) focus=(\S+) popup=(\S+) hit=(\S+) "
    r"scroll=(-?\d+),(-?\d+) areas(.*)$")


class Qmp:
    """A QMP connection for absolute pointer events."""

    def __init__(self, port):
        self.port = port
        self.sock = None
        self.buf = b""

    def connect(self, timeout=20.0):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                self.sock = socket.create_connection(("127.0.0.1", self.port), timeout=5)
                break
            except OSError:
                time.sleep(0.1)
        else:
            raise RuntimeError("cannot connect to QMP")
        self._reply()  # greeting
        self.execute("qmp_capabilities")

    def _reply(self):
        while True:
            while b"\n" in self.buf:
                line, self.buf = self.buf.split(b"\n", 1)
                if not line.strip():
                    continue
                msg = json.loads(line)
                if "event" in msg:
                    continue
                return msg
            chunk = self.sock.recv(65536)
            if not chunk:
                raise RuntimeError("QMP closed")
            self.buf += chunk

    def execute(self, cmd, args=None):
        if self.sock is None:
            self.connect()
        msg = {"execute": cmd}
        if args is not None:
            msg["arguments"] = args
        self.sock.sendall((json.dumps(msg) + "\n").encode())
        reply = self._reply()
        if "error" in reply:
            raise RuntimeError("QMP %s: %s" % (cmd, reply["error"]))
        return reply.get("return")

    def events(self, events):
        self.execute("input-send-event", {"events": events})

    def move(self, x, y, w, h):
        """Puts the pointer on pixel (x, y) of a w x h screen."""
        ax = round(x * 32767 / max(w - 1, 1))
        ay = round(y * 32767 / max(h - 1, 1))
        self.events([
            {"type": "abs", "data": {"axis": "x", "value": ax}},
            {"type": "abs", "data": {"axis": "y", "value": ay}},
        ])

    def button(self, down, button="left"):
        self.events([{"type": "btn", "data": {"down": down, "button": button}}])

    def click(self, x, y, w, h):
        self.move(x, y, w, h)
        time.sleep(0.05)
        self.button(True)
        time.sleep(0.05)
        self.button(False)

    def wheel(self, down=True):
        b = "wheel-down" if down else "wheel-up"
        self.button(True, b)
        self.button(False, b)

    def close(self):
        if self.sock:
            self.sock.close()


class Report:
    def __init__(self, m):
        self.screen = m.group(1)
        self.state = m.group(2)
        self.focus = m.group(3)
        self.popup = m.group(4)
        self.hit = m.group(5)
        self.scroll = (int(m.group(6)), int(m.group(7)))
        self.areas = {}
        for item in m.group(8).split():
            ident, rect = item.split(":")
            self.areas[int(ident)] = tuple(int(v) for v in rect.split(","))
        self.line = m.group(0)

    def center(self, ident):
        x, y, w, h = self.areas[ident]
        return x + w // 2, y + h // 2


class Walker:
    """Follows the reports on the serial port and clicks."""

    def __init__(self, session, qmp, size):
        self.s = session
        self.qmp = qmp
        self.w, self.h = size
        self.last = None
        self.checked = 0

    def latest(self):
        """The newest report line after the session's mark, if any."""
        found = None
        for _, line, unfinished in self.s.lines():
            if unfinished:
                continue
            m = REPORT.search(line)
            if m:
                found = Report(m)
        return found

    def wait_report(self, what, timeout=60.0, change=True):
        """Waits for a report that differs from the last one."""
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            r = self.latest()
            if r and (not change or self.last is None or r.line != self.last.line):
                # Settle: a burst of reports ends with the stable one.
                time.sleep(0.3)
                r2 = self.latest()
                if r2 and r2.line != r.line:
                    continue
                self.last = r
                return r
            if self.s.proc.poll() is not None:
                break
            time.sleep(0.05)
        raise AssertionError("no reaction to %s (last report: %s)" % (
            what, self.last.line[:160] if self.last else "none"))

    def screen(self, name, timeout=120.0):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            r = self.wait_report("the screen " + name, timeout=max(1.0, end - time.monotonic()),
                                 change=False)
            if r.screen == name:
                return r
            time.sleep(0.2)
        raise AssertionError("screen %s did not appear" % name)

    def click(self, ident, what=None):
        r = self.last
        if ident not in r.areas:
            raise AssertionError("%s: no area %d on %s" % (what or ident, ident, r.screen))
        x, y = r.center(ident)
        self.qmp.click(x, y, self.w, self.h)
        return self.wait_report("a click on %s (%d) on %s" % (what or ident, ident, r.screen))

    def check_value(self, ident, what):
        """A click that must change the state (a choice, a switch)."""
        before = self.last
        r = self.click(ident, what)
        if r.state == before.state:
            raise AssertionError("%s (%d) on %s: the state did not change" % (what, ident, r.screen))
        if r.hit != str(ident):
            raise AssertionError("%s (%d): the click went to %s" % (what, ident, r.hit))
        self.checked += 1
        return r

    def check_hit(self, ident, what):
        """A click on a button that changes no choice (Refresh): it must
        take the click."""
        r = self.click(ident, what)
        if r.hit != str(ident):
            raise AssertionError("%s (%d): the click went to %s" % (what, ident, r.hit))
        self.checked += 1
        return r

    def check_focus(self, ident, what):
        """A click that focuses a widget (lists, text fields)."""
        r = self.click(ident, what)
        if r.focus != str(ident):
            raise AssertionError("%s (%d) on %s: not focused after a click (focus %s)"
                                 % (what, ident, r.screen, r.focus))
        self.checked += 1
        return r

    def check_text(self, ident, what, text="a"):
        """Click a text field, type into it: the state changes."""
        self.check_focus(ident, what)
        before = self.last.state
        self.s.type(text)
        r = self.wait_report("typing into %s (%d)" % (what, ident))
        if r.state == before:
            raise AssertionError("%s (%d): typing did not change the text" % (what, ident))
        return r

    def check_dropdown(self, ident, what):
        """Opens the list and picks its rows in turn until the choice
        changes (one of them is the current choice)."""
        before = self.last.state
        n = 0
        while True:
            r = self.click(ident, what)
            if r.popup != str(ident):
                raise AssertionError("%s (%d): the list did not open (popup %s)" % (what, ident, r.popup))
            rows = sorted(k for k in r.areas if k >= POPUP_ROW)
            if not rows:
                raise AssertionError("%s (%d): the open list shows no rows" % (what, ident))
            if n >= len(rows):
                raise AssertionError("%s (%d): choosing rows did not change the state" % (what, ident))
            row = rows[n]
            r = self.click(row, "row %d of %s" % (row - POPUP_ROW, what))
            if r.popup != "-":
                raise AssertionError("%s (%d): the list stayed open" % (what, ident))
            if r.state != before:
                self.checked += 1
                return r
            n += 1

    def check_list(self, ident, which, what):
        """A select list: the wheel scrolls it, each visible row can be
        chosen with a click."""
        r = self.last
        x, y = r.center(ident)
        self.qmp.move(x, y, self.w, self.h)
        time.sleep(0.1)
        self.qmp.wheel(True)
        r = self.wait_report("the wheel over %s" % what)
        if r.scroll[which] <= 0:
            raise AssertionError("%s: the wheel did not scroll it (%s)" % (what, r.scroll))
        self.qmp.wheel(False)
        self.wait_report("the wheel back over %s" % what)
        rows = sorted(k for k in self.last.areas if ident * 1000 < k < (ident + 1) * 1000)
        if len(rows) < 3:
            raise AssertionError("%s: only %d rows reported" % (what, len(rows)))
        for row in rows[:3]:
            if row not in self.last.areas:
                continue
            self.check_value(row, "row %d of %s" % (row - ident * 1000 - 1, what))
        self.checked += 1

    def go(self, ident, screen):
        r = self.click(ident, "Continue" if ident == CONTINUE else "Back")
        return self.screen(screen)


def walk(s, qmp, size, shots=None):
    """The whole installer, every element; returns how many were checked."""
    k = Walker(s, qmp, size)
    k.screen("welcome")
    # Sidebar theme switch (light, then dark again).
    k.check_value(800, "theme switch")
    k.check_value(800, "theme switch")
    k.check_dropdown(10, "installer language")
    k.check_dropdown(10, "installer language")
    k.go(CONTINUE, "location")
    k.go(BACK, "welcome")
    k.go(CONTINUE, "location")
    k.check_list(20, 0, "system language")
    k.check_list(21, 1, "region")
    k.go(CONTINUE, "keyboard")
    k.check_dropdown(30, "keyboard layout")
    k.check_value(31, "system shortcuts")
    for i in (42, 41, 40):
        k.check_value(i, "pointer style %d" % (i - 40))
    k.check_value(45, "large pointer")
    k.check_value(45, "large pointer")
    k.go(CONTINUE, "appearance")
    for i in (52, 51, 50):
        k.check_value(i, "theme card %d" % (i - 50))
    k.check_dropdown(53, "theme")
    k.check_value(50, "theme card 0")
    k.check_dropdown(54, "icons")
    k.check_dropdown(55, "wallpaper")
    for i in (60, 61, 62, 63):
        k.check_value(i, "option %d" % (i - 60))
        k.check_value(i, "option %d" % (i - 60))
    for i in range(77, 69, -1):
        k.check_value(i, "accent %d" % (i - 70))
    k.go(CONTINUE, "disk")
    k.check_hit(REFRESH, "Refresh")
    for i in (82, 81, 80):
        k.check_value(i, "partitioning mode %d" % (i - 80))
    k.check_value(82, "expert mode")
    k.go(CONTINUE, "partitions")
    k.check_dropdown(100, "EFI size")
    k.check_dropdown(101, "root size")
    k.check_text(102, "file system label")
    k.go(CONTINUE, "user")
    # Continue is disabled until every field is filled.
    if CONTINUE in k.last.areas:
        raise AssertionError("Continue is active with empty fields")
    for ident, text in ((110, "anna"), (111, "pw1"), (112, "pw1"), (113, "rpw"), (114, "rpw")):
        k.check_text(ident, "field %d" % ident, text)
    if CONTINUE not in k.last.areas:
        raise AssertionError("Continue stays disabled with every field filled")
    k.go(CONTINUE, "confirm")
    k.go(BACK, "user")
    k.go(CONTINUE, "confirm")
    return k.checked
