use anyhow::{Context, Result};
use nzxt_ctl_common::ipc::{Request, Response, SOCKET_PATH};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

pub use nzxt_ctl_common::ipc::LiveState;

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
