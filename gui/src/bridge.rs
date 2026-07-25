use crate::{config, ipc_client};
use core::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        /// Single QML-facing object exposing the daemon's live state plus
        /// the editable config. Registered as a QML singleton so QML can
        /// reach it as `DaemonBridge.*` without instantiating anything.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // Live readings, pre-formatted for display so QML stays free of
        // Option/unit-formatting logic (mirrors the fmt_* helpers the old
        // GTK frontend used).
        #[qproperty(QString, liquid_temp, cxx_name = "liquidTemp")]
        #[qproperty(QString, cpu_temp, cxx_name = "cpuTemp")]
        #[qproperty(QString, gpu_temp, cxx_name = "gpuTemp")]
        #[qproperty(QString, pump_status, cxx_name = "pumpStatus")]
        #[qproperty(QString, fan_status, cxx_name = "fanStatus")]
        #[qproperty(bool, failsafe_active, cxx_name = "failsafeActive")]
        /// Empty when the daemon is reachable; otherwise the connection
        /// error, so QML can show an inline message instead of silently
        /// displaying stale values.
        #[qproperty(QString, daemon_error, cxx_name = "daemonError")]
        // Editable config, round-tripped through the same TOML file the
        // daemon reads.
        #[qproperty(QString, mode)]
        /// Both curves as JSON, since QML parses JSON natively and this
        /// avoids hand-rolling QVariantList<QVariantMap> marshalling for
        /// what is really just a small list of {temp_c, duty_pct} pairs.
        #[qproperty(QString, curves_json, cxx_name = "curvesJson")]
        #[qproperty(QString, status_message, cxx_name = "statusMessage")]
        type DaemonBridge = super::DaemonBridgeRust;

        /// Polls the daemon over the Unix socket and refreshes every live
        /// property. Driven by a QML Timer (replaces the GTK version's
        /// glib::timeout_add_local).
        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);

        /// Writes mode + curves back to the TOML config, then asks the
        /// daemon to reload it.
        #[qinvokable]
        fn save(self: Pin<&mut Self>);
    }
}

/// Serialisable view of one curve, used only as the JSON shape handed to
/// and received from QML.
#[derive(serde::Serialize, serde::Deserialize)]
struct CurveJson {
    temp_source: String,
    points: Vec<PointJson>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PointJson {
    temp_c: f32,
    duty_pct: u8,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CurvesJson {
    pump: CurveJson,
    fan: CurveJson,
}

pub struct DaemonBridgeRust {
    liquid_temp: QString,
    cpu_temp: QString,
    gpu_temp: QString,
    pump_status: QString,
    fan_status: QString,
    failsafe_active: bool,
    daemon_error: QString,
    mode: QString,
    curves_json: QString,
    status_message: QString,
    /// Last successfully loaded config, kept so `save` can write back a
    /// complete file (preserving fields the GUI doesn't expose, e.g.
    /// hwmon.device_name and poll_interval_ms) rather than reconstructing
    /// a partial one. `None` if the config failed to load at startup.
    cfg: Option<config::Config>,
}

impl Default for DaemonBridgeRust {
    fn default() -> Self {
        let (cfg, mode, curves_json, status_message) = match config::load() {
            Ok(cfg) => {
                let mode = source_mode_str(cfg.mode.active).to_string();
                let curves = curves_to_json(&cfg);
                (Some(cfg), mode, curves, String::new())
            }
            Err(e) => (
                None,
                "auto".to_string(),
                empty_curves_json(),
                format!("Failed to load config: {e}"),
            ),
        };

        Self {
            liquid_temp: QString::from("-- °C"),
            cpu_temp: QString::from("-- °C"),
            gpu_temp: QString::from("-- °C"),
            pump_status: QString::from("-- rpm (--%)"),
            fan_status: QString::from("-- rpm (--%)"),
            failsafe_active: false,
            daemon_error: QString::from(""),
            mode: QString::from(&mode),
            curves_json: QString::from(&curves_json),
            status_message: QString::from(&status_message),
            cfg,
        }
    }
}

impl qobject::DaemonBridge {
    pub fn refresh(mut self: Pin<&mut Self>) {
        match ipc_client::get_state() {
            Ok(state) => {
                self.as_mut().set_liquid_temp(QString::from(&fmt_temp(state.liquid_temp_c)));
                self.as_mut().set_cpu_temp(QString::from(&fmt_temp(state.cpu_temp_c)));
                self.as_mut().set_gpu_temp(QString::from(&fmt_temp(state.gpu_temp_c)));
                self.as_mut().set_pump_status(QString::from(&format!(
                    "{} ({})",
                    fmt_rpm(state.pump_rpm),
                    fmt_pct(state.pump_duty_pct)
                )));
                self.as_mut().set_fan_status(QString::from(&format!(
                    "{} ({})",
                    fmt_rpm(state.fan_rpm),
                    fmt_pct(state.fan_duty_pct)
                )));
                self.as_mut().set_failsafe_active(state.failsafe_active);
                self.as_mut().set_daemon_error(QString::from(""));
            }
            Err(e) => {
                // Leave the last-known readings in place rather than
                // blanking them - the error property is what tells the
                // user the values have gone stale.
                self.as_mut().set_daemon_error(QString::from(&format!("{e}")));
            }
        }
    }

    pub fn save(mut self: Pin<&mut Self>) {
        let Some(mut cfg) = self.cfg.clone() else {
            self.as_mut().set_status_message(QString::from(
                "Cannot save: config was never loaded successfully",
            ));
            return;
        };

        let mode_str = self.mode().to_string();
        let Some(mode) = parse_mode(&mode_str) else {
            self.as_mut()
                .set_status_message(QString::from(&format!("Unknown mode: {mode_str}")));
            return;
        };
        cfg.mode.active = mode;

        let curves_str = self.curves_json().to_string();
        let curves: CurvesJson = match serde_json::from_str(&curves_str) {
            Ok(c) => c,
            Err(e) => {
                self.as_mut()
                    .set_status_message(QString::from(&format!("Invalid curve data: {e}")));
                return;
            }
        };

        match (
            apply_curve(&mut cfg.pump_curve, &curves.pump),
            apply_curve(&mut cfg.fan_curve, &curves.fan),
        ) {
            (Ok(()), Ok(())) => {}
            (Err(e), _) | (_, Err(e)) => {
                self.as_mut().set_status_message(QString::from(&e));
                return;
            }
        }

        let message = match config::save(&cfg) {
            Ok(()) => match ipc_client::request_reload() {
                Ok(()) => "Saved and reloaded.".to_string(),
                Err(e) => format!("Saved, but daemon reload failed: {e}"),
            },
            Err(e) => format!("Save failed: {e}"),
        };

        // Keep the in-memory copy in sync so a subsequent save doesn't
        // silently revert this one.
        self.as_mut().rust_mut().cfg = Some(cfg);
        self.as_mut().set_status_message(QString::from(&message));
    }
}

/// Points must be sorted ascending by temp for the daemon's interpolation
/// to behave correctly - same invariant the GTK frontend enforced on save.
fn apply_curve(target: &mut config::ChannelCurve, src: &CurveJson) -> Result<(), String> {
    let Some(temp_source) = parse_temp_source(&src.temp_source) else {
        return Err(format!("Unknown temp source: {}", src.temp_source));
    };
    if src.points.is_empty() {
        return Err("A curve must have at least one point".to_string());
    }
    let mut points: Vec<config::CurvePoint> = src
        .points
        .iter()
        .map(|p| config::CurvePoint {
            temp_c: p.temp_c,
            duty_pct: p.duty_pct.min(100),
        })
        .collect();
    points.sort_by(|a, b| a.temp_c.partial_cmp(&b.temp_c).unwrap());
    target.temp_source = temp_source;
    target.points = points;
    Ok(())
}

fn curves_to_json(cfg: &config::Config) -> String {
    let to_curve = |c: &config::ChannelCurve| CurveJson {
        temp_source: temp_source_str(c.temp_source).to_string(),
        points: c
            .points
            .iter()
            .map(|p| PointJson {
                temp_c: p.temp_c,
                duty_pct: p.duty_pct,
            })
            .collect(),
    };
    serde_json::to_string(&CurvesJson {
        pump: to_curve(&cfg.pump_curve),
        fan: to_curve(&cfg.fan_curve),
    })
    .unwrap_or_else(|_| empty_curves_json())
}

fn empty_curves_json() -> String {
    r#"{"pump":{"temp_source":"liquid","points":[]},"fan":{"temp_source":"liquid","points":[]}}"#
        .to_string()
}

fn temp_source_str(s: config::TempSource) -> &'static str {
    match s {
        config::TempSource::Liquid => "liquid",
        config::TempSource::Cpu => "cpu",
        config::TempSource::Gpu => "gpu",
    }
}

fn parse_temp_source(s: &str) -> Option<config::TempSource> {
    match s {
        "liquid" => Some(config::TempSource::Liquid),
        "cpu" => Some(config::TempSource::Cpu),
        "gpu" => Some(config::TempSource::Gpu),
        _ => None,
    }
}

fn source_mode_str(m: config::Mode) -> &'static str {
    match m {
        config::Mode::Performance => "performance",
        config::Mode::Silent => "silent",
        config::Mode::Auto => "auto",
    }
}

fn parse_mode(s: &str) -> Option<config::Mode> {
    match s {
        "performance" => Some(config::Mode::Performance),
        "silent" => Some(config::Mode::Silent),
        "auto" => Some(config::Mode::Auto),
        _ => None,
    }
}

fn fmt_temp(v: Option<f32>) -> String {
    match v {
        Some(t) => format!("{t:.1} °C"),
        None => "-- °C".to_string(),
    }
}

fn fmt_rpm(v: Option<u32>) -> String {
    match v {
        Some(r) => format!("{r} rpm"),
        None => "-- rpm".to_string(),
    }
}

fn fmt_pct(v: Option<u8>) -> String {
    match v {
        Some(p) => format!("{p}%"),
        None => "--%".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_curve() -> config::ChannelCurve {
        config::ChannelCurve {
            temp_source: config::TempSource::Cpu,
            points: vec![
                config::CurvePoint { temp_c: 20.0, duty_pct: 30 },
                config::CurvePoint { temp_c: 55.0, duty_pct: 90 },
            ],
        }
    }

    /// The JSON handed to QML must survive the trip back through
    /// apply_curve unchanged - this is the whole contract between the QML
    /// editor and the TOML the daemon reads.
    #[test]
    fn curve_json_round_trips() {
        let cfg = config::Config {
            hwmon: config::HwmonPaths { device_name: "kraken2023elite".into() },
            mode: config::ModeConfig::default(),
            pump_curve: sample_curve(),
            fan_curve: sample_curve(),
            poll_interval_ms: 1000,
        };

        let json = curves_to_json(&cfg);
        let parsed: CurvesJson = serde_json::from_str(&json).unwrap();

        let mut target = sample_curve();
        target.points.clear();
        apply_curve(&mut target, &parsed.pump).unwrap();

        assert_eq!(target.temp_source, config::TempSource::Cpu);
        assert_eq!(target.points.len(), 2);
        assert_eq!(target.points[0].temp_c, 20.0);
        assert_eq!(target.points[0].duty_pct, 30);
        assert_eq!(target.points[1].duty_pct, 90);
    }

    /// QML lets the user reorder/add rows freely, so unsorted input is
    /// expected; the daemon's interpolation requires ascending temps.
    #[test]
    fn apply_curve_sorts_points_ascending() {
        let src = CurveJson {
            temp_source: "liquid".into(),
            points: vec![
                PointJson { temp_c: 55.0, duty_pct: 90 },
                PointJson { temp_c: 20.0, duty_pct: 30 },
                PointJson { temp_c: 35.0, duty_pct: 50 },
            ],
        };
        let mut target = sample_curve();
        apply_curve(&mut target, &src).unwrap();
        let temps: Vec<f32> = target.points.iter().map(|p| p.temp_c).collect();
        assert_eq!(temps, vec![20.0, 35.0, 55.0]);
    }

    #[test]
    fn apply_curve_rejects_empty_and_unknown_source() {
        let mut target = sample_curve();

        let empty = CurveJson { temp_source: "liquid".into(), points: vec![] };
        assert!(apply_curve(&mut target, &empty).is_err());

        let bad_source = CurveJson {
            temp_source: "nonsense".into(),
            points: vec![PointJson { temp_c: 20.0, duty_pct: 30 }],
        };
        assert!(apply_curve(&mut target, &bad_source).is_err());
    }

    #[test]
    fn mode_strings_round_trip() {
        for m in [config::Mode::Performance, config::Mode::Silent, config::Mode::Auto] {
            assert_eq!(parse_mode(source_mode_str(m)), Some(m));
        }
        assert_eq!(parse_mode("bogus"), None);
    }

    #[test]
    fn temp_source_strings_round_trip() {
        for s in [config::TempSource::Liquid, config::TempSource::Cpu, config::TempSource::Gpu] {
            assert_eq!(parse_temp_source(temp_source_str(s)), Some(s));
        }
        assert_eq!(parse_temp_source("bogus"), None);
    }
}
