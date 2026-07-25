use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

const SOCKET_PATH: &str = "/run/nzxt-ctl/daemon.sock";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LiveState {
    pub liquid_temp_c: Option<f32>,
    pub cpu_temp_c: Option<f32>,
    pub gpu_temp_c: Option<f32>,
    pub pump_rpm: Option<u32>,
    pub fan_rpm: Option<u32>,
    pub pump_duty_pct: Option<u8>,
    pub fan_duty_pct: Option<u8>,
    pub active_mode: Option<String>,
    pub failsafe_active: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "action")]
enum Request {
    GetState,
    ReloadConfig,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status")]
enum Response {
    Ok { state: LiveState },
    ReloadOk,
    Error { message: String },
}

/// Opens a fresh connection per call rather than holding one open. Simpler
/// and avoids stale-connection bugs if the daemon restarts - the overhead
/// of a Unix socket connect is negligible at GUI polling rates (~1/sec).
fn send_request(req: &Request) -> Result<Response> {
    let mut stream = UnixStream::connect(SOCKET_PATH)
        .with_context(|| format!("connecting to daemon socket at {}", SOCKET_PATH))?;
    let json = serde_json::to_string(req)?;
    writeln!(stream, "{}", json)?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let response: Response = serde_json::from_str(&line)
        .with_context(|| format!("parsing daemon response: {:?}", line))?;
    Ok(response)
}

pub fn get_state() -> Result<LiveState> {
    match send_request(&Request::GetState)? {
        Response::Ok { state } => Ok(state),
        Response::Error { message } => anyhow::bail!("daemon error: {}", message),
        _ => anyhow::bail!("unexpected response type for GetState"),
    }
}

pub fn request_reload() -> Result<()> {
    match send_request(&Request::ReloadConfig)? {
        Response::ReloadOk => Ok(()),
        Response::Error { message } => anyhow::bail!("daemon error: {}", message),
        _ => anyhow::bail!("unexpected response type for ReloadConfig"),
    }
}