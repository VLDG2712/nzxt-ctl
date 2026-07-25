use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Performance,
    Silent,
    Auto,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Auto
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeConfig {
    #[serde(default)]
    pub active: Mode,
    #[serde(default = "default_silent_duty")]
    pub silent_duty_pct: u8,
    #[serde(default = "default_failsafe_temp")]
    pub failsafe_temp_c: f32,
}

fn default_silent_duty() -> u8 {
    40
}

fn default_failsafe_temp() -> f32 {
    60.0
}

impl Default for ModeConfig {
    fn default() -> Self {
        Self {
            active: Mode::default(),
            silent_duty_pct: default_silent_duty(),
            failsafe_temp_c: default_failsafe_temp(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TempSource {
    Liquid,
    Cpu,
    Gpu,
}

impl Default for TempSource {
    fn default() -> Self {
        TempSource::Liquid
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurvePoint {
    pub temp_c: f32,
    pub duty_pct: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelCurve {
    #[serde(default)]
    pub temp_source: TempSource,
    pub points: Vec<CurvePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HwmonPaths {
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub hwmon: HwmonPaths,
    #[serde(default)]
    pub mode: ModeConfig,
    pub pump_curve: ChannelCurve,
    pub fan_curve: ChannelCurve,
    pub poll_interval_ms: u64,
}

pub fn config_path() -> PathBuf {
    std::env::var("NZXT_CTL_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/etc/nzxt-ctl/config.toml"))
}

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
pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path();
    let text = toml::to_string_pretty(cfg).context("serializing config to TOML")?;
    std::fs::write(&path, text).with_context(|| format!("writing config to {:?}", path))?;
    Ok(())
}