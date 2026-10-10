use anyhow::{Context, Result};
use nzxt_ctl_common::config::{self, Mode};
use nzxt_ctl_common::ipc::{LiveState, Request, Response, SOCKET_PATH};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

pub type SharedState = Arc<Mutex<LiveState>>;

/// Dedicated group that both the socket and /etc/nzxt-ctl/config.toml are
/// shared through, so a non-root GUI process can talk to the root-owned
/// daemon without the socket being world-writable. See INSTALL.md for how
/// this group is created and the user added to it.
const SOCKET_GROUP: &str = "nzxt-ctl";

/// Starts the IPC server on a background thread. Returns the SharedState
/// handle for the control loop to update, and a callback-trigger channel
/// is deliberately NOT included yet - config reload just re-reads from
/// disk on request rather than pushing new curves live; see main.rs.
pub fn start_server(
    reload_flag: Arc<std::sync::atomic::AtomicBool>,
    config_path: PathBuf,
) -> Result<SharedState> {
    let state: SharedState = Arc::new(Mutex::new(LiveState::default()));

    // Clean up a stale socket file from a previous run that didn't shut
    // down cleanly - otherwise bind() fails with "address in use".
    let socket_dir = std::path::Path::new(SOCKET_PATH).parent().unwrap();
    std::fs::create_dir_all(socket_dir)?;
    let _ = std::fs::remove_file(SOCKET_PATH);

    let listener = UnixListener::bind(SOCKET_PATH)?;
    // Prefer chown'ing to the dedicated `nzxt-ctl` group + 0660, so only
    // root and group members (the GUI's user, once added - see
    // INSTALL.md) can connect. If that group doesn't exist yet (e.g.
    // running ad hoc during development, before install), fall back to
    // the previous 0666 world-writable behavior rather than failing to
    // start - flagged loudly via the warning below.
    {
        use std::os::unix::fs::PermissionsExt;
        match nix::unistd::Group::from_name(SOCKET_GROUP) {
            Ok(Some(group)) => {
                nix::unistd::chown(SOCKET_PATH, None, Some(group.gid))
                    .context("chown socket to nzxt-ctl group")?;
                std::fs::set_permissions(SOCKET_PATH, std::fs::Permissions::from_mode(0o660))?;
                log::info!("IPC socket restricted to root:{} (0660)", SOCKET_GROUP);
            }
            Ok(None) => {
                log::warn!(
                    "group '{}' not found - leaving IPC socket world-writable (0666). \
                     See INSTALL.md to set up the dedicated group.",
                    SOCKET_GROUP
                );
                std::fs::set_permissions(SOCKET_PATH, std::fs::Permissions::from_mode(0o666))?;
            }
            Err(e) => {
                log::warn!(
                    "failed to look up group '{}' ({}) - leaving IPC socket world-writable (0666)",
                    SOCKET_GROUP, e
                );
                std::fs::set_permissions(SOCKET_PATH, std::fs::Permissions::from_mode(0o666))?;
            }
        }
    }
    log::info!("IPC socket listening at {}", SOCKET_PATH);

    let state_clone = state.clone();
    let ctx = Arc::new(ConfigCtx { path: config_path, write_lock: Mutex::new(()) });
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let state = state_clone.clone();
                    let reload_flag = reload_flag.clone();
                    let ctx = ctx.clone();
                    thread::spawn(move || {
                        if let Err(e) = handle_client(stream, state, reload_flag, &ctx) {
                            log::debug!("IPC client error: {}", e);
                        }
                    });
                }
                Err(e) => log::warn!("IPC accept error: {}", e),
            }
        }
    });

    Ok(state)
}

/// Cap on a single request line. Real requests are under 100 bytes; an
/// unbounded read_line would let a client grow the daemon's memory without
/// limit. Mostly moot while the socket is group-restricted (0660), but
/// cheap insurance in case those permissions are ever loosened.
const MAX_REQUEST_BYTES: u64 = 64 * 1024;

/// Where the config lives, plus a lock so two SetMode requests (from two
/// clients) never interleave their read-modify-write of the file.
pub struct ConfigCtx {
    pub path: PathBuf,
    pub write_lock: Mutex<()>,
}

/// Rewrite only `[mode] active` in the config file. Written to a temp file
/// in the same directory and renamed over the original, so a crash never
/// leaves a half-written config; the temp file gets the original's owner,
/// group and permissions first (root:nzxt-ctl 0664 per the tmpfiles rule),
/// so the GUI can still save curves afterwards.
fn write_active_mode(path: &Path, mode: Mode) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let text = std::fs::read_to_string(path).with_context(|| format!("read {:?}", path))?;
    let new_text = config::set_active_mode(&text, mode)?;
    if new_text == text {
        return Ok(());
    }
    let meta = std::fs::metadata(path)?;
    let tmp = path.with_extension("toml.tmp");
    {
        let mut f = std::fs::File::create(&tmp).with_context(|| format!("create {:?}", tmp))?;
        f.write_all(new_text.as_bytes())?;
        f.sync_all()?;
    }
    // chown needs root (the daemon); when not root (tests), keep going with
    // our own ownership rather than failing the whole request.
    if let Err(e) = std::os::unix::fs::chown(&tmp, Some(meta.uid()), Some(meta.gid())) {
        log::debug!("chown {:?} failed: {}", tmp, e);
    }
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(meta.mode() & 0o7777))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename {:?} -> {:?}", tmp, path))?;
    Ok(())
}

fn handle_client(
    stream: UnixStream,
    state: SharedState,
    reload_flag: Arc<std::sync::atomic::AtomicBool>,
    ctx: &ConfigCtx,
) -> Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream).take(MAX_REQUEST_BYTES);
    let mut line = String::new();

    loop {
        line.clear();
        reader.set_limit(MAX_REQUEST_BYTES);
        if reader.read_line(&mut line)? == 0 {
            break; // EOF - client disconnected
        }
        // read_line stopping short of a newline with the limit exhausted
        // means the line is oversized; drop the client rather than trying
        // to resynchronise mid-line. (No newline with limit remaining is
        // just a final unterminated line at EOF - process it normally.)
        if !line.ends_with('\n') && reader.limit() == 0 {
            anyhow::bail!("request exceeded {} bytes - dropping client", MAX_REQUEST_BYTES);
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(Request::GetState) => {
                let snapshot = state.lock().unwrap().clone();
                Response::Ok { state: snapshot }
            }
            Ok(Request::ReloadConfig) => {
                // NOTE: this only flags the main loop to reload on its next
                // iteration (see main.rs) - it does not itself re-parse or
                // validate the config, so a malformed file will surface as
                // an error in the daemon's own log, not in this response.
                reload_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Response::ReloadOk
            }
            Ok(Request::SetMode { mode }) => {
                let _guard = ctx.write_lock.lock().unwrap();
                match write_active_mode(&ctx.path, mode) {
                    Ok(()) => {
                        log::info!("mode set to {} over IPC", mode.label());
                        reload_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        Response::ModeSet { mode: mode.label().to_string() }
                    }
                    Err(e) => {
                        log::error!("SetMode {} failed, config unchanged: {:#}", mode.label(), e);
                        Response::Error { message: format!("set mode failed: {:#}", e) }
                    }
                }
            }
            Err(e) => Response::Error {
                message: format!("invalid request: {}", e),
            },
        };
        let json = serde_json::to_string(&response)?;
        writeln!(writer, "{}", json)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    const CONFIG: &str = r#"poll_interval_ms = 1000

[hwmon]
device_name = "kraken2023elite"

[mode]
# comment kept
active = "auto"
silent_duty_pct = 40
failsafe_temp_c = 60.0

[pump_curve]
points = [{ temp_c = 20.0, duty_pct = 50 }, { temp_c = 55.0, duty_pct = 100 }]

[fan_curve]
points = [{ temp_c = 20.0, duty_pct = 30 }, { temp_c = 55.0, duty_pct = 100 }]
"#;

    /// Send one request line to handle_client over a socket pair; return the reply line.
    fn roundtrip(ctx: &ConfigCtx, reload: &Arc<AtomicBool>, req: &str) -> String {
        let (mut client, server) = UnixStream::pair().unwrap();
        let state: SharedState = Arc::new(Mutex::new(LiveState::default()));
        writeln!(client, "{}", req).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        handle_client(server, state, reload.clone(), ctx).unwrap();
        let mut out = String::new();
        BufReader::new(client).read_line(&mut out).unwrap();
        out
    }

    #[test]
    fn set_mode_rewrites_only_active_keeps_permissions_and_requests_reload() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("nzxt-ipc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, CONFIG).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
        let ctx = ConfigCtx { path: path.clone(), write_lock: Mutex::new(()) };
        let reload = Arc::new(AtomicBool::new(false));

        let reply = roundtrip(&ctx, &reload, r#"{"action":"SetMode","mode":"silent"}"#);
        assert_eq!(reply.trim(), r#"{"status":"ModeSet","mode":"Silent"}"#);
        assert!(reload.load(Ordering::SeqCst));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, CONFIG.replace(r#"active = "auto""#, r#"active = "silent""#));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o664);
        assert!(!dir.join("config.toml.tmp").exists());

        // unknown mode: rejected by serde, file untouched
        reload.store(false, Ordering::SeqCst);
        let reply = roundtrip(&ctx, &reload, r#"{"action":"SetMode","mode":"turbo"}"#);
        assert!(reply.contains(r#""status":"Error""#), "{}", reply);
        assert!(!reload.load(Ordering::SeqCst));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

        // an existing client's GetState still works the same
        let reply = roundtrip(&ctx, &reload, r#"{"action":"GetState"}"#);
        assert!(reply.starts_with(r#"{"status":"Ok","state":"#), "{}", reply);
    }

    #[test]
    fn set_mode_refuses_to_touch_a_config_the_daemon_would_reject() {
        let dir = std::env::temp_dir().join(format!("nzxt-ipc-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let bad = CONFIG.replace("failsafe_temp_c = 60.0", "failsafe_temp_c = 6.0");
        std::fs::write(&path, &bad).unwrap();
        let ctx = ConfigCtx { path: path.clone(), write_lock: Mutex::new(()) };
        let reload = Arc::new(AtomicBool::new(false));
        let reply = roundtrip(&ctx, &reload, r#"{"action":"SetMode","mode":"performance"}"#);
        assert!(reply.contains("suspiciously low"), "{}", reply);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
        assert!(!reload.load(Ordering::SeqCst));
    }
}
