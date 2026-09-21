//! GUI-side config I/O. The schema itself lives in nzxt-ctl-common (shared
//! with the daemon); this module only adds the GUI's path override and
//! load/save helpers.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub use nzxt_ctl_common::config::{
    ChannelCurve, Config, CurvePoint, HwmonPaths, LcdConfig, Mode, ModeConfig, TempSource,
};

pub fn config_path() -> PathBuf {
    std::env::var("NZXT_CTL_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Config::default_path())
}

/// Lenient read: parses without validating, so the GUI can still open and
/// let the user FIX a config the daemon rejects, rather than refusing to
/// start alongside it.
pub fn load() -> Result<Config> {
    let path = config_path();
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading config at {:?}", path))?;
    toml::from_str(&text).with_context(|| "parsing config TOML")
}

/// Writes the config file directly (the GUI needs root/group write access
/// to /etc/nzxt-ctl/config.toml for this to work - see the systemd unit's
/// note about tightening permissions via a dedicated group rather than
/// running everything as root).
///
/// Validates first with the same rules the daemon applies on load, so a
/// config the daemon would reject is caught here - in the UI - instead of
/// surfacing later as a reload error in the daemon's journal.
pub fn save(cfg: &Config) -> Result<()> {
    cfg.validate()?;
    let path = config_path();
    let text = toml::to_string_pretty(cfg).context("serializing config to TOML")?;
    write_atomically(&path, text.as_bytes())
}

/// Write-to-temp-then-rename, so a crash mid-save can never leave a
/// truncated file that the daemon then refuses at its next start. The
/// replacement inherits the original's mode and group (the `nzxt-ctl`
/// group-write that lets any member save) - rename alone would leave a
/// file owned by whoever saved last with their umask.
fn write_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    use std::os::unix::fs::{chown, MetadataExt, PermissionsExt};

    let dir = path.parent().context("config path has no parent directory")?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("config"),
        std::process::id()
    ));
    let result = (|| -> Result<()> {
        std::fs::write(&tmp, contents).with_context(|| format!("writing {:?}", tmp))?;
        if let Ok(meta) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, PermissionsExt::from_mode(meta.mode()))
                .with_context(|| format!("setting mode on {:?}", tmp))?;
            // A member may hand a file to a group it belongs to; if we're
            // not in it the daemon can still read the file, so don't fail.
            let _ = chown(&tmp, None, Some(meta.gid()));
        }
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {:?}", path))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
