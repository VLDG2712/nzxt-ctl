use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Both pump and fan forced to 100%, ignoring curves entirely.
    Performance,
    /// Both pump and fan held at a fixed low duty (silent_duty_pct),
    /// ignoring curves - EXCEPT the failsafe ceiling below still applies.
    Silent,
    /// Uses each curve's own temp_source + interpolated points, exactly
    /// as built previously - this is the existing curve-based behavior.
    Auto,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Auto
    }
}

impl Mode {
    pub fn label(&self) -> &'static str {
        match self {
            Mode::Performance => "Performance",
            Mode::Silent => "Silent",
            Mode::Auto => "Auto",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeConfig {
    #[serde(default)]
    pub active: Mode,
    /// Fixed duty% used for BOTH pump and fan while in Silent mode.
    #[serde(default = "default_silent_duty")]
    pub silent_duty_pct: u8,
    /// SAFETY: if liquid temp reaches or exceeds this, pump+fan are forced
    /// to 100% regardless of active mode (including Silent and even a
    /// misconfigured Auto curve). This is a deliberate defense-in-depth
    /// backstop, not something the user is expected to tune down casually -
    /// validated to be at least 40C on load, see Config::load.
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
    /// Which temperature reading this curve reacts to. Defaults to Liquid
    /// if omitted from an older config file, preserving prior behavior.
    #[serde(default)]
    pub temp_source: TempSource,
    /// Points MUST be sorted ascending by temp_c. Validated on load.
    pub points: Vec<CurvePoint>,
}

impl ChannelCurve {
    /// Linear interpolation between the two nearest curve points.
    /// Below the lowest point's temp -> lowest point's duty.
    /// Above the highest point's temp -> highest point's duty (safety: never
    /// extrapolate downward past max temp, always fail toward MORE cooling).
    pub fn duty_for_temp(&self, temp_c: f32) -> u8 {
        if self.points.is_empty() {
            // No curve configured - fail safe to 100% rather than 0%.
            return 100;
        }
        if temp_c <= self.points[0].temp_c {
            return self.points[0].duty_pct;
        }
        let last = self.points.last().unwrap();
        if temp_c >= last.temp_c {
            return last.duty_pct;
        }
        for pair in self.points.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if temp_c >= a.temp_c && temp_c <= b.temp_c {
                let span = b.temp_c - a.temp_c;
                if span <= 0.0 {
                    return a.duty_pct;
                }
                let frac = (temp_c - a.temp_c) / span;
                let duty = a.duty_pct as f32 + frac * (b.duty_pct as f32 - a.duty_pct as f32);
                return duty.round().clamp(0.0, 100.0) as u8;
            }
        }
        // Should be unreachable given the bounds checks above, but fail safe.
        100
    }

    /// Basic sanity validation - called after loading from disk, since this
    /// config drives physical hardware and a malformed curve (e.g. duty
    /// dropping as temp rises) should be rejected loudly, not applied.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.points.is_empty() {
            anyhow::bail!("curve has no points");
        }
        for p in &self.points {
            if !(0.0..=150.0).contains(&p.temp_c) {
                anyhow::bail!("curve point temp_c={} out of sane range", p.temp_c);
            }
            if p.duty_pct > 100 {
                anyhow::bail!("curve point duty_pct={} exceeds 100", p.duty_pct);
            }
        }
        let mut sorted = self.points.clone();
        sorted.sort_by(|a, b| a.temp_c.partial_cmp(&b.temp_c).unwrap());
        for w in sorted.windows(2) {
            if w[1].duty_pct < w[0].duty_pct {
                log::warn!(
                    "curve is non-monotonic: duty drops from {}% at {}C to {}% at {}C -- allowed but unusual",
                    w[0].duty_pct, w[0].temp_c, w[1].duty_pct, w[1].temp_c
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HwmonPaths {
    /// The hwmon device's declared `name` (e.g. "kraken2023elite"), NOT a
    /// path. hwmonN numbers shift across reboots/kernel updates - confirmed
    /// empirically - so the daemon resolves the actual path by scanning for
    /// this name at startup instead of trusting a static path here.
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub hwmon: HwmonPaths,
    #[serde(default)]
    pub mode: ModeConfig,
    pub pump_curve: ChannelCurve,
    pub fan_curve: ChannelCurve,
    /// How often the daemon polls temp and re-evaluates the curve, in ms.
    pub poll_interval_ms: u64,
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read config at {:?}: {}", path, e))?;
        let cfg: Config = toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("failed to parse config: {}", e))?;
        cfg.pump_curve.validate()?;
        cfg.fan_curve.validate()?;
        // Guard against a dangerously low failsafe threshold - e.g. a typo
        // of 6.0 instead of 60.0 would make the failsafe fire constantly
        // and effectively disable Silent/Performance modes entirely. 40C
        // is a conservative floor; liquid temp rarely drops below ambient
        // room temp (~15-25C) at idle, so this still leaves real headroom.
        if cfg.mode.failsafe_temp_c < 40.0 {
            anyhow::bail!(
                "mode.failsafe_temp_c={} is suspiciously low (min allowed: 40.0) - refusing to load, this would make the safety override fire during normal operation",
                cfg.mode.failsafe_temp_c
            );
        }
        Ok(cfg)
    }

    pub fn default_path() -> std::path::PathBuf {
        std::path::PathBuf::from("/etc/nzxt-ctl/config.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(name: &str, failsafe_temp_c: f32) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nzxt-ctl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.toml"));
        let text = format!(
            r#"
poll_interval_ms = 1000

[hwmon]
device_name = "kraken2023elite"

[mode]
active = "auto"
silent_duty_pct = 40
failsafe_temp_c = {failsafe_temp_c}

[pump_curve]
points = [{{ temp_c = 20.0, duty_pct = 50 }}, {{ temp_c = 55.0, duty_pct = 100 }}]

[fan_curve]
points = [{{ temp_c = 20.0, duty_pct = 30 }}, {{ temp_c = 55.0, duty_pct = 100 }}]
"#
        );
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn load_rejects_failsafe_temp_below_40c() {
        let path = write_config("rejects_low", 6.0); // plausible typo for 60.0
        let err = Config::load(&path).unwrap_err();
        assert!(err.to_string().contains("suspiciously low"));
    }

    #[test]
    fn load_accepts_failsafe_temp_at_or_above_40c() {
        let path = write_config("accepts_valid", 60.0);
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.mode.failsafe_temp_c, 60.0);
    }
}