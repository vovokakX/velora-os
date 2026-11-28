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
         log out, log in as root and power off with `shutdown`. velofetch
         is checked on a screenshot (the logo is an image) and through a
         pipe (ASCII logo); the screenshot is kept in build/logs.
  gui    The start menu and every screen of the graphical installer with
         the keyboard, installation, boot from the disk (--resolution,
         --screens, --screens-only). --disk-type virtio|sata|nvme connects
         the disk; --reinstall then boots the Live system again with the
         installed (and mounted) disk, installs over it and boots it.
  clicks Boot velora-uitest.iso (./build.sh --uitest) with a USB tablet:
         every interactive element of the graphical installer is clicked
         through QMP and must react (tests/uiclick.py).
  fps    Boot velora-uitest.iso with a USB tablet, turn the frame counter
         on (F12) and print frames per second while the pointer moves and
         while screens change (--resolution).

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
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib

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


# How the install disk is connected (the gui mode's --disk-type) and the
# name the kernel gives it.
INSTALL_BUS = {
    "virtio": ("virtio-blk-pci,drive=dinstall,disable-legacy=on", "vd0"),
    "sata": ("ide-hd,drive=dinstall,bus=ide.1", "sd0"),
    "nvme": ("nvme,serial=velora1,drive=dinstall", "nvme0n1"),
}
install_bus = "virtio"


def disk_args(kind, path):
    drive = "if=none,id=d%s,file=%s,format=raw" % (kind, path)
    virtio_opts = os.environ.get("VELORA_VIRTIO_OPTS", "")
    device = {
        "virtio": "virtio-blk-pci,drive=dvirtio,disable-legacy=on" + virtio_opts,
        "fs": "virtio-blk-pci,drive=dfs,disable-legacy=on" + virtio_opts,
        "install": INSTALL_BUS[install_bus][0] + (virtio_opts if install_bus == "virtio" else ""),
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
def net_args(hostfwd=None, nic="virtio-net-pci,disable-legacy=on"):
    netdev = "user,id=n0"
    if hostfwd:
        netdev += ",hostfwd=tcp:127.0.0.1:%d-:%d" % hostfwd
    return ["-netdev", netdev, "-device", nic + ",netdev=n0"]


# The net test uses an Intel card (virtio-net is covered by ktest):
# e1000 (82540EM) by default, VELORA_NET_NIC=e1000e for the 82574L.
NET_NIC = os.environ.get("VELORA_NET_NIC", "e1000")
NET_NIC_NAME = {"e1000": "82540EM", "e1000e": "82574L"}.get(NET_NIC, "")

HTTP_FILE = b"Velora fetch test file\n" + b"0123456789" * 2000 + b"\nEND-OF-FILE\n"


class HostDns:
    """A tiny DNS server on the host for the net test: velora.test and
    www.velora.test (a CNAME) are 10.0.2.2, everything else does not
    exist. The guest reaches it at 10.0.2.2:port."""

    def __init__(self):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(("127.0.0.1", 0))
        self.port = self.sock.getsockname()[1]
        self.queries = []
        threading.Thread(target=self._run, daemon=True).start()

    @staticmethod
    def _name(q):
        labels, i = [], 12
        while q[i]:
            labels.append(q[i + 1:i + 1 + q[i]].decode().lower())
            i += 1 + q[i]
        return ".".join(labels), i + 5

    def _run(self):
        while True:
            try:
                q, addr = self.sock.recvfrom(512)
            except OSError:
                return
            name, end = self._name(q)
            self.queries.append(name)
            head = q[:2] + b"\x81\x80" + q[4:6]
            question = q[12:end]
            if name in ("velora.test", "www.velora.test"):
                rr = b"\xc0\x0c\x00\x01\x00\x01\x00\x00\x00\x3c\x00\x04" + bytes([10, 0, 2, 2])
                msg = head + b"\x00\x01\x00\x00\x00\x00" + question + rr
            else:
                msg = q[:2] + b"\x81\x83" + q[4:6] + b"\x00\x00\x00\x00\x00\x00" + question
            self.sock.sendto(msg, addr)

    def close(self):
        self.sock.close()


class HostHttp:
    """An HTTP server on the host: /file.txt, and /old redirects to it."""

    def __init__(self):
        import http.server

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path == "/old":
                    self.send_response(302)
                    self.send_header("Location", "/file.txt")
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                elif self.path == "/file.txt":
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(HTTP_FILE)))
                    self.end_headers()
                    self.wfile.write(HTTP_FILE)
                else:
                    self.send_error(404)

            def log_message(self, *a):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()


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
        if args.mode == "gui":
            return run_gui(args, work)
        if args.mode == "clicks":
            return run_clicks(args, work)
        if args.mode == "fps":
            return run_fps(args, work)
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
            assert s.wait("net0: " + NET_NIC_NAME), "the %s card was not found" % NET_NIC
            choose_live(s)
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
            # DNS, HTTP and names, with servers on the host.
            dns = "10.0.2.2:%d" % hdns.port
            s.run("host velora.test %s" % dns, "velora.test has address 10.0.2.2")
            s.run("host nothing.test %s" % dns, "host: nothing.test: no such name")
            s.run("fetch -q http://10.0.2.2:%d/old | cat" % hhttp.port, "END-OF-FILE", exact=True)
            s.run("DNS_SERVER=%s fetch -o /tmp/f.txt http://www.velora.test:%d/file.txt" % (dns, hhttp.port),
                  "fetch: %d bytes from www.velora.test:%d/file.txt" % (len(HTTP_FILE), hhttp.port))
            s.run("cat /tmp/f.txt", "Velora fetch test file", exact=True)
            s.run("fetch http://10.0.2.2:%d/missing" % hhttp.port, "fetch: HTTP 404")
            s.run("fetch https://velora.test/", "only http:// is supported")
            s.run("DNS_SERVER=%s ping -c 1 velora.test" % dns, "1 packets transmitted, 1 received")
            assert "www.velora.test" in hdns.queries, "the DNS server got no query"
            print("qemu_run: DNS, fetch (redirect, file, errors) and ping by name work")
            s.type("shutdown\n")
            wait_exit(proc)
        except AssertionError as e:
            return fail(str(e))
        result["ok"] = True
        return 0

    hdns = HostDns()
    hhttp = HostHttp()
    try:
        r = run_once(args, work, None, args.log, check,
                     extra=net_args((fwd, 7000), nic=NET_NIC))
    finally:
        server.close()
        hdns.close()
        hhttp.close()
    if r == 0:
        print("qemu_run: PASS (network on %s: DHCP, ping, TCP both ways, DNS, fetch)" % NET_NIC)
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
            assert s.wait("boot disk: usb0"), "the drive the system started from was not recognised"
            assert s.wait("panic log: /mnt/usb0p1/velora-panic.log"), \
                "no panic log on the USB drive"
            choose_live(s)
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
            # The installer does not erase the drive the system runs from.
            s.run("SETUP_ROOT_PASSWORD=x1 SETUP_USER_PASSWORD=x2 "
                  "velora-setup --auto --disk usb0 --user anna --yes",
                  "the system was started from usb0; it cannot be erased")
            print("qemu_run: the installer refused to erase the boot drive")
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


def choose_live(s):
    """The Live image starts with the start menu (velora-start); key 3
    chooses the Live system and the login prompt follows."""
    assert s.wait("velora-start: menu", timeout=120), "no start menu"
    s.keys(["3"])
    assert s.wait("velora-start: live", timeout=30), "the start menu did not choose the Live system"


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
        choose_live(s)
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


def gui_screen(s, monitor, shots, name, wait=True):
    """Waits for an installer screen, takes its screenshot."""
    if wait:
        assert s.wait("velora-start: screen " + name, timeout=120), "no installer screen '%s'" % name
    time.sleep(1.0)
    shots.append(name)
    path = os.path.join(shots.dir, "%02d-%s.png" % (len(shots) - 1, name))
    w, h, rgb = screendump(monitor, path + ".ppm")
    write_png(path, w, h, rgb)
    os.remove(path + ".ppm")
    share = share_near(w, h, rgb, UI_WINDOW, tol=12)
    assert share > 0.1 or name in ("done", "start"), \
        "screen '%s' does not show the installer window (%.2f)" % (name, share)
    return w, h


class Shots(list):
    def __init__(self, directory):
        super().__init__()
        self.dir = directory
        os.makedirs(directory, exist_ok=True)


def gui_step1(args, shots):
    """Live system: the start menu, every installer screen with the
    keyboard (and the mouse on the disk screen), the installation."""
    def check(proc, reader, monitor, deadline):
        s = Session(proc, reader, monitor, deadline)
        try:
            assert s.wait("velora-start: menu", timeout=120), "no start menu"
            gui_screen(s, monitor, shots, "start", wait=False)
            s.keys(["1"])
            assert s.wait("velora-start: chose graphical"), "graphical installation not chosen"
            gui_screen(s, monitor, shots, "welcome")
            # Enter with nothing focused goes on; Continue keeps the focus.
            s.keys(["ret"])
            gui_screen(s, monitor, shots, "location")
            s.keys(["ret"])
            gui_screen(s, monitor, shots, "keyboard")
            # Tab order: theme switch, layout, shortcuts, pointer styles...
            # The blade pointer: Tab x4 to the first style, Right.
            s.keys(["tab", "tab", "tab", "tab", "right"])
            s.keys(["tab", "tab", "tab", "tab", "ret"])
            gui_screen(s, monitor, shots, "appearance")
            s.keys(["ret"])
            gui_screen(s, monitor, shots, "disk")
            # Expert mode with the keyboard: Tab to the radio group, Down x2,
            # then Tab past Refresh and Back to Continue.
            s.keys(["tab", "tab", "down", "down", "tab", "tab", "tab", "ret"])
            gui_screen(s, monitor, shots, "partitions")
            # A 128 MiB EFI partition: Tab to the first list, Down.
            s.keys(["tab", "tab", "down", "tab", "tab", "tab", "tab", "ret"])
            gui_screen(s, monitor, shots, "user")
            s.keys(["tab", "tab"])
            s.type("anna")
            s.keys(["tab"])
            s.type("annapw1")
            s.keys(["tab"])
            s.type("annapw1")
            s.keys(["tab"])
            s.type("rootpw1")
            s.keys(["tab"])
            s.type("rootpw1")
            s.keys(["tab", "tab", "ret"])
            gui_screen(s, monitor, shots, "confirm")
            s.keys(["ret"])
            assert s.wait("velora-start: installing on " + INSTALL_BUS[install_bus][1], timeout=60), \
                "installation did not start"
            assert s.wait("velora-start: phase CopyFiles", timeout=120), "no progress"
            gui_screen(s, monitor, shots, "installing", wait=False)
            if not s.wait("velora-start: installed", timeout=600):
                raise AssertionError("the graphical installation did not finish")
            gui_screen(s, monitor, shots, "done")
            print("qemu_run: graphical installation complete")
            s.keys(["ret"])
            assert s.wait("velora-start: restarting", timeout=30), "Restart did not work"
            wait_exit(proc)
        except AssertionError as e:
            if proc.poll() is None:
                path = os.path.join(os.path.dirname(reader.log.name) or ".", "gui-failure.png")
                w, h, rgb = screendump(monitor, path + ".ppm")
                write_png(path, w, h, rgb)
                os.remove(path + ".ppm")
                print("qemu_run: screen at the failure: %s" % path)
            return fail(str(e))
        return 0
    return check


def disk_partition(n):
    """Name of partition n of the install disk (vd0p2, nvme0n1p2)."""
    return "%sp%d" % (INSTALL_BUS[install_bus][1], n)


def gui_step2(proc, reader, monitor, deadline):
    """Boot from the disk: the user and the settings of the installer."""
    s = Session(proc, reader, monitor, deadline)
    try:
        assert s.wait("root: VeloraFS on " + disk_partition(2)), "root was not mounted from the disk"
        login(s, "anna", "annapw1", "anna@velora")
        s.run("cat /etc/velora/desktop.conf", "pointer=blade")
        s.run("cat /etc/velora/locale.conf", "region=UA")
        s.run("cat /etc/velora/keyboard.conf", "layouts=ru,us")
        s.type("exit\n")
        login(s, "root", "rootpw1", "root@velora")
        s.type("shutdown\n")
        wait_exit(proc)
    except AssertionError as e:
        return fail(str(e))
    return 0


def gui_reinstall(proc, reader, monitor, deadline):
    """The Live system with the installed disk attached: the kernel mounts
    its partitions at boot; the installer offers the disk anyway,
    unmounts them and installs again (another user)."""
    s = Session(proc, reader, monitor, deadline)
    disk = INSTALL_BUS[install_bus][1]
    try:
        assert s.wait("velorafs on %s mounted at /mnt/%s" % (disk_partition(2), disk_partition(2)),
                      timeout=120), "the installed disk was not mounted at boot"
        assert s.wait("velora-start: menu", timeout=120), "no start menu"
        s.keys(["1"])
        assert s.wait("velora-start: screen welcome"), "no welcome screen"
        for screen in ("location", "keyboard", "appearance", "disk", "user"):
            s.keys(["ret"])
            assert s.wait("velora-start: screen " + screen), "no %s screen" % screen
        s.keys(["tab", "tab"])
        s.type("boris")
        for text in ("borispw1", "borispw1", "rootpw2", "rootpw2"):
            s.keys(["tab"])
            s.type(text)
        s.keys(["tab", "tab", "ret"])
        assert s.wait("velora-start: screen confirm"), "no confirmation screen"
        s.keys(["ret"])
        assert s.wait("velora-start: installing on " + disk, timeout=60), \
            "the mounted disk was not offered or the installation did not start"
        assert s.wait("/mnt/%s unmounted" % disk_partition(2), timeout=60), \
            "the partitions of the disk were not unmounted"
        if not s.wait("velora-start: installed", timeout=600):
            raise AssertionError("the second installation did not finish")
        print("qemu_run: installed again over the mounted disk")
        s.keys(["ret"])
        assert s.wait("velora-start: restarting", timeout=30), "Restart did not work"
        wait_exit(proc)
    except AssertionError as e:
        return fail(str(e))
    return 0


def gui_step4(proc, reader, monitor, deadline):
    """Boot from the disk installed again: the new user logs in."""
    s = Session(proc, reader, monitor, deadline)
    try:
        assert s.wait("root: VeloraFS on " + disk_partition(2)), "root was not mounted from the disk"
        login(s, "boris", "borispw1", "boris@velora")
        s.type("exit\n")
        login(s, "root", "rootpw2", "root@velora")
        s.type("shutdown\n")
        wait_exit(proc)
    except AssertionError as e:
        return fail(str(e))
    return 0


def gui_screens_only(args, shots):
    """Every screen up to the confirmation at another resolution, then
    power off without installing."""
    def check(proc, reader, monitor, deadline):
        s = Session(proc, reader, monitor, deadline)
        try:
            assert s.wait("velora-start: menu", timeout=120), "no start menu"
            gui_screen(s, monitor, shots, "start", wait=False)
            s.keys(["1"])
            for name in ("welcome", "location", "keyboard", "appearance"):
                gui_screen(s, monitor, shots, name)
                s.keys(["ret"])
            gui_screen(s, monitor, shots, "disk")
            # Manual mode: Tab to the radio group, Down; the radio buttons
            # are separate stops, so four Tabs (past Refresh and Back)
            # reach Continue.
            s.keys(["tab", "tab", "down", "tab", "tab", "tab", "tab", "ret"])
            gui_screen(s, monitor, shots, "partitions")
            s.keys(["esc"])
            assert s.wait("velora-start: screen disk"), "Escape did not go back"
            print("qemu_run: screens at %s checked; powering off" % args.resolution)
            monitor.command("system_powerdown")
            wait_exit(proc)
        except AssertionError as e:
            return fail(str(e))
        return 0
    return check


def run_gui(args, work):
    global install_bus
    install_bus = args.disk_type
    w, h = (int(v) for v in args.resolution.split("x"))
    display = ["-vga", "none", "-device", "VGA,xres=%d,yres=%d,vgamem_mb=64" % (w, h)]
    shots = Shots(os.path.join(args.screens, args.resolution))
    base = args.log.replace(".log", "")
    disk = os.path.join(work, "install.img")
    with open(disk, "wb") as f:
        f.truncate(INSTALL_DISK_SIZE)
    disks = {"install": disk}
    if args.screens_only:
        r = run_once(args, work, disks, base + "-1.log", gui_screens_only(args, shots), extra=display)
        if r == 0:
            print("qemu_run: PASS (graphical installer screens at %s: %s)" % (args.resolution, shots.dir))
        return r
    steps = 4 if args.reinstall else 2
    print("qemu_run: graphical install, step 1 of %d: installation at %s on a %s disk (%s)"
          % (steps, args.resolution, args.disk_type, INSTALL_BUS[install_bus][1]))
    r = run_once(args, work, disks, base + "-1.log", gui_step1(args, shots), extra=display)
    if r != 0:
        return r
    problem = check_installed_disk(disk, "the graphical installation")
    if problem:
        return fail(problem)
    parts = gpt_partitions(disk)
    esp_mib = (parts[0][1] + 1 - parts[0][0]) * SECTOR // (1024 * 1024)
    if esp_mib != 128:
        return fail("the EFI partition is %d MiB, the installer was told 128" % esp_mib)
    print("qemu_run: the expert layout was written (EFI partition 128 MiB)")
    print("qemu_run: graphical install, step 2 of %d: boot from the disk" % steps)
    r = run_once(args, work, disks, base + "-2.log", gui_step2, iso=None, extra=display)
    if r != 0:
        return r
    if args.reinstall:
        print("qemu_run: graphical install, step 3 of 4: the Live system again, install over the mounted disk")
        r = run_once(args, work, disks, base + "-3.log", gui_reinstall, extra=display)
        if r != 0:
            return r
        problem = check_installed_disk(disk, "the second installation")
        if problem:
            return fail(problem)
        print("qemu_run: graphical install, step 4 of 4: boot from the disk installed again")
        r = run_once(args, work, disks, base + "-4.log", gui_step4, iso=None, extra=display)
        if r != 0:
            return r
    print("qemu_run: PASS (graphical installation on a %s disk, %d screenshots in %s, boot from disk%s)"
          % (args.disk_type, len(shots), shots.dir, ", installed again" if args.reinstall else ""))
    return r


def run_clicks(args, work):
    """Every interactive element of the graphical installer, clicked with
    a USB tablet through QMP; each must react (tests/uiclick.py). Needs
    velora-uitest.iso (./build.sh --uitest)."""
    import uiclick
    w, h = (int(v) for v in args.resolution.split("x"))
    qmp_port = free_port()
    extra = ["-vga", "none", "-device", "VGA,xres=%d,yres=%d,vgamem_mb=64" % (w, h),
             "-device", "qemu-xhci,id=xhci", "-device", "usb-tablet,bus=xhci.0",
             "-qmp", "tcp:127.0.0.1:%d,server=on,wait=off" % qmp_port]
    disk = os.path.join(work, "install.img")
    with open(disk, "wb") as f:
        f.truncate(INSTALL_DISK_SIZE)

    def check(proc, reader, monitor, deadline):
        s = Session(proc, reader, monitor, deadline)
        qmp = uiclick.Qmp(qmp_port)
        try:
            assert s.wait("usb: tablet", timeout=60), "the USB tablet was not found"
            assert s.wait("velora-start: menu", timeout=120), "no start menu"
            qmp.connect()
            s.keys(["1"])
            assert s.wait("velora-start: chose graphical"), "graphical installation not chosen"
            n = uiclick.walk(s, qmp, (w, h))
            print("qemu_run: %d interactive elements reacted to the mouse" % n)
            monitor.command("system_powerdown")
            wait_exit(proc)
        except AssertionError as e:
            if proc.poll() is None:
                path = os.path.join(os.path.dirname(reader.log.name) or ".", "clicks-failure.png")
                fw, fh, rgb = screendump(monitor, path + ".ppm")
                write_png(path, fw, fh, rgb)
                os.remove(path + ".ppm")
                print("qemu_run: screen at the failure: %s" % path)
            return fail(str(e))
        finally:
            qmp.close()
        return 0

    r = run_once(args, work, {"install": disk}, args.log, check, extra=extra)
    if r == 0:
        print("qemu_run: PASS (every element of the installer reacted to clicks at %s)" % args.resolution)
    return r


FPS_LINE = re.compile(r"velora-start: fps ([\d.]+) fps\s+([\d.]+) ms\s+max ([\d.]+) ms\s+pointer ([\d.]+)/s"
                      r"\s+input ([\d.]+)/s(?:\s+anim ([\d.]+) fps\s+gap max ([\d.]+) ms)?")


def last_areas(reader):
    """The interactive areas of the last --uitest report."""
    import uiclick
    with reader.lock:
        lines = [l for _, l in reader.lines]
    for line in reversed(lines):
        m = uiclick.REPORT.search(line)
        if m:
            return uiclick.Report(m).areas
    return {}


def run_fps(args, work):
    """Frame rate of the graphical installer: the counter (F12) is turned
    on, then the pointer circles over the window (a USB tablet through
    QMP), and then screens change back and forth while it moves. Prints
    frames per second, the average and longest frame, pointer moves shown
    and pointer reports read per second for each part. Needs
    velora-uitest.iso (its reports tell where the buttons are).

    Checks: the pointer is drawn at most once a frame (60/s); the xHCI
    controller uses MSI-X and the tablet's reports are read faster than
    the 100 Hz timer (QMP sends 250 positions a second). With
    --xhci-polled QEMU's xHCI has no MSI or MSI-X: the controller must be
    polled and the pointer must still move (D51)."""
    import math
    import uiclick
    w, h = (int(v) for v in args.resolution.split("x"))
    qmp_port = free_port()
    extra = ["-vga", "none", "-device", "VGA,xres=%d,yres=%d,vgamem_mb=64" % (w, h),
             # nec-usb-xhci has had the msi and msix switches for long;
             # qemu-xhci only since QEMU 10 (CI runs 9.1).
             "-device", ("nec-usb-xhci,id=xhci,msi=off,msix=off" if args.xhci_polled
                         else "qemu-xhci,id=xhci"),
             "-device", "usb-tablet,bus=xhci.0",
             "-qmp", "tcp:127.0.0.1:%d,server=on,wait=off" % qmp_port]
    disk = os.path.join(work, "install.img")
    with open(disk, "wb") as f:
        f.truncate(INSTALL_DISK_SIZE)
    rates = {}

    def circle(qmp, seconds, rate, step=None):
        t0 = time.monotonic()
        n = 0
        while True:
            t = time.monotonic() - t0
            if t >= seconds:
                break
            a = t * 2.5
            qmp.move(int(w / 2 + w * 0.3 * math.cos(a)), int(h / 2 + h * 0.3 * math.sin(a)), w, h)
            if step:
                step(t)
            n += 1
            time.sleep(max(0.0, n / rate - (time.monotonic() - t0)))

    def phase(reader, name, run):
        start = time.monotonic() - reader.start
        run()
        end = time.monotonic() - reader.start
        time.sleep(1.2)
        with reader.lock:
            lines = [l for t, l in reader.lines if start + 1.0 < t <= end + 0.2]
        vals = [m.groups() for m in map(FPS_LINE.search, lines) if m]
        if not vals:
            raise AssertionError("no frame counter lines during '%s'" % name)
        n = len(vals)
        avg = [sum(float(v[i]) for v in vals) / n for i in range(5)]
        worst = max(float(v[2]) for v in vals)
        rates[name] = (avg[3], avg[4], max(float(v[3]) for v in vals))
        anim = [(float(v[5]), float(v[6])) for v in vals if v[5]]
        anim_text = "anim --"
        if anim:
            anim_text = "anim %4.1f fps  gap max %4.1f ms" % (
                sum(a for a, _ in anim) / len(anim), max(g for _, g in anim))
        print("qemu_run: fps %-12s %5.1f fps  frame %5.1f ms  max %5.1f ms  pointer %4.0f/s  input %4.0f/s  %s  (%d s)"
              % (name, avg[0], avg[1], worst, avg[3], avg[4], anim_text, n))

    def check(proc, reader, monitor, deadline):
        s = Session(proc, reader, monitor, deadline)
        qmp = uiclick.Qmp(qmp_port)
        try:
            mode = "polled" if args.xhci_polled else "MSI-X"
            assert s.wait("ports, " + mode, timeout=60), "the xHCI controller is not " + mode
            assert s.wait("usb: tablet", timeout=60), "the USB tablet was not found"
            assert s.wait("velora-start: menu", timeout=120), "no start menu"
            qmp.connect()
            s.keys(["1"])
            assert s.wait("velora-start: screen welcome"), "no welcome screen"
            time.sleep(1.0)
            s.keys(["f12"])
            assert s.wait("velora-start: fps counter on"), "F12 did not turn the counter on"
            phase(reader, "pointer", lambda: circle(qmp, 6, 250))
            areas = last_areas(reader)
            cont = areas.get(uiclick.CONTINUE)
            assert cont, "no Continue button in the report"

            def sweep():
                # Back and forth over every widget of the screen: hover
                # effects start and end all the time.
                pts = [(r[0] + r[2] // 2, r[1] + r[3] // 2) for r in areas.values()]
                t0 = time.monotonic()
                n = 0
                while time.monotonic() - t0 < 6:
                    a, b = pts[n % len(pts)], pts[(n + 1) % len(pts)]
                    for k in range(25):
                        qmp.move(a[0] + (b[0] - a[0]) * k // 25, a[1] + (b[1] - a[1]) * k // 25, w, h)
                        time.sleep(0.004)
                    n += 1

            phase(reader, "hover", sweep)
            qmp.click(cont[0] + cont[2] // 2, cont[1] + cont[3] // 2, w, h)
            assert s.wait("velora-start: screen location"), "Continue did not open Location"
            time.sleep(1.0)
            areas = last_areas(reader)
            back, cont = areas.get(uiclick.BACK), areas.get(uiclick.CONTINUE)
            assert back and cont, "no Back or Continue on Location"
            state = {"n": 0}

            def switch(t):
                # A click every 0.45 s: Back, Continue, Back, ...
                if t > 0.45 * state["n"]:
                    b = back if state["n"] % 2 == 0 else cont
                    state["n"] += 1
                    qmp.move(b[0] + b[2] // 2, b[1] + b[3] // 2, w, h)
                    qmp.button(True)
                    qmp.button(False)

            phase(reader, "transitions", lambda: circle(qmp, 6, 120, step=switch))
            for name, (shown, read, most) in rates.items():
                # One second of the counter can hold 61 frame starts.
                assert most <= 62, "pointer drawn %d times a second in '%s'" % (most, name)
                # Slow full frames under TCG show it about 20 times a second.
                assert shown > 5, "the pointer hardly moved in '%s' (%.0f/s)" % (name, shown)
            read = rates["pointer"][1]
            if args.xhci_polled:
                assert read <= 105, "polled xHCI read %.0f reports a second" % read
            else:
                assert read >= 150, "only %.0f tablet reports a second with interrupts" % read
            monitor.command("system_powerdown")
            wait_exit(proc)
        except AssertionError as e:
            return fail(str(e))
        finally:
            qmp.close()
        return 0

    r = run_once(args, work, {"install": disk}, args.log, check, extra=extra)
    if r == 0:
        print("qemu_run: PASS (frame rate at %s measured, xHCI %s)"
              % (args.resolution, "polled" if args.xhci_polled else "MSI-X"))
    return r


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


def screendump(monitor, path, timeout=10.0):
    """Saves the screen through the QEMU monitor (PPM); returns
    (width, height, RGB bytes)."""
    if os.path.exists(path):
        os.remove(path)
    # Quoted: the path may contain spaces.
    monitor.command("screendump \"%s\"" % os.path.abspath(path).replace("\\", "/"))
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            with open(path, "rb") as f:
                data = f.read()
            m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
            if m:
                w, h = int(m.group(1)), int(m.group(2))
                pixels = data[m.end():]
                if len(pixels) >= w * h * 3:
                    return w, h, pixels[:w * h * 3]
        except OSError:
            pass
        time.sleep(0.1)
    raise AssertionError("screendump did not produce %s" % path)


def write_png(path, w, h, rgb):
    """A PNG of the screenshot (smaller than PPM and opens anywhere)."""
    def chunk(kind, body):
        return (struct.pack(">I", len(body)) + kind + body
                + struct.pack(">I", zlib.crc32(kind + body) & 0xffffffff))
    rows = b"".join(b"\0" + rgb[y * w * 3:(y + 1) * w * 3] for y in range(h))
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(rows, 6)))
        f.write(chunk(b"IEND", b""))


def share_near(w, h, rgb, colour, tol=6, step=7):
    """Share of sampled pixels within `tol` of `colour`."""
    hits = total = 0
    for i in range(0, w * h, step):
        r, g, b = rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2]
        total += 1
        if abs(r - colour[0]) <= tol and abs(g - colour[1]) <= tol and abs(b - colour[2]) <= tol:
            hits += 1
    return hits / max(total, 1)


# velora-ui dark theme window colour (libc/ui/src/theme.rs).
UI_WINDOW = (36, 36, 39)


def check_uidemo(s, monitor, reader):
    """The graphical toolkit on the real screen: uidemo takes the screen,
    reacts to keys and the mouse, and gives the screen back on Escape."""
    logs = os.path.dirname(reader.log.name) or "."
    s.type("uidemo\n")
    assert s.wait("first frame", timeout=120), "uidemo did not draw its first frame"
    time.sleep(1.0)

    def shot(name):
        path = os.path.join(logs, name)
        w, h, rgb = screendump(monitor, path + ".ppm")
        write_png(path, w, h, rgb)
        os.remove(path + ".ppm")
        return w, h, rgb

    w, h, first = shot("uidemo-1.png")
    share = share_near(w, h, first, UI_WINDOW, tol=10)
    assert share > 0.15, "uidemo window not on the screen (%.2f)" % share
    console = share_near(w, h, first, CONSOLE_BG, tol=2)
    assert console < 0.05, "the text console still shows (%.2f)" % console
    # Keyboard: Tab to the language list, open it, pick the next language.
    s.keys(["tab", "tab", "ret", "down", "ret"])
    # Mouse (PS/2): move and click somewhere empty.
    monitor.command("mouse_move 40 30")
    monitor.command("mouse_button 1")
    monitor.command("mouse_button 0")
    time.sleep(1.5)
    _, _, second = shot("uidemo-2.png")
    assert second != first, "the screen did not change after keys and mouse"
    s.keys(["esc"])
    assert s.wait("uidemo: done", timeout=30), "uidemo did not end on Escape"
    s.run("A=ui; echo $A-back", "ui-back")
    time.sleep(1.0)
    _, _, after = shot("uidemo-3.png")
    console = share_near(w, h, after, CONSOLE_BG, tol=2)
    assert console > 0.5, "the text console did not come back (%.2f)" % console
    print("qemu_run: uidemo drew %dx%d, took keys and the mouse, gave the screen back" % (w, h))


# Console colours (kernel/src/console/text.rs). The logo's own dark
# background is made transparent by velofetch, so the console background
# shows through it.
CONSOLE_BG = (0x0b, 0x0e, 0x14)
CONSOLE_FG = (0xc0, 0xca, 0xf5)
# The mark of assets/velora-logo.png is 656x583 pixels (aspect 1.125).
# Points inside it, as fractions of its size, at least 15 source pixels
# away from any edge: on a stroke, and in the gaps.
LOGO_ASPECT = 656 / 583
LOGO_STROKES = [(0.498, 0.238), (0.498, 0.3), (0.2, 0.2), (0.8, 0.2), (0.46, 0.8), (0.42, 0.7)]
LOGO_GAPS = [(0.498, 0.64), (0.223, 0.787), (0.75, 0.8), (0.65, 0.1), (0.35, 0.05)]


def check_logo(w, h, rgb):
    """Finds the velofetch logo on a screenshot: white strokes near the
    left edge with the shape of the mark, the console background in its
    gaps (no box around it), and the summary text to the right."""
    def px(x, y):
        i = (y * w + x) * 3
        return rgb[i], rgb[i + 1], rgb[i + 2]

    def near(c, ref, tol=3):
        return all(abs(a - b) <= tol for a, b in zip(c, ref))

    xs, ys = [], []
    for y in range(h):
        for x in range(w // 3):
            if min(px(x, y)) >= 240:
                xs.append(x)
                ys.append(y)
    if len(xs) < 1000:
        return "no logo on the screen"
    x0, x1, y0, y1 = min(xs), max(xs) + 1, min(ys), max(ys) + 1
    print("qemu_run: velofetch logo at (%d, %d), %dx%d pixels" % (x0, y0, x1 - x0, y1 - y0))
    if x0 > 40 or y1 - y0 < 96 or abs((x1 - x0) / (y1 - y0) - LOGO_ASPECT) > 0.06:
        return "the logo does not have the shape of the mark: (%d,%d)-(%d,%d)" % (x0, y0, x1, y1)

    def at(fx, fy):
        return px(x0 + int(fx * (x1 - x0)), y0 + int(fy * (y1 - y0)))
    for f in LOGO_STROKES:
        if min(at(*f)) < 230:
            return "logo stroke missing at %s: %s" % (f, at(*f))
    for f in LOGO_GAPS:
        if not near(at(*f), CONSOLE_BG):
            return "not the console background inside the logo at %s: %s" % (f, at(*f))
    # Around the mark: only the console background (no box).
    for x in range(max(0, x0 - 4), min(w, x1 + 4)):
        for y in (y0 - 4, y1 + 3):
            if 0 <= y < h and not near(px(x, y), CONSOLE_BG):
                return "something around the logo at (%d, %d): %s" % (x, y, px(x, y))
    text = sum(1 for y in range(y0, y1, 2) for x in range(x1, w, 2) if near(px(x, y), CONSOLE_FG))
    if text < 200:
        return "no summary text next to the logo"
    return None


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
    if not wait_for(proc, reader, "velora-start: menu", deadline):
        return fail("no start menu")
    t = reader.find("velora-start: menu")
    print("qemu_run: start menu after %.2f s (QEMU start, firmware included)" % t)
    if max_seconds and t > max_seconds:
        return fail("start menu after %.2f s, limit %.2f s" % (t, max_seconds))
    monitor.keys(["3"], delay=0.05)
    if not wait_for(proc, reader, "velora login:", deadline):
        return fail("no login prompt after choosing the Live system")
    print("qemu_run: login prompt after %.2f s" % reader.find("velora login:"))

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
        assert s.wait("Disks:"), "velofetch did not finish"
        time.sleep(1.0)
        shot = os.path.join(os.path.dirname(reader.log.name) or ".", "velofetch.png")
        w, h, rgb = screendump(monitor, shot + ".ppm")
        write_png(shot, w, h, rgb)
        os.remove(shot + ".ppm")
        problem = check_logo(w, h, rgb)
        assert not problem, "%s (screenshot: %s)" % (problem, shot)
        # Images go to the screen only: the serial port gets the text.
        assert "\x1b_G" not in reader.text(), "image data reached the serial port"
        print("qemu_run: velofetch shows the logo as an image (screenshot %s)" % shot)
        s.run("velofetch | cat", "V E L O R A")
        s.run("velofetch --ascii", "V E L O R A")
        print("qemu_run: velofetch falls back to the ASCII logo")
        check_uidemo(s, monitor, reader)
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
    p.add_argument("mode", choices=["ktest", "boot", "install", "net", "usb", "gui", "clicks", "fps"])
    p.add_argument("--resolution", default="1280x720", help="screen size for the gui mode")
    p.add_argument("--disk-type", choices=["virtio", "sata", "nvme"], default="virtio",
                   help="how the install disk is connected (gui mode)")
    p.add_argument("--reinstall", action="store_true",
                   help="gui mode: then boot the Live system again and install over the mounted disk")
    p.add_argument("--screens", default=os.path.join("docs", "design", "installer-screens"),
                   help="where the gui mode saves its screenshots")
    p.add_argument("--screens-only", action="store_true",
                   help="gui mode: go through the screens without installing")
    p.add_argument("--xhci-polled", action="store_true",
                   help="fps mode: QEMU's xHCI without MSI and MSI-X (the polling fallback)")
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
        args.iso = {"ktest": "velora-test.iso", "clicks": "velora-uitest.iso", "fps": "velora-uitest.iso"}.get(args.mode, "velora.iso")
    if not args.log:
        args.log = os.path.join("build", "logs", args.mode + ".log")
    sys.exit(run(args))


if __name__ == "__main__":
    main()
