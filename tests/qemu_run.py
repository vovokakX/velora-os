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
  net    Boot velora.iso with user-mode networking: DHCP, ping of the
         gateway, a TCP exchange with a server on the host in both
         directions, a refused connection.
  install
         End-to-end installation: boot velora.iso with a blank disk, run
         velora-setup (refused once interactively, then unattended), power
         off, boot from the disk alone, log in as the new user, work with
         files, change a password, reboot again and check the files. The
         partitions are checked on the host after every boot.
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
        "install": "virtio-blk-pci,drive=dinstall,disable-legacy=on" + os.environ.get("VELORA_VIRTIO_OPTS", ""),
        "ahci": "ide-hd,drive=dahci,bus=ide.1",
        "nvme": "nvme,serial=velora0,drive=dnvme",
    }[kind]
    return ["-drive", drive, "-device", device]


USB_STICK_TEXT = "VELORA-USB-STICK\n"


def make_usb_stick(workdir):
    """A 48 MiB USB drive image: GPT with one FAT32 partition holding
    hello.txt (built by tools/mkusb)."""
    src = os.path.join(workdir, "hello.txt")
    with open(src, "w", newline="\n") as f:
        f.write(USB_STICK_TEXT)
    path = os.path.join(workdir, "stick.img")
    subprocess.run(["cargo", "run", "-q", "--release", "-p", "mkusb", "--", "--out", path,
                    "--size-mib", "48", "--file", "hello.txt=" + src], check=True,
                   stdout=subprocess.DEVNULL)
    return path


def usb_args(stick):
    """An xHCI controller with a USB keyboard, mouse and flash drive.
    Key presses typed through the monitor go to the USB keyboard (the
    last keyboard added)."""
    return ["-device", "qemu-xhci,id=xhci",
            "-device", "usb-kbd,bus=xhci.0",
            "-device", "usb-mouse,bus=xhci.0",
            "-drive", "if=none,id=stick,format=raw,file=" + stick,
            "-device", "usb-storage,bus=xhci.0,drive=stick"]


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
    paths["stick"] = make_usb_stick(workdir)
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


def build_command(iso, workdir, memory, monitor_port, disks=None, extra=None):
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
        # Four CPUs by default (VELORA_SMP changes it), as the SMP tests expect.
        "-smp", os.environ.get("VELORA_SMP", "4"),
        "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=" + code,
        "-drive", "if=pflash,format=raw,unit=1,file=" + vars_copy,
        "-serial", "stdio",
        "-display", "none",
        "-no-reboot",
        "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
        # Human monitor on TCP, used to inject key presses.
        "-monitor", "tcp:127.0.0.1:%d,server=on,wait=off" % monitor_port,
    ]
    if iso:
        cmd += ["-cdrom", iso]
    for kind, path in (disks or {}).items():
        if kind != "stick":  # the USB drive comes with usb_args()
            cmd += disk_args(kind, path)
    cmd += extra or []
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


def check_usb_stick(path, boot):
    """The kernel tests write usb.txt on the USB drive's FAT32 partition
    (it starts at 1 MiB)."""
    cmd = ["cargo", "run", "-q", "--release", "-p", "velora-fsck", "--",
           path, "--offset", str(1024 * 1024), "--size", str(46 * 1024 * 1024),
           "--cat", "/usb.txt"]
    r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    text = r.stdout.decode("utf-8", errors="replace")
    want = "written over USB, boot %d" % boot
    if r.returncode != 0 or want not in text:
        return "the USB drive does not hold the file written by the kernel: " + text.strip()[:200]
    return None


def check_panic_log(path):
    """The deliberate panic at the end of the kernel tests must be in the
    panic log on the FAT32 half of the file system disk."""
    cmd = ["cargo", "run", "-q", "--release", "-p", "velora-fsck", "--",
           path, "--offset", str(FS_HALF), "--size", str(FS_HALF),
           "--cat", "/velora-panic.log"]
    r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    text = r.stdout.decode("utf-8", errors="replace")
    if r.returncode != 0:
        return "cannot read the panic log: " + text.strip()
    for want in ("VELORA PANIC LOG", "KERNEL PANIC", "ktest: deliberate panic"):
        if want not in text:
            return "the panic log has no '%s'" % want
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
        "(files and the panic log from boot 1 intact)",
        "the previous boot ended with a panic",
    ],
}


# User-mode networking: the guest gets 10.0.2.15 by DHCP, the gateway
# 10.0.2.2 answers ping and forwards TCP to the host's 127.0.0.1.
def net_args(hostfwd=None):
    netdev = "user,id=n0"
    if hostfwd:
        netdev += ",hostfwd=tcp:127.0.0.1:%d-:%d" % hostfwd
    return ["-netdev", netdev, "-device", "virtio-net-pci,netdev=n0,disable-legacy=on"]


def run_once(args, work, disks, log, check, iso="default", extra=None):
    port = free_port()
    iso = args.iso if iso == "default" else iso
    if extra is None and args.mode == "ktest":
        extra = net_args() + usb_args(disks["stick"])
    cmd, accel = build_command(iso, work, args.memory, port, disks, extra)
    print("qemu_run: mode=%s accel=%s iso=%s" % (args.mode, accel, iso or "none (boot from disk)"))
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
        if args.mode == "install":
            return run_install(args, work)
        if args.mode == "net":
            return run_net(args, work)
        if args.mode == "usb":
            return run_usb(args, work)
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
            problem = check_usb_stick(disks["stick"], boot)
            if problem:
                return fail("after boot %d: %s" % (boot, problem))
            problem = check_panic_log(disks["fs"])
            if problem:
                return fail("after boot %d: %s" % (boot, problem))
            print("qemu_run: panic log of boot %d read on the host" % boot)
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
    mouse_moved = False
    while proc.poll() is None and time.monotonic() < deadline:
        if not keys_sent and reader.find("ktest: keyboard: waiting for input") is not None:
            monitor.keys(KTEST_KEYS)
            keys_sent = True
        if not mouse_moved and reader.find("ktest: mouse: waiting for movement") is not None:
            monitor.command("mouse_move 12 -7")
            monitor.command("mouse_button 1")
            monitor.command("mouse_button 0")
            mouse_moved = True
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
        "vtest: 34 passed, 0 failed",
        "ktest: ALL PASSED",
        # The deliberate panic must produce a complete report.
        "KERNEL PANIC",
        "message:  ktest: deliberate panic",
        "location: kernel/src/ktest/mod.rs",
        "backtrace:",
        "ktest: panic path ok",
        "net0: 10.0.2.15/24 via 10.0.2.2 (DHCP)",
        "usb: storage usb0",
        "usb: keyboard",
        "usb: mouse",
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
        # Search position: line index, and an offset in that line. A match
        # in a complete line moves past the whole line; a match in the
        # unfinished last line (full-screen programs draw without
        # newlines) moves just past the match.
        self.mark = 0
        self.offset = 0

    def lines(self):
        """(index, text, unfinished) of every line from the search position."""
        with self.reader.lock:
            done = self.reader.lines
            out = [(i, done[i][1], False) for i in range(self.mark, len(done))]
            return out + [(len(done), self.reader.partial, True)]

    def wait(self, text, exact=False, timeout=60.0):
        end = min(self.deadline, time.monotonic() + timeout)
        while time.monotonic() < end:
            for i, line, unfinished in self.lines():
                start = self.offset if i == self.mark else 0
                rest = line[start:]
                if exact:
                    clean = re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", rest).split("\r")[-1]
                    found, pos = clean.strip() == text, len(line)
                else:
                    p = rest.find(text)
                    found, pos = p >= 0, start + p + len(text)
                if found:
                    if unfinished:
                        self.mark, self.offset = i, pos
                    else:
                        self.mark, self.offset = i + 1, 0
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


class HostServer:
    """TCP server on 127.0.0.1 for the guest: reads until the end of the
    input, answers "host-reply:<text>" and closes."""

    def __init__(self):
        self.sock = socket.socket()
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(4)
        self.port = self.sock.getsockname()[1]
        self.received = []
        threading.Thread(target=self._run, daemon=True).start()

    def _run(self):
        while True:
            try:
                conn, _ = self.sock.accept()
            except OSError:
                return
            with conn:
                conn.settimeout(20)
                data = b""
                try:
                    while True:
                        chunk = conn.recv(4096)
                        if not chunk:
                            break
                        data += chunk
                    self.received.append(data)
                    conn.sendall(b"host-reply:" + data.strip() + b"\n")
                except OSError:
                    pass

    def close(self):
        self.sock.close()


def connect_to_guest(port, payload, expect, seconds=60):
    """Connects through hostfwd until the guest listens and answers."""
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=5) as c:
                c.sendall(payload)
                c.shutdown(socket.SHUT_WR)
                data = b""
                while True:
                    chunk = c.recv(4096)
                    if not chunk:
                        break
                    data += chunk
                if expect in data:
                    return True
        except OSError:
            pass
        time.sleep(0.5)
    return False


def run_net(args, work):
    server = HostServer()
    fwd = free_port()
    result = {}

    def check(proc, reader, monitor, deadline):
        s = Session(proc, reader, monitor, deadline)
        try:
            assert s.wait("net0: virtio-net"), "no network card found"
            login(s, "root", "velora", "root@velora")
            assert wait_for(proc, reader, "net0: 10.0.2.15/24", deadline), "DHCP did not configure 10.0.2.15"
            s.run("ip", "inet 10.0.2.15/24")
            s.run("ping -c 3 10.0.2.2", "3 packets transmitted, 3 received")
            print("qemu_run: DHCP and ping work")
            s.run("echo hello-velora | nc 10.0.2.2 %d" % server.port, "host-reply:hello-velora")
            assert b"hello-velora" in b"".join(server.received), "the host server got nothing"
            print("qemu_run: guest-to-host TCP works")
            s.type("nc -l 7000 velora-ack\n")
            ok = connect_to_guest(fwd, b"from-host-data\n", b"velora-ack")
            assert ok, "the guest did not answer a connection from the host"
            assert s.wait("from-host-data", exact=True), "the guest did not print the host's data"
            print("qemu_run: host-to-guest TCP works")
            # QEMU's user network drops a SYN to a closed host port instead
            # of answering RST, so this ends with "timed out" (RST handling
            # is covered by the stack's unit tests).
            s.type("nc 10.0.2.2 1 < /dev/null\n")
            assert s.wait("nc: 10.0.2.2:1: ", timeout=40), "no error for a closed port"
            s.run("ip", "frames received")
            s.type("shutdown\n")
            wait_exit(proc)
        except AssertionError as e:
            return fail(str(e))
        result["ok"] = True
        return 0

    try:
        r = run_once(args, work, None, args.log, check, extra=net_args((fwd, 7000)))
    finally:
        server.close()
    if r == 0:
        print("qemu_run: PASS (network: DHCP, ping, TCP both ways)")
    return r


def run_usb(args, work):
    """Boots velora-usb.img the way a real PC boots a USB drive: the image is
    a USB mass storage device on an xHCI controller and there is no CD.
    The machine has no HPET, so the kernel must calibrate its timers
    against the ACPI PM timer."""
    if not os.path.exists(args.usb):
        sys.exit("qemu_run: %s does not exist (run ./build.sh first)" % args.usb)
    image = os.path.join(work, "usb.img")
    shutil.copyfile(args.usb, image)
    extra = [
        "-machine", "hpet=off",
        "-device", "qemu-xhci,id=xhci",
        "-drive", "if=none,id=stick,format=raw,file=" + image,
        "-device", "usb-storage,bus=xhci.0,drive=stick,bootindex=0",
    ]

    def check(proc, reader, monitor, deadline):
        s = Session(proc, reader, monitor, deadline)
        try:
            assert s.wait("calibrated against the ACPI PM timer"), \
                "the timers were not calibrated against the ACPI PM timer"
            # The drive the system booted from is seen over USB, and its
            # EFI partition (with /boot/velora) gets the panic log.
            assert s.wait("usb: storage usb0"), "the USB drive was not found"
            assert s.wait("panic log: /mnt/usb0p1/velora-panic.log"), \
                "no panic log on the USB drive"
            login(s, "user", "velora", "user@velora")
            print("qemu_run: booted from the USB image: login prompt after %.2f s"
                  % reader.find("velora login:"))
            # The kernel log, saved and then printed (markers that the
            # typed commands do not contain).
            s.run("dmesg > /tmp/k.txt; ls -l /tmp/k.txt", "k.txt")
            s.run("cat /tmp/k.txt", "calibrated against the ACPI PM timer")
            s.run("ls /dev", "kmsg", exact=True)
            s.run("ls /mnt/usb0p1/boot", "initramfs.tar", exact=True)
            s.type("exit\n")
            login(s, "root", "velora", "root@velora")
            # QEMU runs with -no-reboot, so a restart ends it.
            s.type("reboot\n")
            assert s.wait("Velora OS: restarting"), "reboot did not start"
            wait_exit(proc)
        except AssertionError as e:
            return fail(str(e))
        return 0

    r = run_once(args, work, None, args.log, check, iso=None, extra=extra)
    if r == 0:
        print("qemu_run: PASS (boot from the USB image, USB drive seen, PM timer, dmesg, reboot)")
    return r


INSTALL_DISK_SIZE = 256 * 1024 * 1024


def gpt_partitions(path):
    """(first_lba, last_lba) of every partition in the image's primary GPT."""
    import struct
    with open(path, "rb") as f:
        f.seek(SECTOR)
        h = f.read(SECTOR)
        if h[:8] != b"EFI PART":
            return []
        entries_lba, count, size = struct.unpack_from("<QII", h, 72)
        f.seek(entries_lba * SECTOR)
        data = f.read(count * size)
    parts = []
    for i in range(count):
        e = data[i * size:(i + 1) * size]
        if e[:16] != bytes(16):
            first, last = struct.unpack_from("<QQ", e, 32)
            parts.append((first, last))
    return parts


def check_installed_disk(path, after):
    parts = gpt_partitions(path)
    if len(parts) != 2:
        return "expected 2 GPT partitions after %s, found %d" % (after, len(parts))
    for (first, last), what in zip(parts, ("FAT32", "VeloraFS")):
        problem = host_fsck(path, first * SECTOR, (last + 1 - first) * SECTOR, what)
        if problem:
            return "after %s: %s" % (after, problem)
    print("qemu_run: EFI partition and root file system checked on the host after %s" % after)
    return None


def login(s, user, password, prompt):
    assert s.wait("velora login:"), "no login prompt"
    s.type(user + "\n")
    assert s.wait("Password:"), "no password prompt"
    s.type(password + "\n")
    assert s.wait(prompt), "no prompt '%s' after logging in as %s" % (prompt, user)


def wait_exit(proc, seconds=60):
    end = time.monotonic() + seconds
    while proc.poll() is None and time.monotonic() < end:
        time.sleep(0.1)
    assert proc.poll() is not None, "QEMU still running after shutdown"


def install_step1(proc, reader, monitor, deadline):
    """Live system: refuse once interactively, then install unattended."""
    s = Session(proc, reader, monitor, deadline)
    try:
        login(s, "root", "velora", "root@velora")
        s.run("df", "initramfs")
        # Interactive run, answered "no" at the confirmation.
        s.type("velora-setup\n")
        assert s.wait("Disk to install on:"), "installer did not ask for a disk"
        s.type("vd0\n")
        assert s.wait("Name of the new user"), "installer did not ask for a user"
        s.type("anna\n")
        for _ in range(2):
            assert s.wait("Password for"), "installer did not ask for a password"
            s.type("secret\n")
            assert s.wait("Repeat the password:"), "no password repeat prompt"
            s.type("secret\n")
        assert s.wait("Type 'yes' to erase vd0"), "no confirmation question"
        s.type("no\n")
        assert s.wait("Nothing was changed."), "refusal not confirmed"
        print("qemu_run: interactive installer refused without changes")
        s.type("SETUP_ROOT_PASSWORD=rootpw1 SETUP_USER_PASSWORD=annapw1 "
               "velora-setup --auto --disk vd0 --user anna --yes\n")
        if not s.wait("Installation complete.", timeout=300):
            raise AssertionError("installation did not complete")
        print("qemu_run: unattended installation complete")
        s.type("shutdown\n")
        wait_exit(proc)
    except AssertionError as e:
        return fail(str(e))
    return 0


def install_step2(proc, reader, monitor, deadline):
    """First boot from the disk: log in, work with files, change a password."""
    s = Session(proc, reader, monitor, deadline)
    try:
        assert s.wait("root: VeloraFS on vd0p2"), "root was not mounted from the disk"
        assert s.wait("velora login:"), "no login prompt"
        print("qemu_run: installed system: login prompt after %.2f s" % reader.find("velora login:"))
        # The Live password no longer works.
        s.type("root\n")
        assert s.wait("Password:"), "no password prompt"
        s.type("velora\n")
        assert s.wait("Login incorrect"), "the Live password was accepted"
        login(s, "anna", "annapw1", "anna@velora")
        s.run("df", "velorafs")
        s.run("pwd", "/home/anna", exact=True)
        s.run("X=keep; echo ${X}-me > notes.txt; mkdir projects; cp notes.txt projects/copy.txt; cat projects/copy.txt",
              "keep-me", exact=True)
        s.run("ls /home", "anna", exact=True)
        s.run("cat /etc/shadow", "permission denied")
        s.run("velofetch", "Memory:")
        s.type("exit\n")  # login() waits for the prompt
        login(s, "root", "rootpw1", "root@velora")
        s.type("passwd anna\n")
        assert s.wait("New password for anna:"), "passwd did not ask"
        s.type("annapw2\n")
        assert s.wait("Repeat the password:"), "passwd did not ask again"
        s.type("annapw2\n")
        assert s.wait("password for anna changed"), "password not changed"
        s.type("shutdown\n")
        wait_exit(proc)
    except AssertionError as e:
        return fail(str(e))
    return 0


def install_step3(proc, reader, monitor, deadline):
    """Second boot from the disk: the files and the new password persist."""
    s = Session(proc, reader, monitor, deadline)
    try:
        login(s, "anna", "annapw2", "anna@velora")
        s.run("cat notes.txt", "keep-me", exact=True)
        s.run("cat projects/copy.txt", "keep-me", exact=True)
        s.run("ls -l", "projects")
        s.type("exit\n")  # login() waits for the prompt
        login(s, "root", "rootpw1", "root@velora")
        s.type("shutdown\n")
        wait_exit(proc)
    except AssertionError as e:
        return fail(str(e))
    return 0


def run_install(args, work):
    disk = os.path.join(work, "install.img")
    with open(disk, "wb") as f:
        f.truncate(INSTALL_DISK_SIZE)
    disks = {"install": disk}
    base = args.log.replace(".log", "")
    steps = [
        ("installation from the ISO", install_step1, args.iso),
        ("first boot from the disk", install_step2, None),
        ("second boot from the disk", install_step3, None),
    ]
    for n, (what, check, iso) in enumerate(steps, 1):
        print("qemu_run: install test, step %d of 3: %s" % (n, what))
        result = run_once(args, work, disks, "%s-%d.log" % (base, n), check, iso=iso)
        if result != 0:
            return result
        problem = check_installed_disk(disk, what)
        if problem:
            return fail(problem)
    print("qemu_run: PASS (install, boot from disk, login, files persist)")
    return 0


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
        # Plume: a new file with Cyrillic text, saved on exit after the
        # question; then search, cut, paste and Ctrl+S.
        s.type("plume note.txt\n")
        assert s.wait("New file"), "plume did not start"
        s.type("first line\n")
        s.keys(["alt-shift"])
        s.type("ghbdtn")
        s.keys(["alt-shift"])
        s.type(" 42\n")
        s.keys(["ctrl-x"])
        assert s.wait("Save the changes"), "plume did not ask to save the changes"
        s.type("y")
        s.run("cat note.txt", "привет 42", exact=True)
        s.type("plume note.txt\n")
        assert s.wait("Read 3 lines"), "plume did not open the file"
        s.keys(["ctrl-w"])
        s.type("42\n")
        s.keys(["ctrl-k", "up", "ctrl-u", "ctrl-s"])
        assert s.wait("Wrote 3 lines to note.txt"), "plume did not save with Ctrl+S"
        s.keys(["ctrl-x"])
        s.run("cat note.txt", "привет 42", exact=True)
        assert s.wait("first line", exact=True), "cut and paste gave the wrong order"
        print("qemu_run: plume created, edited and saved a file")
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
        # The ACPI power button (QEMU raises the fixed PWRBTN event).
        s.monitor.command("system_powerdown")
        assert s.wait("Power button pressed"), "the power button was not noticed"
        end = time.monotonic() + 30
        while proc.poll() is None and time.monotonic() < end:
            time.sleep(0.1)
        assert proc.poll() is not None, "QEMU still running after the power button"
        print("qemu_run: the power button powered the machine off")
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
    p.add_argument("mode", choices=["ktest", "boot", "install", "net", "usb"])
    p.add_argument("--iso")
    p.add_argument("--usb", default="velora-usb.img", help="image for the usb mode")
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
