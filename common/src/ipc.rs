//! Wire format for the daemon's Unix socket. The daemon serves it
//! (daemon/src/ipc.rs), the GUI consumes it (gui/src/ipc_client.rs); both
//! sides derive Serialize AND Deserialize on every type so neither side can
//! drift from the other.

use serde::{Deserialize, Serialize};

pub const SOCKET_PATH: &str = "/run/nzxt-ctl/daemon.sock";

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

/// Requests the GUI can send over the socket. Kept minimal for v1 - just
/// reading state and reloading config after an external edit. Direct
/// curve-point mutation over IPC is a deliberate NOT-yet-implemented gap:
/// see the note in the daemon's handle_client.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum Request {
    GetState,
    /// GUI writes /etc/nzxt-ctl/config.toml directly (it needs to anyway,
    /// so the config survives daemon restarts), then asks the daemon to
    /// pick up the change without a full restart.
    ReloadConfig,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum Response {
    Ok { state: LiveState },
    ReloadOk,
    Error { message: String },
}
