#!/bin/bash
# Registers the sushLoad native-messaging host for every installed Chromium browser, so the
# browser extension can hand downloads to the app. Usage: install-native-host.sh [path-to-binary]
# (default: the binary inside /Applications/sushLoad.app). The app also does this at startup.
set -euo pipefail
BIN="${1:-/Applications/sushLoad.app/Contents/MacOS/sushload}"
case "$BIN" in /*) ;; *) BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")" ;; esac
NAME="com.soroushdeimi.sushload"
EXT_ID="ibiipbpgmolcjidbajmjlkjcnngphfok"
SUPPORT="$HOME/Library/Application Support"

n=0
for b in "Google/Chrome" "Google/Chrome Beta" "Chromium" "BraveSoftware/Brave-Browser" "Microsoft Edge"; do
  [ -d "$SUPPORT/$b" ] || continue
  dir="$SUPPORT/$b/NativeMessagingHosts"
  mkdir -p "$dir"
  cat > "$dir/$NAME.json" <<JSON
{
  "name": "$NAME",
  "description": "sushLoad download manager",
  "path": "$BIN",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://$EXT_ID/"]
}
JSON
  echo "Registered native host for $b"
  n=$((n + 1))
done
[ "$n" -gt 0 ] || echo "No Chromium-based browser found; nothing registered."
