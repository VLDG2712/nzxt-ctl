//! Toolkit-free GUI logic: mapping between the shared config schema and the
//! plain values the Slint UI holds (combo indices, ints, display strings),
//! plus building a validated-ready Config from the UI's edits.
//!
//! Everything here is pure so it can be unit tested without a display.
//! main.rs only shuttles values between these functions and the window.

use crate::config;
use nzxt_ctl_common::ipc::LiveState;

/// Upper end of the curve editor's temperature axis (°C).
pub const TEMP_MAX: f32 = 150.0;

/// Order of the mode selector buttons in the UI.
pub const MODES: [config::Mode; 3] =
    [config::Mode::Performance, config::Mode::Silent, config::Mode::Auto];

/// Order of every temperature-source combo box in the UI.
pub const SOURCES: [config::TempSource; 3] =
    [config::TempSource::Liquid, config::TempSource::Cpu, config::TempSource::Gpu];

pub fn mode_index(m: config::Mode) -> i32 {
    MODES.iter().position(|x| *x == m).unwrap_or(2) as i32
}

pub fn mode_from_index(i: i32) -> Option<config::Mode> {
    usize::try_from(i).ok().and_then(|i| MODES.get(i).copied())
}

pub fn source_index(s: config::TempSource) -> i32 {
    SOURCES.iter().position(|x| *x == s).unwrap_or(0) as i32
}

pub fn source_from_index(i: i32) -> Option<config::TempSource> {
    usize::try_from(i).ok().and_then(|i| SOURCES.get(i).copied())
}

/// One curve as edited in the UI: (temp °C, duty %). Kept unsorted while
/// editing - dragged points may cross freely; `apply_curve` sorts on save.
#[derive(Debug, Clone, PartialEq)]
pub struct EditedCurve {
    pub source_index: i32,
    pub points: Vec<(f32, i32)>,
}

impl EditedCurve {
    pub fn from_config(c: &config::ChannelCurve) -> Self {
        Self {
            source_index: source_index(c.temp_source),
            points: c.points.iter().map(|p| (p.temp_c, p.duty_pct as i32)).collect(),
        }
    }
}

/// Everything editable in the main window, as plain values.
#[derive(Debug, Clone, PartialEq)]
pub struct Edits {
    pub mode_index: i32,
    pub silent_duty: i32,
    pub failsafe_temp: i32,
    pub lcd_enabled: bool,
    pub lcd_source_index: i32,
    pub pump: EditedCurve,
    pub fan: EditedCurve,
}

impl Edits {
    pub fn from_config(cfg: &config::Config) -> Self {
        Self {
            mode_index: mode_index(cfg.mode.active),
            silent_duty: cfg.mode.silent_duty_pct as i32,
            failsafe_temp: cfg.mode.failsafe_temp_c.round() as i32,
            lcd_enabled: cfg.lcd.enabled,
            lcd_source_index: source_index(cfg.lcd.source),
            pump: EditedCurve::from_config(&cfg.pump_curve),
            fan: EditedCurve::from_config(&cfg.fan_curve),
        }
    }
}

/// Applies the UI's edits on top of the last loaded config, so fields the
/// GUI doesn't expose (hwmon.device_name, poll_interval_ms, ...) are
/// written back untouched. Range validation is left to the shared
/// Config::validate() that config::save runs - a visible error beats
/// silently writing a different number than the user sees.
pub fn build_config(base: &config::Config, e: &Edits) -> Result<config::Config, String> {
    let mut cfg = base.clone();
    cfg.mode.active =
        mode_from_index(e.mode_index).ok_or_else(|| format!("Unknown mode #{}", e.mode_index))?;
    cfg.mode.silent_duty_pct = e.silent_duty.clamp(0, 100) as u8;
    cfg.mode.failsafe_temp_c = e.failsafe_temp as f32;
    cfg.lcd.enabled = e.lcd_enabled;
    cfg.lcd.source = source_from_index(e.lcd_source_index)
        .ok_or_else(|| format!("Unknown LCD source #{}", e.lcd_source_index))?;
    apply_curve(&mut cfg.pump_curve, &e.pump).map_err(|m| format!("Pump curve: {m}"))?;
    apply_curve(&mut cfg.fan_curve, &e.fan).map_err(|m| format!("Fan curve: {m}"))?;
    Ok(cfg)
}

/// Points must be sorted ascending by temp for the daemon's interpolation
/// to behave correctly.
pub fn apply_curve(target: &mut config::ChannelCurve, src: &EditedCurve) -> Result<(), String> {
    let temp_source = source_from_index(src.source_index)
        .ok_or_else(|| format!("unknown temp source #{}", src.source_index))?;
    if src.points.is_empty() {
        return Err("a curve must have at least one point".to_string());
    }
    let mut points: Vec<config::CurvePoint> = src
        .points
        .iter()
        .map(|&(t, d)| config::CurvePoint {
            temp_c: t.clamp(0.0, TEMP_MAX),
            duty_pct: d.clamp(0, 100) as u8,
        })
        .collect();
    points.sort_by(|a, b| a.temp_c.total_cmp(&b.temp_c));
    target.temp_source = temp_source;
    target.points = points;
    Ok(())
}

/// SVG path commands for the curve line, in data units: x = °C on
/// 0..TEMP_MAX, y = 100 - duty (so 100 % is at the top). The UI draws it
/// in a Path whose viewbox is TEMP_MAX x 100, which scales it to the plot.
/// The line is extended flat to both edges to match duty_for_temp's
/// clamping outside the point range.
pub fn curve_path(points: &[(f32, i32)]) -> String {
    if points.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<(f32, i32)> = points.to_vec();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let y = |d: i32| 100 - d.clamp(0, 100);
    let mut s = format!("M 0 {}", y(sorted[0].1));
    for (t, d) in &sorted {
        s.push_str(&format!(" L {} {}", t.clamp(0.0, TEMP_MAX), y(*d)));
    }
    s.push_str(&format!(" L {} {}", TEMP_MAX, y(sorted[sorted.len() - 1].1)));
    s
}

/// Where a point added from the numeric editor's "Add point" button goes:
/// a step past the last point, so a new row starts somewhere sensible
/// rather than at 0 °C / 0 %.
pub fn next_point(points: &[(f32, i32)]) -> (f32, i32) {
    match points.last() {
        Some(&(t, d)) => ((t + 5.0).min(TEMP_MAX), (d + 10).min(100)),
        None => (20.0, 50),
    }
}

/// Pre-formatted live readings for the status tiles.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveText {
    pub liquid: String,
    pub cpu: String,
    pub gpu: String,
    pub pump: String,
    pub fan: String,
}

impl Default for LiveText {
    fn default() -> Self {
        Self {
            liquid: fmt_temp(None),
            cpu: fmt_temp(None),
            gpu: fmt_temp(None),
            pump: fmt_channel(None, None),
            fan: fmt_channel(None, None),
        }
    }
}

impl LiveText {
    pub fn from_state(s: &LiveState) -> Self {
        Self {
            liquid: fmt_temp(s.liquid_temp_c),
            cpu: fmt_temp(s.cpu_temp_c),
            gpu: fmt_temp(s.gpu_temp_c),
            pump: fmt_channel(s.pump_rpm, s.pump_duty_pct),
            fan: fmt_channel(s.fan_rpm, s.fan_duty_pct),
        }
    }
}

pub fn fmt_temp(v: Option<f32>) -> String {
    match v {
        Some(t) => format!("{t:.1} °C"),
        None => "-- °C".to_string(),
    }
}

pub fn fmt_channel(rpm: Option<u32>, duty: Option<u8>) -> String {
    let rpm = rpm.map_or_else(|| "--".to_string(), |r| r.to_string());
    let duty = duty.map_or_else(|| "--".to_string(), |d| d.to_string());
    format!("{rpm} rpm · {duty}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_curve() -> config::ChannelCurve {
        config::ChannelCurve {
            temp_source: config::TempSource::Cpu,
            points: vec![
                config::CurvePoint { temp_c: 20.0, duty_pct: 30 },
                config::CurvePoint { temp_c: 55.5, duty_pct: 90 },
            ],
        }
    }

    fn sample_config() -> config::Config {
        config::Config {
            hwmon: config::HwmonPaths { device_name: "kraken2023elite".into() },
            mode: config::ModeConfig::default(),
            pump_curve: sample_curve(),
            fan_curve: sample_curve(),
            poll_interval_ms: 1234,
            lcd: config::LcdConfig::default(),
        }
    }

    /// Loading a config into the UI values and building it straight back
    /// must not change anything - this is the whole contract between the
    /// editor and the TOML the daemon reads (fractional temps included).
    #[test]
    fn edits_round_trip_unchanged() {
        let cfg = sample_config();
        let rebuilt = build_config(&cfg, &Edits::from_config(&cfg)).unwrap();
        assert_eq!(rebuilt.pump_curve.points.len(), 2);
        assert_eq!(rebuilt.pump_curve.points[1].temp_c, 55.5);
        assert_eq!(rebuilt.pump_curve.temp_source, config::TempSource::Cpu);
        assert_eq!(rebuilt.mode.active, cfg.mode.active);
        assert_eq!(rebuilt.lcd.source, cfg.lcd.source);
        // Fields the GUI never shows are preserved
        assert_eq!(rebuilt.poll_interval_ms, 1234);
        assert_eq!(rebuilt.hwmon.device_name, "kraken2023elite");
    }

    #[test]
    fn build_config_applies_edits() {
        let cfg = sample_config();
        let mut e = Edits::from_config(&cfg);
        e.mode_index = mode_index(config::Mode::Silent);
        e.silent_duty = 140; // clamped
        e.failsafe_temp = 55;
        e.lcd_enabled = true;
        e.lcd_source_index = source_index(config::TempSource::Gpu);
        let out = build_config(&cfg, &e).unwrap();
        assert_eq!(out.mode.active, config::Mode::Silent);
        assert_eq!(out.mode.silent_duty_pct, 100);
        assert_eq!(out.mode.failsafe_temp_c, 55.0);
        assert!(out.lcd.enabled);
        assert_eq!(out.lcd.source, config::TempSource::Gpu);
    }

    /// The editor lets points cross freely while dragging, so unsorted
    /// input is expected; the daemon's interpolation requires ascending.
    #[test]
    fn apply_curve_sorts_points_ascending() {
        let src = EditedCurve {
            source_index: 0,
            points: vec![(55.0, 90), (20.0, 30), (35.0, 50)],
        };
        let mut target = sample_curve();
        apply_curve(&mut target, &src).unwrap();
        let temps: Vec<f32> = target.points.iter().map(|p| p.temp_c).collect();
        assert_eq!(temps, vec![20.0, 35.0, 55.0]);
        assert_eq!(target.temp_source, config::TempSource::Liquid);
    }

    #[test]
    fn apply_curve_rejects_empty_and_unknown_source() {
        let mut target = sample_curve();
        let empty = EditedCurve { source_index: 0, points: vec![] };
        assert!(apply_curve(&mut target, &empty).is_err());
        let bad = EditedCurve { source_index: 7, points: vec![(20.0, 30)] };
        assert!(apply_curve(&mut target, &bad).is_err());
    }

    #[test]
    fn index_mappings_round_trip() {
        for m in MODES {
            assert_eq!(mode_from_index(mode_index(m)), Some(m));
        }
        for s in SOURCES {
            assert_eq!(source_from_index(source_index(s)), Some(s));
        }
        assert_eq!(mode_from_index(-1), None);
        assert_eq!(mode_from_index(3), None);
        assert_eq!(source_from_index(3), None);
    }

    #[test]
    fn curve_path_is_sorted_and_extended_to_edges() {
        assert_eq!(curve_path(&[]), "");
        assert_eq!(
            curve_path(&[(60.0, 100), (30.0, 40)]),
            "M 0 60 L 30 60 L 60 0 L 150 0"
        );
    }

    #[test]
    fn next_point_steps_and_clamps() {
        assert_eq!(next_point(&[]), (20.0, 50));
        assert_eq!(next_point(&[(30.0, 40)]), (35.0, 50));
        assert_eq!(next_point(&[(148.0, 95)]), (150.0, 100));
    }

    #[test]
    fn live_text_formats_missing_values() {
        let t = LiveText::default();
        assert_eq!(t.liquid, "-- °C");
        assert_eq!(t.pump, "-- rpm · --%");
        let s = LiveState {
            liquid_temp_c: Some(32.04),
            pump_rpm: Some(2189),
            pump_duty_pct: Some(57),
            ..Default::default()
        };
        let t = LiveText::from_state(&s);
        assert_eq!(t.liquid, "32.0 °C");
        assert_eq!(t.pump, "2189 rpm · 57%");
    }
}
