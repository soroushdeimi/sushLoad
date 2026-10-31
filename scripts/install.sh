#!/bin/bash
# Builds the bundle, installs it to /Applications and links `sushload` into ~/.local/bin.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
"$ROOT/scripts/bundle.sh"

rm -rf /Applications/sushLoad.app
cp -R "$ROOT/dist/sushLoad.app" /Applications/sushLoad.app

mkdir -p "$HOME/.local/bin"
ln -sf /Applications/sushLoad.app/Contents/MacOS/sushload "$HOME/.local/bin/sushload"
echo "Installed /Applications/sushLoad.app; run 'sushload --web' for the browser UI (make sure ~/.local/bin is on PATH)."
