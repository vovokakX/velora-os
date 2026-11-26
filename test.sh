#!/usr/bin/env bash
# Runs every test: host unit tests, then the kernel in headless QEMU.
#
#   ./test.sh            all tests
#   ./test.sh --host     host unit tests only (no QEMU needed)

set -euo pipefail
cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi
PY=python3
command -v "$PY" >/dev/null 2>&1 && "$PY" -c "" 2>/dev/null || PY=python

echo "==> host unit tests"
cargo test --workspace --exclude velora-kernel

if [ "${1:-}" = "--host" ]; then
    exit 0
fi

echo "==> kernel tests in QEMU"
./build.sh --test
"$PY" tests/qemu_run.py ktest

echo "==> boot test in QEMU"
./build.sh
"$PY" tests/qemu_run.py boot --max-boot-seconds "${VELORA_MAX_BOOT_SECONDS:-15}"

echo "==> network test in QEMU (DHCP, ping, TCP with the host)"
"$PY" tests/qemu_run.py net --timeout 300

echo "==> installation test in QEMU (install, boot from disk, files persist)"
"$PY" tests/qemu_run.py install --timeout 900

echo "==> all tests passed"
