#!/usr/bin/env bash
# Downloads the Inter font (SIL OFL 1.1, see docs/third-party.md) into
# build/fonts, checked by SHA-256. The font files are not kept in the
# repository. Used by build.sh and test.sh.

set -euo pipefail
cd "$(dirname "$0")/.."

INTER_VERSION="4.1"
INTER_SHA256="9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e"
INTER_URL="https://github.com/rsms/inter/releases/download/v${INTER_VERSION}/Inter-${INTER_VERSION}.zip"
FONTS="build/fonts"
FACES="Inter-Regular Inter-Medium Inter-SemiBold"

missing=0
for f in $FACES; do
    [ -f "$FONTS/$f.ttf" ] || missing=1
done
[ -f "$FONTS/LICENSE.txt" ] || missing=1
if [ "$missing" = 0 ]; then
    exit 0
fi

echo "==> fetching Inter $INTER_VERSION"
mkdir -p build "$FONTS"
zip="build/Inter-$INTER_VERSION.zip"
if ! echo "$INTER_SHA256  $zip" | sha256sum -c --quiet - 2>/dev/null; then
    curl -fsSL -o "$zip" "$INTER_URL"
    echo "$INTER_SHA256  $zip" | sha256sum -c --quiet -
fi
for f in $FACES; do
    unzip -q -o -j "$zip" "extras/ttf/$f.ttf" -d "$FONTS"
done
unzip -q -o -j "$zip" "LICENSE.txt" -d "$FONTS"
