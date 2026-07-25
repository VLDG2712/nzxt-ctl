//! GUI-side config I/O. The schema itself lives in nzxt-ctl-common (shared
//! with the daemon); this module only adds the GUI's path override and
//! load/save helpers.

use anyhow::{Context, Result};
use std::path::PathBuf;

pub use nzxt_ctl_common::config::{
    ChannelCurve, Config, CurvePoint, HwmonPaths, Mode, ModeConfig, TempSource,
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
    std::fs::write(&path, text).with_context(|| format!("writing config to {:?}", path))?;
    Ok(())
}
