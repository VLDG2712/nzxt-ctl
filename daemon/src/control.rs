use crate::config::{ChannelCurve, Mode, TempSource};

/// Pure decision function extracted from the control loop so it can be unit
/// tested without real hwmon paths. Given the active mode, whether the
/// safety failsafe fired this cycle, and the latest readings, returns the
/// (pump_duty, fan_duty) to apply - `None` for a channel means "keep last
/// duty" (selected temp source unavailable this cycle).
pub fn decide_duty(
    mode: Mode,
    failsafe_triggered: bool,
    silent_duty_pct: u8,
    pump_curve: &ChannelCurve,
    fan_curve: &ChannelCurve,
    liquid: Option<f32>,
    cpu: Option<f32>,
    gpu: Option<f32>,
) -> (Option<u8>, Option<u8>) {
    if failsafe_triggered {
        return (Some(100), Some(100));
    }

    let pick_temp = |source: TempSource| -> Option<f32> {
        match source {
            TempSource::Liquid => liquid,
            TempSource::Cpu => cpu,
            TempSource::Gpu => gpu,
        }
    };

    match mode {
        Mode::Performance => (Some(100), Some(100)),
        Mode::Silent => (Some(silent_duty_pct), Some(silent_duty_pct)),
        Mode::Auto => {
            let p = pick_temp(pump_curve.temp_source).map(|t| pump_curve.duty_for_temp(t));
            let f = pick_temp(fan_curve.temp_source).map(|t| fan_curve.duty_for_temp(t));
            (p, f)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CurvePoint;

    fn curve(source: TempSource, points: &[(f32, u8)]) -> ChannelCurve {
        ChannelCurve {
            temp_source: source,
            points: points
                .iter()
                .map(|&(temp_c, duty_pct)| CurvePoint { temp_c, duty_pct })
                .collect(),
        }
    }

    fn default_curves() -> (ChannelCurve, ChannelCurve) {
        (
            curve(TempSource::Liquid, &[(20.0, 50), (55.0, 100)]),
            curve(TempSource::Liquid, &[(20.0, 30), (55.0, 100)]),
        )
    }

    #[test]
    fn performance_mode_always_full_regardless_of_temps() {
        let (pump, fan) = default_curves();
        let result = decide_duty(
            Mode::Performance,
            false,
            40,
            &pump,
            &fan,
            Some(20.0),
            None,
            None,
        );
        assert_eq!(result, (Some(100), Some(100)));
    }

    #[test]
    fn silent_mode_uses_fixed_duty_for_both_channels() {
        let (pump, fan) = default_curves();
        let result = decide_duty(
            Mode::Silent,
            false,
            35,
            &pump,
            &fan,
            Some(80.0), // even a hot reading is ignored in Silent
            None,
            None,
        );
        assert_eq!(result, (Some(35), Some(35)));
    }

    #[test]
    fn auto_mode_interpolates_each_curve_from_its_own_temp_source() {
        let pump = curve(TempSource::Liquid, &[(20.0, 50), (60.0, 100)]);
        let fan = curve(TempSource::Cpu, &[(20.0, 30), (60.0, 100)]);
        let result = decide_duty(
            Mode::Auto,
            false,
            40,
            &pump,
            &fan,
            Some(20.0), // drives pump only
            Some(60.0), // drives fan only
            None,
        );
        assert_eq!(result, (Some(50), Some(100)));
    }

    #[test]
    fn auto_mode_keeps_last_duty_when_selected_source_unavailable() {
        let pump = curve(TempSource::Gpu, &[(20.0, 50), (60.0, 100)]);
        let fan = curve(TempSource::Liquid, &[(20.0, 30), (60.0, 100)]);
        // GPU reading missing this cycle (e.g. nvidia-smi failed) - pump
        // side must be None (caller keeps last duty), NOT silently
        // substitute liquid or cpu.
        let result = decide_duty(Mode::Auto, false, 40, &pump, &fan, Some(30.0), None, None);
        assert_eq!(result.0, None);
        assert!(result.1.is_some());
    }

    #[test]
    fn failsafe_overrides_every_mode_including_silent() {
        let (pump, fan) = default_curves();
        for mode in [Mode::Performance, Mode::Silent, Mode::Auto] {
            let result = decide_duty(mode, true, 10, &pump, &fan, Some(65.0), None, None);
            assert_eq!(result, (Some(100), Some(100)), "mode={:?}", mode);
        }
    }
}
