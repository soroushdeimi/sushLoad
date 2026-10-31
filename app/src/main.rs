//! sushLoad: a light download manager. `sushload` opens a native window; `sushload --web` serves the
//! same UI to a browser instead.

mod server;
mod window;

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use sushload_core::Engine;

const DEFAULT_PORT: u16 = 47815;
const BUNDLE_ID: &str = "com.soroushdeimi.sushload";

struct Args {
    web: bool,
    no_open: bool,
    port: u16,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        web: false,
        no_open: false,
        port: DEFAULT_PORT,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--web" => a.web = true,
            "--no-open" => a.no_open = true,
            "--port" => {
                let v = it.next().ok_or("--port needs a number")?;
                a.port = v.parse().map_err(|_| format!("bad port: {v}"))?;
            }
            "-h" | "--help" => {
                println!("usage: sushload [--web [--no-open]] [--port N]");
                std::process::exit(0);
            }
            // macOS passes -psn_* when launching a bundle from older Finders; ignore unknown -psn.
            s if s.starts_with("-psn_") => {}
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(a)
}

fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library")
        .join("Application Support")
        .join("sushLoad")
}

/// Returns `(port, token)` of a live instance recorded in `instance.json`, if it still answers.
fn running_instance(dir: &std::path::Path) -> Option<(u16, String)> {
    let v: Value = serde_json::from_slice(&std::fs::read(dir.join("instance.json")).ok()?).ok()?;
    let port = u16::try_from(v.get("port")?.as_u64()?).ok()?;
    let token = v.get("token")?.as_str()?.to_string();
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
    let req = format!(
        "GET /api/state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Sushload-Token: {token}\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).ok()?;
    let mut head = [0u8; 16];
    s.read_exact(&mut head).ok()?;
    head.starts_with(b"HTTP/1.1 200").then_some((port, token))
}

fn write_instance(dir: &std::path::Path, port: u16, token: &str) {
    let v = json!({ "pid": std::process::id(), "port": port, "token": token });
    let tmp = dir.join("instance.json.tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        let _ = std::fs::rename(&tmp, dir.join("instance.json"));
    }
}

fn open_in_browser(url: &str) {
    let _ = Command::new("open").arg(url).spawn();
}

/// Blocks SIGINT/SIGTERM in this thread (and every thread spawned after) and waits for one.
/// Zero CPU while waiting.
fn block_signals() -> libc::sigset_t {
    // SAFETY: plain libc calls on a locally owned, initialised sigset.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    }
}

fn wait_for_signal(set: &libc::sigset_t) {
    let mut sig: libc::c_int = 0;
    // SAFETY: `set` is a valid sigset; sigwait only writes `sig`.
    unsafe {
        libc::sigwait(set, &mut sig);
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("sushload: {e}");
            std::process::exit(2);
        }
    };
    let dir = data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("sushload: cannot create {}: {e}", dir.display());
        std::process::exit(1);
    }

    if let Some((port, token)) = running_instance(&dir) {
        let url = format!("http://127.0.0.1:{port}/?t={token}");
        if args.web {
            println!("sushLoad is already running: {url}");
            if !args.no_open {
                open_in_browser(&url);
            }
        } else {
            println!("sushLoad is already running; bringing it to the front.");
            let _ = Command::new("open").args(["-b", BUNDLE_ID]).status();
        }
        return;
    }

    let sigset = args.web.then(block_signals);

    let engine = match Engine::start(dir.clone()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("sushload: cannot start the engine: {e}");
            std::process::exit(1);
        }
    };
    let srv = match server::start(engine.clone(), args.port) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sushload: {e}");
            std::process::exit(1);
        }
    };
    write_instance(&dir, srv.port, &srv.token);
    let url = format!("http://127.0.0.1:{}/?t={}", srv.port, srv.token);

    if let Some(set) = sigset {
        println!("sushLoad web UI: {url}");
        if !args.no_open {
            open_in_browser(&url);
        }
        wait_for_signal(&set);
    } else {
        window::run(engine.clone(), format!("{url}&app=1"));
    }

    engine.shutdown();
    let _ = std::fs::remove_file(dir.join("instance.json"));
}
