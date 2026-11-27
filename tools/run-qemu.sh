#!/usr/bin/env bash
# Runs velora.iso in a QEMU window for trying it by hand.
#
#   tools/run-qemu.sh                    1920x1080, ISO and a 2 GiB disk
#   tools/run-qemu.sh --resolution 1280x720
#   tools/run-qemu.sh --full-screen      the same in full screen
#   tools/run-qemu.sh --disk-only        boot the installed system from the disk
#
# The disk is build/run-disk.img (created when missing, never another
# file). The pointer is a USB tablet, so the mouse works without being
# grabbed. On Windows the window is made DPI-aware for this QEMU process
# only (SDL_WINDOWS_DPI_AWARENESS): otherwise, with display scaling such
# as 125%, Windows stretches the window and every pixel is blurred.
# Uses WHPX on Windows, KVM on Linux when available, else TCG.

set -euo pipefail
cd "$(dirname "$0")/.."

RES=""
FULL=0
ISO=1
for arg in "$@"; do
    case "$arg" in
        --resolution) RES="next" ;;
        --full-screen) FULL=1 ;;
        --disk-only) ISO=0 ;;
        -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
        *)
            if [ "$RES" = "next" ]; then RES="$arg"; else
                echo "run-qemu.sh: unknown argument '$arg'" >&2; exit 2
            fi ;;
    esac
done

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
[ -f build/run-vars.fd ] || cp "$VARS" build/run-vars.fd
if [ ! -f build/run-disk.img ]; then
    truncate -s 2G build/run-disk.img
fi
if [ "$ISO" = 1 ] && [ ! -f velora.iso ]; then
    echo "run-qemu.sh: velora.iso not found (run ./build.sh first)" >&2
    exit 1
fi

if [ -z "$RES" ] || [ "$RES" = "next" ]; then
    RES="1920x1080"
fi
W="${RES%x*}"
H="${RES#*x}"

if [ "$WINDOWS" = 1 ]; then
    ACCEL=(-accel whpx,kernel-irqchip=off -accel tcg -cpu qemu64)
    export SDL_WINDOWS_DPI_AWARENESS=permonitorv2
elif [ -w /dev/kvm ]; then
    ACCEL=(-accel kvm -cpu host)
else
    ACCEL=(-accel tcg -cpu max)
fi

ARGS=(-machine q35 -m 1G -smp 4 "${ACCEL[@]}"
    -drive "if=pflash,format=raw,readonly=on,file=$CODE"
    -drive "if=pflash,format=raw,file=build/run-vars.fd"
    -drive "file=build/run-disk.img,format=raw,if=virtio"
    -netdev user,id=n0 -device e1000,netdev=n0
    -device qemu-xhci -device usb-kbd -device usb-tablet
    -vga none -device "VGA,xres=$W,yres=$H,vgamem_mb=64"
    -display sdl -serial file:build/run-serial.log)
[ "$ISO" = 1 ] && ARGS+=(-cdrom velora.iso)
[ "$FULL" = 1 ] && ARGS+=(-full-screen)

echo "run-qemu.sh: ${W}x${H}, serial output in build/run-serial.log"
exec "$QEMU" "${ARGS[@]}"
