use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// hwmonN numbers are NOT stable across reboots/kernel updates - confirmed
/// empirically (moved from hwmon5 to hwmon2 between runs on the same
/// machine). Always resolve by scanning for the device's declared `name`
/// file rather than trusting a hardcoded path.
pub fn discover_hwmon_path(expected_name: &str) -> Result<PathBuf> {
    let hwmon_root = PathBuf::from("/sys/class/hwmon");
    let entries = fs::read_dir(&hwmon_root)
        .with_context(|| format!("reading {:?}", hwmon_root))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name_path = path.join("name");
        if let Ok(name) = fs::read_to_string(&name_path) {
            if name.trim() == expected_name {
                log::info!("discovered hwmon device '{}' at {:?}", expected_name, path);
                return Ok(path);
            }
        }
    }

    anyhow::bail!(
        "no hwmon device found with name '{}' under {:?} - is the nzxt_kraken3 kernel driver loaded? check `lsmod | grep nzxt`",
        expected_name,
        hwmon_root
    );
}

/// Retries discover_hwmon_path for up to `timeout` before giving up. At
/// boot, systemd can start this daemon before USB enumeration and the
/// nzxt_kraken3 driver have finished binding and created the hwmon entry -
/// a single fail-fast attempt races this and fails right after boot even
/// though the device is fine a moment later. Confirmed as a real boot-time
/// failure mode (not hypothetical) via the journal.
///
/// NOTE: this only covers a slow-to-appear device. If the device is
/// genuinely absent (unplugged, disconnected mid-session - confirmed
/// separately via `dmesg` showing a USB disconnect with no reconnect),
/// this will still exhaust the full timeout and fail; no retry window
/// fixes a device that just isn't there.
pub fn discover_hwmon_path_with_retry(expected_name: &str, timeout: Duration) -> Result<PathBuf> {
    let start = Instant::now();
    let mut attempt: u32 = 1;

    loop {
        match discover_hwmon_path(expected_name) {
            Ok(path) => {
                if attempt > 1 {
                    log::info!(
                        "hwmon device found on attempt {} after {:.1}s",
                        attempt,
                        start.elapsed().as_secs_f32()
                    );
                }
                return Ok(path);
            }
            Err(e) => {
                let elapsed = start.elapsed();
                if elapsed >= timeout {
                    return Err(e.context(format!(
                        "hwmon device '{}' not found after {} attempts over {:.1}s (timeout {:.1}s) - giving up",
                        expected_name,
                        attempt,
                        elapsed.as_secs_f32(),
                        timeout.as_secs_f32()
                    )));
                }
                log::warn!(
                    "hwmon device '{}' not found yet (attempt {}, {:.1}s elapsed) - retrying: {}",
                    expected_name,
                    attempt,
                    elapsed.as_secs_f32(),
                    e
                );
                std::thread::sleep(Duration::from_millis(500));
                attempt += 1;
            }
        }
    }
}

/// Wraps read/write access to one hwmon PWM channel (pump=1, fan=2 on this
/// device, confirmed empirically: pwm1/fan1 = pump, pwm2/fan2 = fan).
pub struct HwmonChannel {
    base: PathBuf,
    index: u8,
}

impl HwmonChannel {
    pub fn new(base_path: &str, index: u8) -> Self {
        Self {
            base: PathBuf::from(base_path),
            index,
        }
    }

    fn pwm_path(&self) -> PathBuf {
        self.base.join(format!("pwm{}", self.index))
    }

    fn pwm_enable_path(&self) -> PathBuf {
        self.base.join(format!("pwm{}_enable", self.index))
    }

    fn fan_input_path(&self) -> PathBuf {
        self.base.join(format!("fan{}_input", self.index))
    }

    /// Ensure manual control mode is active. Must be called before writing
    /// pwm values directly - confirmed empirically that pwm writes are
    /// silently ignored while pwm*_enable == 0.
    pub fn ensure_manual_mode(&self) -> Result<()> {
        let current = fs::read_to_string(self.pwm_enable_path())
            .context("reading pwm_enable")?
            .trim()
            .to_string();
        if current != "1" {
            fs::write(self.pwm_enable_path(), "1").context("writing pwm_enable=1")?;
            log::info!("channel {}: set pwm_enable=1 (manual mode)", self.index);
        }
        Ok(())
    }

    /// duty_pct: 0-100. Converted to the 0-255 range the kernel expects.
    pub fn set_duty_pct(&self, duty_pct: u8) -> Result<()> {
        let duty_pct = duty_pct.min(100);
        let raw = ((duty_pct as u32 * 255) / 100) as u8;
        fs::write(self.pwm_path(), raw.to_string())
            .with_context(|| format!("writing pwm{} = {}", self.index, raw))?;
        Ok(())
    }

    pub fn read_rpm(&self) -> Result<u32> {
        let text = fs::read_to_string(self.fan_input_path()).context("reading fan_input")?;
        text.trim()
            .parse::<u32>()
            .context("parsing fan_input as u32")
    }
}

/// Reads coolant temperature. temp1_input on this device, in millidegrees C
/// (confirmed empirically: e.g. "29100" = 29.1C).
pub struct TempSensor {
    path: PathBuf,
}

impl TempSensor {
    pub fn new(base_path: &str, index: u8) -> Self {
        Self {
            path: PathBuf::from(base_path).join(format!("temp{}_input", index)),
        }
    }

    pub fn read_celsius(&self) -> Result<f32> {
        let text = fs::read_to_string(&self.path).context("reading temp_input")?;
        let millideg: i32 = text.trim().parse().context("parsing temp_input")?;
        Ok(millideg as f32 / 1000.0)
    }
}

/// Discovers a CPU temp sensor by trying common hwmon driver names in
/// order. coretemp = Intel, k10temp/zenpower = AMD. Picks the FIRST
/// temp*_input file under the matched device - on most consumer CPUs
/// this is "Package id 0" / Tdie, a reasonable single "CPU temp" summary,
/// but multi-socket or unusual layouts may need per-core selection later.
pub fn discover_cpu_temp_sensor() -> Result<TempSensor> {
    let candidates = ["coretemp", "k10temp", "zenpower"];
    let hwmon_root = PathBuf::from("/sys/class/hwmon");
    let entries = fs::read_dir(&hwmon_root).context("reading /sys/class/hwmon")?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name_path = path.join("name");
        if let Ok(name) = fs::read_to_string(&name_path) {
            let name = name.trim();
            if candidates.contains(&name) {
                log::info!("discovered CPU temp sensor '{}' at {:?}", name, path);
                return Ok(TempSensor::new(&path.to_string_lossy(), 1));
            }
        }
    }

    anyhow::bail!(
        "no CPU temp sensor found (tried: {:?}) - your CPU's hwmon driver may not be loaded, or use a different name than expected",
        candidates
    );
}

/// GPU temp via `nvidia-smi`. Shells out rather than using NVML bindings
/// directly - simpler, no extra native dependency, and nvidia-smi's
/// startup overhead (~50-150ms) is fine at a 1-second poll interval, but
/// would NOT be fine if this were called much more frequently.
pub fn read_nvidia_gpu_temp() -> Result<f32> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"])
        .output()
        .context("running nvidia-smi - is it installed and in PATH?")?;

    if !output.status.success() {
        anyhow::bail!(
            "nvidia-smi exited with error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let text = String::from_utf8_lossy(&output.stdout);
    // If multiple GPUs are present, nvidia-smi prints one line per GPU -
    // we only take the first line/GPU for now.
    let first_line = text.lines().next().unwrap_or("").trim();
    first_line
        .parse::<f32>()
        .with_context(|| format!("parsing nvidia-smi output: {:?}", text))
}