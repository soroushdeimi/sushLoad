#!/bin/bash
# Builds dist/sushLoad.app (ad-hoc signed) and dist/sushLoad.dmg.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="/opt/homebrew/bin:$PATH"

cargo build --release -p sushload

DIST="$ROOT/dist"
APP="$DIST/sushLoad.app"
rm -rf "$APP" "$DIST/sushLoad.dmg" "$DIST/dmg-staging"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# Apple Silicon kills a binary overwritten in place, so it is always a fresh file.
rm -f "$APP/Contents/MacOS/sushload"
cp "$ROOT/target/release/sushload" "$APP/Contents/MacOS/sushload"
chmod 755 "$APP/Contents/MacOS/sushload"

VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
VERSION="${VERSION:-0.1.0}"

# Icon (best effort).
ICON_KEY=""
TMP="$(mktemp -d)"
if python3 "$ROOT/scripts/make_icon.py" "$TMP/icon.png" 2>/dev/null; then
  SET="$TMP/sushLoad.iconset"
  mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    sips -z $s $s "$TMP/icon.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
    sips -z $((s*2)) $((s*2)) "$TMP/icon.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
  done
  if iconutil -c icns "$SET" -o "$APP/Contents/Resources/sushLoad.icns" 2>/dev/null; then
    ICON_KEY="<key>CFBundleIconFile</key><string>sushLoad</string>"
  fi
fi
rm -rf "$TMP"

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
