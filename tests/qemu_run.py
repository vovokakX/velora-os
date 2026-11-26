#!/usr/bin/env python3
"""Boots a Velora ISO in headless QEMU and checks the serial output.

Modes:
  ktest  Boot velora-test.iso twice with the same disks. The kernel runs its
         tests and exits QEMU through isa-debug-exit. Pass = exit status 33
         and all markers seen in both boots. The first boot creates file
         systems on a blank disk; the second checks that the files survived
         and that the journal was replayed. After each boot the disk writes
         and both file systems are checked on the host (velora-fsck, and
         fsck.fat when it is installed).
  boot   Boot velora.iso, measure the time to the login prompt, then log
         in through the emulated keyboard and run a shell scenario
         (commands, pipes, redirection, Tab, history, Ctrl+C, RU layout),
         log out, log in as root and power off with `shutdown`.

Environment overrides: QEMU (binary), OVMF_CODE, OVMF_VARS, VELORA_ACCEL,
VELORA_QEMU_EXTRA (extra QEMU arguments, e.g. "-trace virtio*"),
VELORA_VIRTIO_OPTS (appended to the virtio-blk device, e.g. ",ioeventfd=off").
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
    "/usr/share/edk2/ovmf",
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


# Test disks: (kind, QEMU arguments). Each image gets a signature at LBA 1;
# the kernel tests read it and write "VELORA-WRITE-<KIND>" to LBA 2.
DISK_KINDS = ["virtio", "ahci", "nvme"]
DISK_SIZE = 32 * 1024 * 1024
SECTOR = 512
# Blank disk for the file system tests: VeloraFS in the first half, FAT32
# in the second (see kernel/src/ktest/fs.rs).
FS_DISK_SIZE = 128 * 1024 * 1024
FS_HALF = FS_DISK_SIZE // 2


def disk_args(kind, path):
    drive = "if=none,id=d%s,file=%s,format=raw" % (kind, path)
    device = {
        "virtio": "virtio-blk-pci,drive=dvirtio,disable-legacy=on" + os.environ.get("VELORA_VIRTIO_OPTS", ""),
        "fs": "virtio-blk-pci,drive=dfs,disable-legacy=on" + os.environ.get("VELORA_VIRTIO_OPTS", ""),
        "ahci": "ide-hd,drive=dahci,bus=ide.1",
        "nvme": "nvme,serial=velora0,drive=dnvme",
    }[kind]
    return ["-drive", drive, "-device", device]


def make_disks(workdir):
    paths = {}
    for kind in DISK_KINDS:
        path = os.path.join(workdir, kind + ".img")
        with open(path, "wb") as f:
            f.truncate(DISK_SIZE)
            f.seek(SECTOR)
            f.write(("VELORA-DISK-" + kind.upper()).encode())
        paths[kind] = path
    path = os.path.join(workdir, "fs.img")
    with open(path, "wb") as f:
        f.truncate(FS_DISK_SIZE)
    paths["fs"] = path
    return paths


def check_disk_writes(paths):
    """Returns the kinds whose image does not contain the kernel's write."""
    bad = []
    for kind in DISK_KINDS:
        with open(paths[kind], "rb") as f:
            f.seek(2 * SECTOR)
            want = ("VELORA-WRITE-" + kind.upper()).encode()
            if f.read(len(want)) != want:
                bad.append(kind)
    return bad


def build_command(iso, workdir, memory, monitor_port, disks=None):
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
    for kind, path in (disks or {}).items():
        cmd += disk_args(kind, path)
    # Extra QEMU arguments for debugging, e.g. VELORA_QEMU_EXTRA="-trace virtio*".
    cmd += os.environ.get("VELORA_QEMU_EXTRA", "").split()
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


KEY_NAMES = {
    " ": "spc", "\n": "ret", "\t": "tab", ".": "dot", ",": "comma", "-": "minus",
    "=": "equal", "/": "slash", ";": "semicolon", "'": "apostrophe", "`": "grave_accent",
    "[": "bracket_left", "]": "bracket_right", "\\": "backslash",
    "_": "shift-minus", "+": "shift-equal", "?": "shift-slash", ":": "shift-semicolon",
    '"': "shift-apostrophe", "~": "shift-grave_accent", "{": "shift-bracket_left",
    "}": "shift-bracket_right", "|": "shift-backslash", "<": "shift-comma", ">": "shift-dot",
    "!": "shift-1", "@": "shift-2", "#": "shift-3", "$": "shift-4", "%": "shift-5",
    "^": "shift-6", "&": "shift-7", "*": "shift-8", "(": "shift-9", ")": "shift-0",
}


def text_keys(text):
    """QEMU key names for ASCII text (US positions)."""
    out = []
    for c in text:
        if c in KEY_NAMES:
            out.append(KEY_NAMES[c])
        elif c.isupper():
            out.append("shift-" + c.lower())
        else:
            out.append(c)
    return out


class SerialReader:
    """Collects QEMU's stdout (the serial port) in a background thread.
    Complete lines go to `lines`; text after the last newline (a prompt
    such as "login: ") is kept in `partial` and is searched as well."""

    def __init__(self, proc, log_path):
        self.proc = proc
        self.lines = []
        self.partial = ""
        self.start = time.monotonic()
        self.lock = threading.Lock()
        self.log = open(log_path, "w", encoding="utf-8", errors="replace")
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _emit(self, line):
        self.log.write(line + "\n")
        self.log.flush()
        stamp = "%6.2f " % (time.monotonic() - self.start) if TIMESTAMPS else ""
        sys.stdout.write("  | " + stamp + line + "\n")
        sys.stdout.flush()

    def _run(self):
        pending = b""
        while True:
            chunk = self.proc.stdout.read1(4096)
            if not chunk:
                break
            pending += chunk
            while b"\n" in pending:
                raw, pending = pending.split(b"\n", 1)
                line = raw.decode("utf-8", errors="replace").rstrip("\r")
                with self.lock:
                    self.lines.append((time.monotonic() - self.start, line))
                    self.partial = ""
                self._emit(line)
            with self.lock:
                self.partial = pending.decode("utf-8", errors="ignore")
        if pending:
            line = pending.decode("utf-8", errors="replace")
            with self.lock:
                self.lines.append((time.monotonic() - self.start, line))
                self.partial = ""
            self._emit(line)

    def find(self, text):
        with self.lock:
            for t, line in self.lines:
                if text in line:
                    return t
            if text in self.partial:
                return time.monotonic() - self.start
        return None

    def text(self):
        with self.lock:
            return "\n".join([line for _, line in self.lines] + [self.partial])


def host_fsck(path, offset, size, what):
    """Checks one volume of a disk image with velora-fsck (built from this
    repository) and, for FAT, with fsck.fat when it is installed."""
    cmd = ["cargo", "run", "-q", "--release", "-p", "velora-fsck", "--",
           path, "--offset", str(offset), "--size", str(size)]
    r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    for line in r.stdout.splitlines():
        print("  | " + line)
    if r.returncode != 0:
        return "velora-fsck found problems in %s" % what
    fsck_fat = shutil.which("fsck.fat")
    if what == "FAT32" and fsck_fat:
        part = path + ".fat"
        with open(path, "rb") as src, open(part, "wb") as dst:
            src.seek(offset)
            dst.write(src.read(size))
        r = subprocess.run([fsck_fat, "-n", "-v", part], stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT, text=True)
        for line in r.stdout.splitlines()[-4:]:
            print("  | fsck.fat: " + line)
        os.remove(part)
        if r.returncode != 0:
            return "fsck.fat found problems in FAT32"
    elif what == "FAT32":
        print("qemu_run: fsck.fat not installed, FAT32 checked with velora-fsck only")
    return None


# Markers each ktest boot must print besides the common ones.
BOOT_MARKERS = {
    1: [
        "ktest: fs: boot 1: VeloraFS created",
        "vtest: files in /mnt/vd1: 8 passed, 0 failed",
        "vtest: files in /mnt/fat: 8 passed, 0 failed",
    ],
    2: [
        "(journal replayed)",
        "ktest: fs: boot 2: files from boot 1 intact",
        "vtest: files in /mnt/vd1: 8 passed, 0 failed",
        "(files from boot 1 intact)",
    ],
}


def run_once(args, work, disks, log, check):
    port = free_port()
    cmd, accel = build_command(args.iso, work, args.memory, port, disks)
    print("qemu_run: mode=%s accel=%s iso=%s" % (args.mode, accel, args.iso))
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    reader = SerialReader(proc, log)
    monitor = Monitor(port)
    deadline = time.monotonic() + args.timeout
    try:
        return check(proc, reader, monitor, deadline)
    finally:
        monitor.close()
        if proc.poll() is None:
            proc.kill()
        proc.wait()
        reader.thread.join(timeout=5)
        reader.log.close()


def run(args):
    if not os.path.exists(args.iso):
        sys.exit("qemu_run: %s does not exist (run ./build.sh first)" % args.iso)
    os.makedirs(os.path.dirname(args.log) or ".", exist_ok=True)
    with tempfile.TemporaryDirectory() as work:
        if args.mode != "ktest":
            return run_once(args, work, None, args.log,
                            lambda p, r, m, d: check_boot(p, r, m, d, args.max_boot_seconds))
        disks = make_disks(work)
        for boot in (1, 2):
            log = args.log if boot == 1 else args.log.replace(".log", "-boot2.log")
            print("qemu_run: kernel tests, boot %d of 2" % boot)
            result = run_once(args, work, disks, log,
                              lambda p, r, m, d: check_ktest(p, r, m, d, BOOT_MARKERS[boot]))
            if result != 0:
                return result
            bad = check_disk_writes(disks)
            if bad:
                return fail("kernel writes did not reach the disk images: %s" % bad)
            print("qemu_run: disk writes verified on the host: %s" % ", ".join(DISK_KINDS))
            for offset, what in ((0, "VeloraFS"), (FS_HALF, "FAT32")):
                problem = host_fsck(disks["fs"], offset, FS_HALF, what)
                if problem:
                    return fail("after boot %d: %s" % (boot, problem))
            print("qemu_run: VeloraFS and FAT32 checked on the host after boot %d" % boot)
        print("qemu_run: PASS (kernel tests, two boots)")
        return 0


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


def check_ktest(proc, reader, monitor, deadline, extra_markers):
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
        "vtest: 32 passed, 0 failed",
        "ktest: ALL PASSED",
        # The deliberate panic must produce a complete report.
        "KERNEL PANIC",
        "message:  ktest: deliberate panic",
        "location: kernel/src/ktest/mod.rs",
        "backtrace:",
        "ktest: panic path ok",
    ] + extra_markers
    missing = [m for m in required if m not in out.replace("\\", "/")]
    if missing:
        return fail("missing output: %s" % missing)
    if "CPU EXCEPTION" in out:
        return fail("unexpected CPU exception report")
    print("qemu_run: boot passed")
    return 0


class Session:
    """Types into the console and waits for output that appears after the
    previous step (so text from earlier commands does not count)."""

    def __init__(self, proc, reader, monitor, deadline):
        self.proc, self.reader, self.monitor, self.deadline = proc, reader, monitor, deadline
        self.mark = 0

    def lines(self):
        with self.reader.lock:
            return [line for _, line in self.reader.lines[self.mark:]] + [self.reader.partial]

    def wait(self, text, exact=False, timeout=60.0):
        end = min(self.deadline, time.monotonic() + timeout)
        while time.monotonic() < end:
            for i, line in enumerate(self.lines()):
                clean = re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", line).split("\r")[-1]
                if (clean.strip() == text) if exact else (text in line):
                    self.mark += i + 1
                    return True
            if self.proc.poll() is not None:
                return False
            time.sleep(0.05)
        return False

    def type(self, text):
        self.monitor.keys(text_keys(text), delay=0.03)

    def keys(self, names):
        self.monitor.keys(names, delay=0.05)

    def run(self, command, expect, exact=False):
        self.type(command + "\n")
        if not self.wait(expect, exact):
            raise AssertionError("no '%s' after typing: %s" % (expect, command))


def check_boot(proc, reader, monitor, deadline, max_seconds):
    out_bad = ("KERNEL PANIC", "CPU EXCEPTION")
    if not wait_for(proc, reader, "boot complete", deadline):
        return fail("kernel did not report 'boot complete'")
    m = re.search(r"memory in use: (\d+) KiB of (\d+) KiB", reader.text())
    if not m:
        return fail("kernel did not report memory use")
    used_kib = int(m.group(1))
    print("qemu_run: memory in use after boot: %d KiB (%.1f MiB)" % (used_kib, used_kib / 1024))
    if used_kib >= 64 * 1024:
        return fail("idle memory use %d KiB is not below 64 MiB" % used_kib)
    if not wait_for(proc, reader, "velora login:", deadline):
        return fail("no login prompt")
    t = reader.find("velora login:")
    print("qemu_run: login prompt after %.2f s (QEMU start, firmware included)" % t)
    if max_seconds and t > max_seconds:
        return fail("login prompt after %.2f s, limit %.2f s" % (t, max_seconds))

    s = Session(proc, reader, monitor, deadline)
    try:
        # A wrong password is refused.
        s.type("user\n")
        assert s.wait("Password:"), "no password prompt"
        s.type("wrong\n")
        assert s.wait("Login incorrect"), "wrong password accepted"
        assert s.wait("velora login:"), "no second login prompt"
        s.type("user\n")
        assert s.wait("Password:"), "no password prompt"
        s.type("velora\n")
        assert s.wait("user@velora"), "no shell prompt after login"
        print("qemu_run: logged in as user")

        s.run("X=abc; echo $X$X", "abcabc")
        s.run("pwd", "/home/user", exact=True)
        s.run("mkdir -p docs/notes && cd docs/notes && pwd", "/home/user/docs/notes", exact=True)
        s.run("echo first > a.txt; echo second >> a.txt; A=pipe; cat a.txt | cat > b.txt; echo ${A}-ok", "pipe-ok")
        s.run("cat b.txt", "second", exact=True)
        s.run("ls -l", "b.txt")
        s.run("ls /bin", "velofetch")
        s.run("ls -l /etc", "-rw------- ")
        s.run("cat /etc/shadow", "permission denied")
        s.run("nosuchcmd", "command not found")
        s.run("false_status=1; ls /nothing || echo $?$?", "11")
        s.run("date", " UTC")
        s.run("uptime", "processes, boot took")
        s.run("mem", "total:")
        s.run("ps", "PID  PPID")
        s.run("velofetch", "Memory:")
        # Tab completion and history.
        s.type("velof\t\n")
        assert s.wait("Memory:"), "Tab completion did not run velofetch"
        s.keys(["up", "ret"])
        assert s.wait("Memory:"), "history (Up) did not repeat the command"
        # Russian layout.
        s.type("echo ")
        s.keys(["alt-shift"])
        s.type("ghbdtn")
        s.keys(["alt-shift"])
        s.type("\n")
        assert s.wait("привет", exact=True), "Russian input was not echoed"
        # Ctrl+C ends a program that waits for input.
        s.type("cat\n")
        time.sleep(1.0)
        s.keys(["ctrl-c"])
        s.run("echo status-$?", "status-130")
        s.run("exit", "velora login:")
        print("qemu_run: shell commands, pipes, redirection, Tab, history and Ctrl+C work")

        # Root can power the machine off.
        s.type("root\n")
        assert s.wait("Password:"), "no password prompt"
        s.type("velora\n")
        assert s.wait("root@velora"), "no root prompt"
        s.run("whoami", "root", exact=True)
        s.type("shutdown\n")
        end = time.monotonic() + 30
        while proc.poll() is None and time.monotonic() < end:
            time.sleep(0.1)
        assert proc.poll() is not None, "QEMU still running after shutdown"
        print("qemu_run: shutdown powered the machine off")
    except AssertionError as e:
        return fail(str(e))
    out = reader.text()
    for bad in out_bad:
        if bad in out:
            return fail("'%s' in boot log" % bad)
    print("qemu_run: PASS (boot, login and shell)")
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
