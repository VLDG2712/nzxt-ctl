use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// Snapshot of live state, shared between the control loop (writer) and
/// any number of connected GUI clients (readers). Deliberately simple -
/// one struct, one mutex, no per-field locking, since update frequency
/// (~1/sec) and payload size are both small enough that lock contention
/// is not a real concern here.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LiveState {
    pub liquid_temp_c: Option<f32>,
    pub cpu_temp_c: Option<f32>,
    pub gpu_temp_c: Option<f32>,
    pub pump_rpm: Option<u32>,
    pub fan_rpm: Option<u32>,
    pub pump_duty_pct: Option<u8>,
    pub fan_duty_pct: Option<u8>,
    /// Which mode the daemon is actually running, as ground truth - the
    /// GUI displays this rather than assuming its own last-selected value
    /// is still accurate (e.g. after an external config edit or reload).
    pub active_mode: Option<String>,
    /// Whether the safety failsafe is currently overriding the active
    /// mode this cycle - surfaced so the GUI can show a clear warning
    /// rather than silent 100% duty with no explanation.
    pub failsafe_active: bool,
}

pub type SharedState = Arc<Mutex<LiveState>>;

/// Requests the GUI can send over the socket. Kept minimal for v1 - just
/// reading state and reloading config after an external edit. Direct
/// curve-point mutation over IPC is a deliberate NOT-yet-implemented gap:
/// see the note in handle_client below.
#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
enum Request {
    GetState,
    /// GUI writes /etc/nzxt-ctl/config.toml directly (it needs to anyway,
    /// so the config survives daemon restarts), then asks the daemon to
    /// pick up the change without a full restart.
    ReloadConfig,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status")]
enum Response {
    Ok { state: LiveState },
    ReloadOk,
    Error { message: String },
}

const SOCKET_PATH: &str = "/run/nzxt-ctl/daemon.sock";

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

fn handle_client(
    stream: UnixStream,
    state: SharedState,
    reload_flag: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let mut writer = stream.try_clone()?;
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = line?;
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