#!/bin/sh
# Build a double-clickable macOS "PurrCode.app" from the release binaries.
#
# Usage:
#   scripts/package-app.sh                 # uses target/release binaries
#   scripts/package-app.sh --bin <path>    # point at a specific purrcode binary
#
# The bundle is self-contained: it embeds the `purrcode` binary (which also
# acts as the daemon via `purrcode serve`) and a launcher that opens the native
# desktop IDE. Users drag PurrCode.app into /Applications and double-click —
# no terminal required.
set -eu

cd "$(dirname "$0")/.."

BIN="${PURRCODE_BIN:-target/release/purrcode}"
APP_NAME="PurrCode.app"
STAGE="${TMPDIR:-/tmp}/purrcode-app-stage"
ICONSET="${STAGE}/PurrCode.iconset"
VERSION="${PURRCODE_VERSION:-$(grep -m1 '^version' Cargo.toml | sed 's/^version = "\([^"]*\)".*/\1/')}"

if [ ! -x "$BIN" ]; then
    echo "Binary not found: $BIN" >&2
    echo "Build it first: cargo build --release -p purrcode-cli" >&2
    exit 1
fi

rm -rf "$STAGE"
mkdir -p "${STAGE}/${APP_NAME}/Contents/MacOS"
mkdir -p "${STAGE}/${APP_NAME}/Contents/Resources"

# ── Icon: 512/256/128/64/48/32/16 at @1x and @2x from brand/icons ──────────
mkdir -p "$ICONSET"
cp brand/icons/16.png   "$ICONSET/icon_16x16.png"
cp brand/icons/32.png   "$ICONSET/icon_16x16@2x.png"
cp brand/icons/32.png   "$ICONSET/icon_32x32.png"
cp brand/icons/64.png   "$ICONSET/icon_32x32@2x.png"
cp brand/icons/128.png  "$ICONSET/icon_128x128.png"
cp brand/icons/256.png  "$ICONSET/icon_128x128@2x.png"
cp brand/icons/256.png  "$ICONSET/icon_256x256.png"
cp brand/icons/512.png  "$ICONSET/icon_256x256@2x.png"
cp brand/icons/512.png  "$ICONSET/icon_512x512.png"
cp brand/icons/1024.png "$ICONSET/icon_512x512@2x.png" 2>/dev/null || \
    sips -z 1024 1024 brand/icons/512.png --out "$ICONSET/icon_512x512@2x.png" >/dev/null
iconutil -c icns "$ICONSET" -o "${STAGE}/${APP_NAME}/Contents/Resources/AppIcon.icns"

# ── The launcher: opens the native IDE, daemon auto-starts on first use ────
# macOS's default filesystem is case-insensitive, so `PurrCode` and `purrcode`
# are the SAME file. The launcher (named `PurrCode`, the CFBundleExecutable)
# must therefore not sit beside a file literally named `purrcode` — it would
# overwrite the launcher. The real binary lives in Contents/Resources instead.
cat > "${STAGE}/${APP_NAME}/Contents/MacOS/PurrCode" <<'LAUNCHER'
#!/bin/sh
# Launch the PurrCode native desktop IDE. The first run initializes config and
# starts the loopback daemon; a second double-click reuses it.
DIR="$(cd "$(dirname "$0")" && pwd)"
exec "$DIR/../Resources/purrcode" ide "$@"
LAUNCHER
chmod +x "${STAGE}/${APP_NAME}/Contents/MacOS/PurrCode"

# ── Info.plist: name, bundle id, icon, and no unnecessary entitlements ─────
cat > "${STAGE}/${APP_NAME}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>PurrCode</string>
    <key>CFBundleDisplayName</key>
    <string>PurrCode</string>
    <key>CFBundleIdentifier</key>
    <string>dev.purrcode.PurrCode</string>
    <key>CFBundleExecutable</key>
    <string>PurrCode</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
PLIST

# ── The real binary lives in Resources (not MacOS) so it cannot collide with
# ── the `PurrCode` launcher on case-insensitive filesystems ────────────────
cp "$BIN" "${STAGE}/${APP_NAME}/Contents/Resources/purrcode"
chmod +x "${STAGE}/${APP_NAME}/Contents/Resources/purrcode"

OUT="dist/${APP_NAME}"
rm -rf "$OUT"
mkdir -p dist
cp -R "${STAGE}/${APP_NAME}" "$OUT"

echo "Built $OUT"
echo "Install: drag $OUT into /Applications, then double-click PurrCode."
