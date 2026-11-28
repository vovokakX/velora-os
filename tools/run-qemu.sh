#!/usr/bin/env bash
# Runs velora.iso in a QEMU window for trying it by hand.
#
#   tools/run-qemu.sh                      1920x1080, ISO and a 4 GiB virtio disk
#   tools/run-qemu.sh --resolution 1280x720
#   tools/run-qemu.sh --disk sata          the disk on AHCI (also: virtio, nvme)
#   tools/run-qemu.sh --fresh-disk         a new empty disk image first
#   tools/run-qemu.sh --full-screen        the same in full screen
#   tools/run-qemu.sh --display sdl        an SDL window instead of GTK
#   tools/run-qemu.sh --disk-only          boot the installed system from the disk
#
# The disk is build/run-disk.img (created with qemu-img when missing or
# with --fresh-disk; never another file). The pointer is a USB tablet, so
# the mouse works without being grabbed.
#
# The GTK window is redrawn at the monitor's refresh rate; QEMU redraws an
# SDL window only every 30 ms (about 33 frames per second). Both are made
# DPI-aware for this QEMU process only: otherwise, with display scaling
# such as 125%, Windows stretches the window and every pixel is blurred.
#
# Acceleration: WHPX on Windows, KVM on Linux. The script checks first
# that it starts; if not, it warns and uses TCG (much slower).
#
# A machine reset hangs QEMU under WHPX (QEMU's own system_reset too, with
# one CPU as well; TCG resets fine). So QEMU runs with -no-reboot, and when
# the guest restarts the script starts QEMU again. After an installation
# it starts without the ISO, as if the install drive had been removed.

set -euo pipefail
cd "$(dirname "$0")/.."

RES=""
FULL=0
ISO=1
DISK=virtio
DISPLAY_KIND=gtk
FRESH=0
want=""
for arg in "$@"; do
    if [ -n "$want" ]; then
        case "$want" in
            res) RES="$arg" ;;
            disk) DISK="$arg" ;;
            display) DISPLAY_KIND="$arg" ;;
        esac
        want=""
        continue
    fi
    case "$arg" in
        --resolution) want=res ;;
        --disk) want=disk ;;
        --display) want=display ;;
        --full-screen) FULL=1 ;;
        --disk-only) ISO=0 ;;
        --fresh-disk) FRESH=1 ;;
        -h|--help) sed -n '2,27p' "$0"; exit 0 ;;
        *) echo "run-qemu.sh: unknown argument '$arg'" >&2; exit 2 ;;
    esac
done
case "$DISK" in
    virtio|sata|nvme) ;;
    *) echo "run-qemu.sh: --disk must be virtio, sata or nvme" >&2; exit 2 ;;
esac
case "$DISPLAY_KIND" in
    gtk|sdl) ;;
    *) echo "run-qemu.sh: --display must be gtk or sdl" >&2; exit 2 ;;
esac

case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) WINDOWS=1 ;;
    *) WINDOWS=0 ;;
esac

QEMU="${QEMU:-}"
if [ -z "$QEMU" ]; then
    if [ "$WINDOWS" = 1 ] && [ -x "/c/Program Files/qemu/qemu-system-x86_64.exe" ]; then
        QEMU="/c/Program Files/qemu/qemu-system-x86_64.exe"
    else
        QEMU="qemu-system-x86_64"
    fi
fi
QDIR="$(dirname "$(command -v "$QEMU" || echo "$QEMU")")"
QEMU_IMG="$QDIR/qemu-img"
[ -x "$QEMU_IMG" ] || [ -x "$QEMU_IMG.exe" ] || QEMU_IMG="$(command -v qemu-img || true)"
CODE="${OVMF_CODE:-}"
VARS="${OVMF_VARS:-}"
for d in "$QDIR/share" /usr/share/edk2/x64 /usr/share/OVMF /usr/share/qemu; do
    [ -z "$CODE" ] && [ -f "$d/edk2-x86_64-code.fd" ] && CODE="$d/edk2-x86_64-code.fd"
    [ -z "$CODE" ] && [ -f "$d/OVMF_CODE_4M.fd" ] && CODE="$d/OVMF_CODE_4M.fd"
    [ -z "$VARS" ] && [ -f "$d/edk2-i386-vars.fd" ] && VARS="$d/edk2-i386-vars.fd"
    [ -z "$VARS" ] && [ -f "$d/OVMF_VARS_4M.fd" ] && VARS="$d/OVMF_VARS_4M.fd"
done
[ -n "$CODE" ] && [ -n "$VARS" ] || { echo "run-qemu.sh: OVMF firmware not found (set OVMF_CODE and OVMF_VARS)" >&2; exit 1; }

mkdir -p build
if [ "$FRESH" = 1 ] || [ ! -f build/run-disk.img ]; then
    rm -f build/run-disk.img
    if [ -n "$QEMU_IMG" ]; then
        "$QEMU_IMG" create -q -f raw build/run-disk.img 4G
    else
        truncate -s 4G build/run-disk.img
    fi
    echo "run-qemu.sh: new empty disk build/run-disk.img (4 GiB)"
fi
if [ "$ISO" = 1 ] && [ ! -f velora.iso ]; then
    echo "run-qemu.sh: velora.iso not found (run ./build.sh first)" >&2
    exit 1
fi

[ -n "$RES" ] || RES="1920x1080"
W="${RES%x*}"
H="${RES#*x}"

# Does the hardware accelerator start? QEMU that is still running after
# three seconds has initialised it; one that failed has already exited.
accel_works() {
    local code=0
    timeout 3 "$QEMU" -accel "$1" -machine q35 -cpu "$2" -nodefaults -display none -S \
        >/dev/null 2>&1 || code=$?
    [ "$code" = 124 ]
}
if [ "$WINDOWS" = 1 ]; then
    HW=(whpx,kernel-irqchip=off qemu64 WHPX)
    export SDL_WINDOWS_DPI_AWARENESS=permonitorv2
else
    HW=(kvm host KVM)
fi
if accel_works "${HW[0]}" "${HW[1]}"; then
    ACCEL=(-accel "${HW[0]}" -cpu "${HW[1]}")
    ACCEL_NAME="${HW[2]}"
else
    echo "run-qemu.sh: WARNING: ${HW[2]} is not available; falling back to TCG (software emulation, much slower)" >&2
    if [ "$WINDOWS" = 1 ]; then
        echo "run-qemu.sh: turn on the Windows feature 'Windows Hypervisor Platform' to use WHPX" >&2
    fi
    ACCEL=(-accel tcg -cpu max)
    ACCEL_NAME=TCG
fi

case "$DISK" in
    virtio) DISK_DEV=(-device virtio-blk-pci,drive=disk0) ;;
    sata) DISK_DEV=(-device ide-hd,drive=disk0,bus=ide.1) ;;
    nvme) DISK_DEV=(-device nvme,serial=velora0,drive=disk0) ;;
esac

if [ "$DISPLAY_KIND" = gtk ]; then
    DISP="gtk,zoom-to-fit=off"
else
    DISP="sdl"
fi

ARGS=(-machine q35 -m 1G -smp 4 "${ACCEL[@]}"
    -drive "if=pflash,format=raw,readonly=on,file=$CODE"
    -drive "if=pflash,format=raw,file=build/run-vars.fd"
    -drive "if=none,id=disk0,format=raw,file=build/run-disk.img" "${DISK_DEV[@]}"
    -netdev user,id=n0 -device e1000,netdev=n0
    -device qemu-xhci -device usb-kbd -device usb-tablet
    -vga none -device "VGA,xres=$W,yres=$H,vgamem_mb=64"
    -display "$DISP" -serial file:build/run-serial.log -no-reboot)
[ "$FULL" = 1 ] && ARGS+=(-full-screen)

echo "run-qemu.sh: ${W}x${H}, $ACCEL_NAME, $DISK disk, $DISPLAY_KIND window; serial output in build/run-serial.log"
while :; do
    # Fresh firmware variables every start: boot entries saved by an
    # earlier run (the ISO first, say) made the firmware stop in its
    # shell instead of booting the installed disk.
    cp "$VARS" build/run-vars.fd
    MEDIA=()
    [ "$ISO" = 1 ] && MEDIA=(-cdrom velora.iso)
    "$QEMU" "${ARGS[@]}" "${MEDIA[@]}"
    # The window was closed, or the system was shut down.
    tail -n 3 build/run-serial.log | grep -q "Velora OS: restarting" || break
    if grep -q "velora-start: installed" build/run-serial.log; then
        ISO=0
    fi
    if [ "$ISO" = 1 ]; then
        echo "run-qemu.sh: the system restarted; starting QEMU again"
    else
        echo "run-qemu.sh: the system restarted; starting QEMU again from the disk"
    fi
done
