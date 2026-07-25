mod control;
mod hwmon;
mod ipc;

use anyhow::Result;
use nzxt_ctl_common::config::{self, Config};
use hwmon::{HwmonChannel, TempSensor};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn main() -> Result<()> {
    env_logger::init();
    log::info!("nzxt-ctl-daemon starting");

    let config_path = std::env::var("NZXT_CTL_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| Config::default_path());

    let mut cfg = Config::load(&config_path)
        .map_err(|e| anyhow::anyhow!("config load failed ({:?}): {}", config_path, e))?;
    log::info!("loaded config from {:?}", config_path);

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        log::info!("shutdown signal received");
        r.store(false, Ordering::SeqCst);
    })?;

    // Resolve the actual hwmon path by device name, since hwmonN numbers
    // shift across reboots (confirmed empirically: hwmon5 -> hwmon2 on the
    // same machine between runs).
    let hwmon_path = hwmon::discover_hwmon_path(&cfg.hwmon.device_name)?;
    let hwmon_path_str = hwmon_path.to_string_lossy().to_string();

    let liquid_temp_sensor = TempSensor::new(&hwmon_path_str, 1);
    let pump = HwmonChannel::new(&hwmon_path_str, 1);
    let fan = HwmonChannel::new(&hwmon_path_str, 2);

    // CPU/GPU sensors are best-effort: if discovery fails (e.g. no NVIDIA
    // GPU present, or an unrecognized CPU hwmon driver name), we log once
    // and continue with that reading simply absent from the live state -
    // this should never block fan/pump control, which only depends on
    // liquid_temp_sensor.
    let cpu_temp_sensor = match hwmon::discover_cpu_temp_sensor() {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("CPU temp sensor unavailable: {}", e);
            None
        }
    };
    let gpu_available = std::process::Command::new("which")
        .arg("nvidia-smi")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !gpu_available {
        log::warn!("nvidia-smi not found in PATH - GPU temp will be unavailable");
    }

    // Enter manual mode once at startup. If this fails, we bail rather than
    // silently running a loop that writes duty values nobody applies -
    // exactly the bug we hit by hand earlier (pwm writes ignored at enable=0).
    pump.ensure_manual_mode()?;
    fan.ensure_manual_mode()?;
    log::info!("pump and fan channels in manual mode, entering control loop");

    let reload_flag = Arc::new(AtomicBool::new(false));
    let shared_state = ipc::start_server(reload_flag.clone())?;

    let mut last_pump_duty: Option<u8> = None;
    let mut last_fan_duty: Option<u8> = None;

    while running.load(Ordering::SeqCst) {
        if reload_flag.swap(false, Ordering::SeqCst) {
            match Config::load(&config_path) {
                Ok(new_cfg) => {
                    cfg = new_cfg;
                    log::info!("config reloaded from {:?}", config_path);
                    // Force a re-application of duty even if the temp
                    // hasn't changed, since the CURVE itself may have.
                    last_pump_duty = None;
                    last_fan_duty = None;
                }
                Err(e) => {
                    log::error!(
                        "config reload requested but failed to parse, KEEPING previous config: {}",
                        e
                    );
                }
            }
        }

        let liquid_temp = liquid_temp_sensor.read_celsius().ok();
        let cpu_temp = cpu_temp_sensor.as_ref().and_then(|s| s.read_celsius().ok());
        let gpu_temp = if gpu_available {
            hwmon::read_nvidia_gpu_temp().ok()
        } else {
            None
        };

        // SAFETY FAILSAFE: checked unconditionally, before mode branching,
        // regardless of Performance/Silent/Auto. If liquid temp is known
        // and at/above the configured ceiling, force both channels to
        // 100% and skip normal mode logic entirely for this cycle. This
        // is deliberately liquid-temp-only (not CPU/GPU) since it's the
        // most direct signal of AIO loop thermal load.
        let failsafe_triggered = match liquid_temp {
            Some(t) if t >= cfg.mode.failsafe_temp_c => {
                log::warn!(
                    "FAILSAFE: liquid temp {:.1}C >= threshold {:.1}C - forcing 100% regardless of mode ({:?})",
                    t, cfg.mode.failsafe_temp_c, cfg.mode.active
                );
                true
            }
            _ => false,
        };

        let (pump_duty, fan_duty) = control::decide_duty(
            cfg.mode.active,
            failsafe_triggered,
            cfg.mode.silent_duty_pct,
            &cfg.pump_curve,
            &cfg.fan_curve,
            liquid_temp,
            cpu_temp,
            gpu_temp,
        );

        if cfg.mode.active == config::Mode::Auto && !failsafe_triggered {
            if pump_duty.is_none() {
                log::error!(
                    "Auto mode: pump curve's temp source ({:?}) unavailable this cycle, keeping last duty",
                    cfg.pump_curve.temp_source
                );
            }
            if fan_duty.is_none() {
                log::error!(
                    "Auto mode: fan curve's temp source ({:?}) unavailable this cycle, keeping last duty",
                    cfg.fan_curve.temp_source
                );
            }
        }

        if let Some(pump_duty) = pump_duty {
            if last_pump_duty != Some(pump_duty) {
                if let Err(e) = pump.set_duty_pct(pump_duty) {
                    log::error!("failed to set pump duty: {}", e);
                } else {
                    log::debug!(
                        "pump duty -> {}% (mode={:?}, failsafe={})",
                        pump_duty, cfg.mode.active, failsafe_triggered
                    );
                    last_pump_duty = Some(pump_duty);
                }
            }
        }

        if let Some(fan_duty) = fan_duty {
            if last_fan_duty != Some(fan_duty) {
                if let Err(e) = fan.set_duty_pct(fan_duty) {
                    log::error!("failed to set fan duty: {}", e);
                } else {
                    log::debug!(
                        "fan duty -> {}% (mode={:?}, failsafe={})",
                        fan_duty, cfg.mode.active, failsafe_triggered
                    );
                    last_fan_duty = Some(fan_duty);
                }
            }
        }

        // Update shared state for IPC clients regardless of whether the
        // curve logic above succeeded - GUI should still see RPM/temps
        // even if, say, only the GPU read failed this cycle.
        {
            let mut s = shared_state.lock().unwrap();
            s.liquid_temp_c = liquid_temp;
            s.cpu_temp_c = cpu_temp;
            s.gpu_temp_c = gpu_temp;
            s.pump_rpm = pump.read_rpm().ok();
            s.fan_rpm = fan.read_rpm().ok();
            s.pump_duty_pct = last_pump_duty;
            s.fan_duty_pct = last_fan_duty;
            s.active_mode = Some(cfg.mode.active.label().to_string());
            s.failsafe_active = failsafe_triggered;
        }

        std::thread::sleep(Duration::from_millis(cfg.poll_interval_ms));
    }

    log::info!("nzxt-ctl-daemon exiting cleanly");
    Ok(())
}