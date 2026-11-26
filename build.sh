#!/usr/bin/env bash
# Builds the bootable Velora OS image.
#
#   ./build.sh           -> velora.iso       (normal kernel)
#   ./build.sh --test    -> velora-test.iso  (kernel with the in-kernel test runner)
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
        -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
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

PROGRAMS="hello vtest login vsh ls cat cp mv rm mkdir touch echo clear ps kill mem uptime date whoami reboot shutdown help velofetch"
FILES=()
for p in $PROGRAMS; do
    FILES+=(--file "bin/$p=$UBIN/$p:755")
done
cargo run -q -p mkinitrd --release -- \
    --out "$INITRD" \
    "${FILES[@]}" \
    --file "etc/motd=userland/rootfs/etc/motd:644" \
    --file "etc/passwd=userland/rootfs/etc/passwd:644" \
    --file "etc/group=userland/rootfs/etc/group:644" \
    --file "etc/shadow=$SHADOW:600" \
    --dir "dev" --dir "mnt" --dir "tmp:1777" --dir "home" --dir "root:700"

# 4. Bootloader config.
CONF="$BUILD/limine-$MODE.conf"
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
    --file "licenses/spleen.txt=docs/licenses/SPLEEN.txt"

echo "==> done: $OUT"
