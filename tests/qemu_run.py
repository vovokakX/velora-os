#!/usr/bin/env python3
"""Boots a Velora ISO in headless QEMU and checks the serial output.

Modes:
  ktest  Boot velora-test.iso. The kernel runs its tests and exits QEMU
         through isa-debug-exit. Pass = exit status 33 and all markers seen.
  boot   Boot velora.iso and wait until the kernel reports boot complete.
         Measures the time from QEMU start to that line.

Environment overrides: QEMU (binary), OVMF_CODE, OVMF_VARS, VELORA_ACCEL.
"""

import argparse
import os
import re
import platform
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time

TIMESTAMPS = False
QEMU_SUCCESS = 33  # (0x10 << 1) | 1, see kernel/src/arch/x86_64/qemu.rs

FIRMWARE_DIRS = [
    "/usr/share/qemu",
    "/usr/share/OVMF",
    "/usr/share/edk2/x64",
    "/opt/homebrew/share/qemu",
    "/usr/local/share/qemu",
    r"C:\Program Files\qemu\share",
]
CODE_NAMES = ["edk2-x86_64-code.fd", "OVMF_CODE_4M.fd", "OVMF_CODE.fd"]
VARS_NAMES = ["edk2-i386-vars.fd", "OVMF_VARS_4M.fd", "OVMF_VARS.fd"]


def find_qemu():
    exe = os.environ.get("QEMU")
    if exe:
        return exe
    found = shutil.which("qemu-system-x86_64")
    if found:
        return found
    win = r"C:\Program Files\qemu\qemu-system-x86_64.exe"
    if os.path.exists(win):
        return win
    sys.exit("qemu_run: qemu-system-x86_64 not found (set QEMU=...)")


def find_firmware(env, names):
    if os.environ.get(env):
        return os.environ[env]
    for d in FIRMWARE_DIRS:
        for n in names:
            p = os.path.join(d, n)
            if os.path.exists(p):
                return p
    sys.exit("qemu_run: UEFI firmware not found (set %s=...)" % env)


def pick_accel():
    if os.environ.get("VELORA_ACCEL"):
        return os.environ["VELORA_ACCEL"]
    if platform.system() == "Linux" and os.access("/dev/kvm", os.R_OK | os.W_OK):
        return "kvm"
    return "tcg"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def build_command(iso, workdir, memory, monitor_port):
    code = find_firmware("OVMF_CODE", CODE_NAMES)
    vars_src = find_firmware("OVMF_VARS", VARS_NAMES)
    vars_copy = os.path.join(workdir, "vars.fd")
    shutil.copyfile(vars_src, vars_copy)
    accel = pick_accel()
    cmd = [
        find_qemu(),
        "-machine", "q35",
        "-accel", accel,
        "-cpu", "max" if accel == "tcg" else "host",
        "-m", memory,
        "-smp", "1",
        "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=" + code,
        "-drive", "if=pflash,format=raw,unit=1,file=" + vars_copy,
        "-cdrom", iso,
        "-serial", "stdio",
        "-display", "none",
        "-no-reboot",
        "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
        # Human monitor on TCP, used to inject key presses.
        "-monitor", "tcp:127.0.0.1:%d,server=on,wait=off" % monitor_port,
    ]
    return cmd, accel


class Monitor:
    """QEMU human monitor connection, used for `sendkey`."""

    def __init__(self, port):
        self.port = port
        self.sock = None

    def connect(self, timeout=10.0):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                self.sock = socket.create_connection(("127.0.0.1", self.port), timeout=2)
                self.sock.settimeout(0.2)
                self._drain()
                return
            except OSError:
                time.sleep(0.1)
        raise RuntimeError("cannot connect to the QEMU monitor")

    def _drain(self):
        try:
            while self.sock.recv(4096):
                pass
        except OSError:
            pass

    def command(self, cmd):
        if self.sock is None:
            self.connect()
        self.sock.sendall((cmd + "\n").encode())
        time.sleep(0.05)
        self._drain()

    def keys(self, names, delay=0.08):
        for name in names:
            self.command("sendkey " + name)
            time.sleep(delay)

    def close(self):
        if self.sock:
            self.sock.close()


def text_keys(text):
    """QEMU key names for plain lowercase text (US positions)."""
    names = {" ": "spc", "\n": "ret", ".": "dot", ",": "comma", "-": "minus"}
    return [names.get(c, c) for c in text]


class SerialReader:
    """Collects QEMU's stdout (the serial port) in a background thread."""

    def __init__(self, proc, log_path):
        self.proc = proc
        self.lines = []
        self.first_seen = {}
        self.start = time.monotonic()
        self.lock = threading.Lock()
        self.log = open(log_path, "w", encoding="utf-8", errors="replace")
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _run(self):
        for raw in iter(self.proc.stdout.readline, b""):
            line = raw.decode("utf-8", errors="replace").rstrip("\r\n")
            with self.lock:
                self.lines.append((time.monotonic() - self.start, line))
            self.log.write(line + "\n")
            self.log.flush()
            stamp = "%6.2f " % (time.monotonic() - self.start) if TIMESTAMPS else ""
            sys.stdout.write("  | " + stamp + line + "\n")
            sys.stdout.flush()

    def find(self, text):
        with self.lock:
            for t, line in self.lines:
                if text in line:
                    return t
        return None

    def text(self):
        with self.lock:
            return "\n".join(line for _, line in self.lines)


def run(args):
    if not os.path.exists(args.iso):
        sys.exit("qemu_run: %s does not exist (run ./build.sh first)" % args.iso)
    os.makedirs(os.path.dirname(args.log) or ".", exist_ok=True)
    with tempfile.TemporaryDirectory() as work:
        port = free_port()
        cmd, accel = build_command(args.iso, work, args.memory, port)
        print("qemu_run: mode=%s accel=%s iso=%s" % (args.mode, accel, args.iso))
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        reader = SerialReader(proc, args.log)
        monitor = Monitor(port)
        deadline = time.monotonic() + args.timeout
        try:
            if args.mode == "ktest":
                return check_ktest(proc, reader, monitor, deadline)
            return check_boot(proc, reader, monitor, deadline, args.max_boot_seconds)
        finally:
            monitor.close()
            if proc.poll() is None:
                proc.kill()
            proc.wait()
            reader.thread.join(timeout=5)
            reader.log.close()


def fail(msg):
    print("qemu_run: FAIL: " + msg)
    return 1


# Keys sent when the kernel keyboard test asks for input. The kernel
# expects the text "velA\nйЦ\n" (see kernel/src/ktest/irq.rs).
KTEST_KEYS = ["v", "e", "l", "shift-a", "ret", "alt-shift", "q", "shift-w", "alt-shift", "ret"]


def wait_for(proc, reader, text, deadline):
    while time.monotonic() < deadline:
        if reader.find(text) is not None:
            return True
        if proc.poll() is not None:
            return reader.find(text) is not None
        time.sleep(0.05)
    return False


def check_ktest(proc, reader, monitor, deadline):
    keys_sent = False
    while proc.poll() is None and time.monotonic() < deadline:
        if not keys_sent and reader.find("ktest: keyboard: waiting for input") is not None:
            monitor.keys(KTEST_KEYS)
            keys_sent = True
        time.sleep(0.1)
    if proc.poll() is None:
        return fail("timeout: kernel tests did not finish")
    reader.thread.join(timeout=5)
    out = reader.text()
    if proc.returncode != QEMU_SUCCESS:
        return fail("QEMU exit status %d (expected %d)" % (proc.returncode, QEMU_SUCCESS))
    required = [
        "Velora OS",
        "GDT, TSS, IDT, syscall entry and SSE state set up",
        "scheduler running",
        "hello from ring 3",
        "Hello from /bin/hello",
        "vtest: 14 passed, 0 failed",
        "ktest: ALL PASSED",
        # The deliberate panic must produce a complete report.
        "KERNEL PANIC",
        "message:  ktest: deliberate panic",
        "location: kernel/src/ktest/mod.rs",
        "backtrace:",
        "ktest: panic path ok",
    ]
    missing = [m for m in required if m not in out.replace("\\", "/")]
    if missing:
        return fail("missing output: %s" % missing)
    if "CPU EXCEPTION" in out:
        return fail("unexpected CPU exception report")
    print("qemu_run: PASS (kernel tests)")
    return 0


def check_boot(proc, reader, monitor, deadline, max_seconds):
    marker = "boot complete"
    while time.monotonic() < deadline:
        t = reader.find(marker)
        if t is not None:
            break
        if proc.poll() is not None:
            break
        time.sleep(0.05)
    t = reader.find(marker)
    out = reader.text()
    for bad in ("KERNEL PANIC", "CPU EXCEPTION"):
        if bad in out:
            return fail("'%s' in boot log" % bad)
    if t is None:
        return fail("kernel did not report '%s'" % marker)
    print("qemu_run: boot complete after %.2f s (QEMU start, firmware included)" % t)
    m = re.search(r"memory in use: (\d+) KiB of (\d+) KiB", out)
    if not m:
        return fail("kernel did not report memory use")
    used_kib = int(m.group(1))
    print("qemu_run: memory in use after boot: %d KiB (%.1f MiB)" % (used_kib, used_kib / 1024))
    if used_kib >= 64 * 1024:
        return fail("idle memory use %d KiB is not below 64 MiB" % used_kib)
    if max_seconds and t > max_seconds:
        return fail("boot took %.2f s, limit %.2f s" % (t, max_seconds))
    if "Hello from /bin/hello" not in out or "/bin/hello (pid 1) exited with status 0" not in out:
        return fail("the first user program did not run")
    # Keyboard: type a line in each layout and check the echo.
    if not wait_for(proc, reader, "keyboard test ready", deadline):
        return fail("keyboard test did not start")
    monitor.keys(text_keys("hello world\n"))
    if not wait_for(proc, reader, "echo: hello world", deadline):
        return fail("typed text was not echoed")
    monitor.keys(["alt-shift"] + text_keys("ghbdtn\n"))
    if not wait_for(proc, reader, "echo: привет", deadline):
        return fail("Russian layout input was not echoed")
    print("qemu_run: keyboard input works (EN and RU)")
    print("qemu_run: PASS (boot)")
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("mode", choices=["ktest", "boot"])
    p.add_argument("--iso")
    p.add_argument("--timeout", type=float, default=120.0)
    p.add_argument("--memory", default="256M")
    p.add_argument("--max-boot-seconds", type=float, default=0.0)
    p.add_argument("--log")
    p.add_argument("--timestamps", action="store_true", help="prefix each serial line with seconds since QEMU start")
    args = p.parse_args()
    global TIMESTAMPS
    TIMESTAMPS = args.timestamps
    if not args.iso:
        args.iso = "velora-test.iso" if args.mode == "ktest" else "velora.iso"
    if not args.log:
        args.log = os.path.join("build", "logs", args.mode + ".log")
    sys.exit(run(args))


if __name__ == "__main__":
    main()
