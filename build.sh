#!/usr/bin/env bash
# Builds the bootable Velora OS image.
#
#   ./build.sh           -> velora.iso       (normal kernel)
#   ./build.sh --test    -> velora-test.iso  (kernel with the in-kernel test runner)
#   ./build.sh --uitest  -> velora-uitest.iso (normal kernel; the installer
#                           reports its widgets for tests/qemu_run.py clicks)
#
# Works on Linux and on Windows (Git Bash). Needs: cargo/rustup, curl,
# unzip, sha256sum. The Rust toolchain is pinned in rust-toolchain.toml.

set -euo pipefail
cd "$(dirname "$0")"

LIMINE_VERSION="12.11.1"
LIMINE_SHA256="27773b8b974f6ab55d7be5bd15a6914fe70a91f0c56858ca0cbe76c590b60a22"
LIMINE_URL="https://github.com/limine-bootloader/limine/releases/download/v${LIMINE_VERSION}/limine-binary.zip"

MODE="release"
OUT="velora.iso"
FEATURES=""
CMDLINE=""
for arg in "$@"; do
    case "$arg" in
        --test) MODE="test"; OUT="velora-test.iso"; FEATURES="--features ktest"; CMDLINE="velora.test" ;;
        --uitest) OUT="velora-uitest.iso"; CMDLINE="velora.uitest" ;;
        -h|--help) sed -n '2,11p' "$0"; exit 0 ;;
        *) echo "build.sh: unknown argument '$arg'" >&2; exit 2 ;;
    esac
done

if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi
for tool in cargo curl unzip sha256sum; do
    command -v "$tool" >/dev/null 2>&1 || { echo "build.sh: '$tool' not found" >&2; exit 1; }
done

BUILD=build
LIMINE_DIR="$BUILD/limine-$LIMINE_VERSION"
mkdir -p "$BUILD"

# 1. Bootloader (the only third-party binary), verified by checksum.
if [ ! -f "$LIMINE_DIR/BOOTX64.EFI" ]; then
    echo "==> fetching Limine $LIMINE_VERSION"
    zip="$BUILD/limine-binary-$LIMINE_VERSION.zip"
    curl -fsSL -o "$zip" "$LIMINE_URL"
    echo "$LIMINE_SHA256  $zip" | sha256sum -c --quiet -
    rm -rf "$LIMINE_DIR" "$BUILD/limine-binary"
    unzip -q -o "$zip" -d "$BUILD"
    mv "$BUILD/limine-binary" "$LIMINE_DIR"
fi

# Inter (the font of the graphical programs), verified by checksum.
./tools/fetch-fonts.sh
# Only the glyphs Velora shows: fontdue keeps every outline in memory.
echo "==> subsetting fonts"
mkdir -p build/fonts/subset
for f in Inter-Regular Inter-Medium Inter-SemiBold; do
    cargo run -q -p fontsubset --release -- "build/fonts/$f.ttf" "build/fonts/subset/$f.ttf"
done
cargo run -q -p fontsubset --release -- build/fonts/JetBrainsMono-Regular.ttf \
    build/fonts/subset/JetBrainsMono-Regular.ttf --terminal

# 2. Kernel.
echo "==> building kernel ($MODE)"
# shellcheck disable=SC2086
cargo build -p velora-kernel --target x86_64-unknown-none --release $FEATURES
KERNEL="target/x86_64-unknown-none/release/velora"
KCOPY="$BUILD/velora-$MODE.elf"
cp "$KERNEL" "$KCOPY"

# 3. User programs and the initramfs. They are built with their own flags
# (lower-half code, static relocation model) and target directory.
echo "==> building user programs"
USER_TARGET="target/user"
(cd userland && CARGO_TARGET_DIR="../$USER_TARGET" \
    RUSTFLAGS="-C relocation-model=static -C force-frame-pointers=yes" \
    cargo build --release --target x86_64-unknown-none)
UBIN="$USER_TARGET/x86_64-unknown-none/release"
INITRD="$BUILD/initramfs.tar"

# Password hashes for the Live system's accounts. Only hashes go into the
# image; the passwords come from the environment (defaults are documented
# in README.md and are meant to be changed by the installer).
: "${VELORA_ROOT_PASSWORD:=velora}"
: "${VELORA_USER_PASSWORD:=velora}"
export VELORA_ROOT_PASSWORD VELORA_USER_PASSWORD
SHADOW="$BUILD/shadow"
cargo run -q -p mkpasswd --release -- --out "$SHADOW" \
    root=VELORA_ROOT_PASSWORD user=VELORA_USER_PASSWORD

# What velora-setup installs: the kernel without debug data and Limine.
KSTRIP="$BUILD/velora-$MODE-stripped.elf"
cargo run -q -p kstrip --release -- "$KCOPY" "$KSTRIP"

PROGRAMS="hello vtest login vsh ls cat cp mv rm mkdir touch echo clear ps kill mem uptime date whoami reboot shutdown help velofetch df passwd ip ping nc host fetch plume dmesg velora-setup velora-start uidemo lumi"
FILES=()
for p in $PROGRAMS; do
    FILES+=(--file "bin/$p=$UBIN/$p:755")
done
# Pictures in assets/wallpapers appear in Lumi's wallpaper choice (the
# built-in wallpapers are drawn by Lumi itself).
for f in assets/wallpapers/*.png; do
    [ -f "$f" ] || continue
    FILES+=(--file "usr/share/wallpapers/$(basename "$f")=$f:644")
done
cargo run -q -p mkinitrd --release -- \
    --out "$INITRD" \
    "${FILES[@]}" \
    --file "etc/motd=userland/rootfs/etc/motd:644" \
    --file "etc/passwd=userland/rootfs/etc/passwd:644" \
    --file "etc/group=userland/rootfs/etc/group:644" \
    --file "etc/shadow=$SHADOW:600" \
    --file "usr/lib/velora/velora=$KSTRIP:644" \
    --file "usr/lib/velora/BOOTX64.EFI=$LIMINE_DIR/BOOTX64.EFI:644" \
    --file "usr/share/fonts/Inter-Regular.ttf=build/fonts/subset/Inter-Regular.ttf:644" \
    --file "usr/share/fonts/Inter-Medium.ttf=build/fonts/subset/Inter-Medium.ttf:644" \
    --file "usr/share/fonts/Inter-SemiBold.ttf=build/fonts/subset/Inter-SemiBold.ttf:644" \
    --file "usr/share/fonts/JetBrainsMono-Regular.ttf=build/fonts/subset/JetBrainsMono-Regular.ttf:644" \
    --dir "usr/share/wallpapers" --dir "dev" --dir "mnt" --dir "tmp:1777" --dir "home" --dir "root:700"

# 4. Bootloader config.
CONF="$BUILD/limine-${OUT%.iso}.conf"
sed "s|@CMDLINE@|$CMDLINE|" boot/limine.conf > "$CONF"

# 5. ISO image.
echo "==> building $OUT"
cargo run -q -p mkiso --release -- \
    --out "$OUT" \
    --label VELORA \
    --efi "$LIMINE_DIR/BOOTX64.EFI" \
    --file "boot/limine.conf=$CONF" \
    --file "boot/velora=$KCOPY" \
    --file "boot/initramfs.tar=$INITRD" \
    --file "licenses/velora.txt=LICENSE" \
    --file "licenses/limine.txt=docs/licenses/LIMINE.txt" \
    --file "licenses/spleen.txt=docs/licenses/SPLEEN.txt" \
    --file "licenses/inter.txt=docs/licenses/INTER.txt" \
    --file "licenses/jetbrains.txt=docs/licenses/JETBRAINS-MONO.txt" \
    --file "licenses/crates.txt=docs/licenses/RUST-CRATES.txt"

# 6. Image for USB drives (release build only): GPT + FAT32 EFI partition.
if [ "$OUT" = "velora.iso" ]; then
    echo "==> building velora-usb.img"
    cargo run -q -p mkusb --release -- \
        --out velora-usb.img \
        --size-mib 64 \
        --file "EFI/BOOT/BOOTX64.EFI=$LIMINE_DIR/BOOTX64.EFI" \
        --file "boot/limine.conf=$CONF" \
        --file "boot/velora=$KCOPY" \
        --file "boot/initramfs.tar=$INITRD" \
        --file "licenses/velora.txt=LICENSE" \
        --file "licenses/limine.txt=docs/licenses/LIMINE.txt" \
        --file "licenses/spleen.txt=docs/licenses/SPLEEN.txt" \
        --file "licenses/inter.txt=docs/licenses/INTER.txt" \
        --file "licenses/jetbrains.txt=docs/licenses/JETBRAINS-MONO.txt" \
        --file "licenses/crates.txt=docs/licenses/RUST-CRATES.txt"
fi

echo "==> done: $OUT"
