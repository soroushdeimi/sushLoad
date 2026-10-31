<p align="center"><img src="assets/AppIcon.png" width="128" alt="sushLoad icon"></p>

# sushLoad

A lightweight IDM-style download manager for macOS, written in Rust.

- Segmented downloads over HTTP(S) `Range` requests (1–16 connections), with dynamic splitting:
  a connection that finishes takes half of the largest remaining segment.
- Pause / resume that survives quitting, crashes and restarts (state in
  `~/Library/Application Support/sushLoad/state.json`, data in `<file>.part`).
- Queue (`max_concurrent`), global speed limit, retries with backoff, falls back to one
  connection when the server has no Range support.
- Category folders: `~/Downloads/sushLoad/{Video,Music,Documents,Archives,Programs,Images,Other}`.
- Proxy: system proxy, none, or custom `http://`, `https://`, `socks5://`, `socks5h://` (with user/pass).
- English and Persian (RTL) UI.

## Two ways to run

| | |
|---|---|
| **App** | `sushLoad.app`: native window (system WebKit view, not Electron), Dock icon, menus. Closing the window keeps downloads running. |
| **Browser** | `sushload --web` serves the same UI on `127.0.0.1` (token-protected) and opens it in your browser. `--no-open`, `--port N`. |

## Browser extension (Chrome, Brave, Edge)

Like IDM's integration module: it offers to take over downloads of chosen file types (hold
<kbd>Alt</kbd> while clicking to skip), adds "Download with sushLoad" / "Download all links" to the
context menu, and shows a "Download with sushLoad" pill over videos. Downloads are handed to the
app over native messaging together with the page's cookies, Referer and User-Agent, so links
that need a login work; cookies are only ever sent to the link's own site.

1. Install the app (`scripts/install.sh`); it registers the native messaging host.
2. Open `chrome://extensions`, turn on *Developer mode*, *Load unpacked*, and pick the
   `extension` folder (sushLoad › Settings › Browser › *Show extension folder*).

## Build / install

```bash
scripts/bundle.sh    # dist/sushLoad.app + dist/sushLoad.dmg (ad-hoc signed)
scripts/install.sh   # copies to /Applications and links ~/.local/bin/sushload
cargo test --workspace
```

## Icon

`assets/logo.png` is the artwork; `scripts/make_icon.py` (needs Pillow) turns it into
`assets/AppIcon.png` / `AppIcon.icns` in the macOS rounded-square grid.

## Layout

- `core/` — engine (`engine/`), category rules, shared types (`model.rs`). No UI code.
- `app/` — local HTTP API + embedded web UI (`web/`), native window (`window.rs`).
