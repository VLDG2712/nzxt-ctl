use anyhow::{Context, Result};
use nzxt_ctl_common::ipc::{LiveState, Request, Response, SOCKET_PATH};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
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
pub fn start_server(reload_flag: Arc<std::sync::atomic::AtomicBool>) -> Result<SharedState> {
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
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let state = state_clone.clone();
                    let reload_flag = reload_flag.clone();
                    thread::spawn(move || {
                        if let Err(e) = handle_client(stream, state, reload_flag) {
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

fn handle_client(
    stream: UnixStream,
    state: SharedState,
    reload_flag: Arc<std::sync::atomic::AtomicBool>,
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
            Err(e) => Response::Error {
                message: format!("invalid request: {}", e),
            },
        };
        let json = serde_json::to_string(&response)?;
        writeln!(writer, "{}", json)?;
    }
    Ok(())
}