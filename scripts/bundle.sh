#!/bin/bash
# Builds dist/sushLoad.app (ad-hoc signed) and dist/sushLoad.dmg.
# SUSHLOAD_BIN=<path> bundles an already built binary (CI: the universal one) instead of building.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="/opt/homebrew/bin:$PATH"

BIN="${SUSHLOAD_BIN:-}"
if [ -z "$BIN" ]; then
  cargo build --release -p sushload
  BIN="$ROOT/target/release/sushload"
fi

DIST="$ROOT/dist"
APP="$DIST/sushLoad.app"
rm -rf "$APP" "$DIST/sushLoad.dmg" "$DIST/dmg-staging"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# Apple Silicon kills a binary overwritten in place, so it is always a fresh file.
rm -f "$APP/Contents/MacOS/sushload"
cp "$BIN" "$APP/Contents/MacOS/sushload"
chmod 755 "$APP/Contents/MacOS/sushload"

# Browser extension: users load this folder unpacked (Settings > Browser > Show extension folder).
rm -rf "$APP/Contents/Resources/extension"
cp -R "$ROOT/extension" "$APP/Contents/Resources/extension"
find "$APP/Contents/Resources/extension" -name '.DS_Store' -delete

VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
VERSION="${VERSION:-0.1.0}"

# Icon: assets/AppIcon.icns is generated from assets/logo.png by scripts/make_icon.py.
cp "$ROOT/assets/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
ICON_KEY="<key>CFBundleIconFile</key><string>AppIcon</string>"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>com.soroushdeimi.sushload</string>
  <key>CFBundleName</key><string>sushLoad</string>
  <key>CFBundleDisplayName</key><string>sushLoad</string>
  <key>CFBundleExecutable</key><string>sushload</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  $ICON_KEY
</dict>
</plist>
PLIST

codesign --force --deep -s - "$APP"

STAGE="$DIST/dmg-staging"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/sushLoad.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname sushLoad -srcfolder "$STAGE" -ov -format UDZO "$DIST/sushLoad.dmg" >/dev/null
rm -rf "$STAGE"

echo "Built $APP and $DIST/sushLoad.dmg"
