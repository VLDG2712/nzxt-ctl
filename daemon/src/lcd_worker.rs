//! Owns the LCD on a dedicated thread so the cooling control loop never
//! blocks on USB. The loop just posts the latest reading and the desired
//! on/off state; this thread renders, uploads, keeps the panel alive, and
//! retries device init if the Kraken isn't ready yet (same boot-time USB
//! race that hwmon discovery handles). A wedged panel can at worst stall
//! this thread - fan and pump control carry on regardless.

use nzxt_ctl_common::config::TempSource;
use nzxt_ctl_daemon::gauge;
use nzxt_ctl_daemon::lcd::{LcdController, KEEPALIVE_INTERVAL};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

enum Command {
    SetEnabled(bool),
    Show {
        source: TempSource,
        temp_c: Option<f32>,
    },
    Shutdown,
}

pub struct LcdHandle {
    tx: Sender<Command>,
    done: Receiver<()>,
}

impl LcdHandle {
    pub fn spawn(resolution: (u32, u32)) -> Self {
        let (tx, rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        thread::Builder::new()
            .name("lcd".into())
            .spawn(move || {
                Worker::new(resolution).run(rx);
                let _ = done_tx.send(());
            })
            .expect("spawning LCD thread");
        Self { tx, done }
    }

    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.tx.send(Command::SetEnabled(enabled));
    }

    pub fn show(&self, source: TempSource, temp_c: Option<f32>) {
        let _ = self.tx.send(Command::Show { source, temp_c });
    }

    /// Hands the panel back to the firmware and waits briefly for the
    /// thread to confirm. Bounded so a hung device can't hold up daemon
    /// exit - the keep-alive stopping reverts the panel within ~30s anyway.
    pub fn shutdown(self) {
        let _ = self.tx.send(Command::Shutdown);
        let _ = self.done.recv_timeout(Duration::from_secs(5));
    }
}

/// Exponential moving average with whole-degree output. CPU temps jitter
/// across degree boundaries nearly every poll at idle, which unsmoothed
/// meant a 1.6MB USB upload each time just to flicker 41 <-> 42.
struct Smoother {
    alpha: f32,
    value: Option<f32>,
}

impl Smoother {
    fn new(alpha: f32) -> Self {
        Self { alpha, value: None }
    }

    fn reset(&mut self) {
        self.value = None;
    }

    /// Feeds one raw reading and returns the smoothed value; a missing
    /// reading clears the history so a sensor coming back starts fresh.
    fn update(&mut self, raw: Option<f32>) -> Option<f32> {
        self.value = match (raw, self.value) {
            (Some(t), Some(prev)) => Some(prev + (t - prev) * self.alpha),
            (Some(t), None) => Some(t),
            (None, _) => None,
        };
        self.value
    }
}

struct Worker {
    resolution: (u32, u32),
    want_enabled: bool,
    ctl: Option<LcdController>,
    init_failed_logged: bool,
    smoother: Smoother,
    last_source: Option<TempSource>,
    last_rendered: Option<(TempSource, Option<i32>)>,
    last_touch: Instant,
}

impl Worker {
    fn new(resolution: (u32, u32)) -> Self {
        Self {
            resolution,
            want_enabled: false,
            ctl: None,
            init_failed_logged: false,
            smoother: Smoother::new(0.3),
            last_source: None,
            last_rendered: None,
            last_touch: Instant::now(),
        }
    }

    fn run(mut self, rx: Receiver<Command>) {
        let mut pending: Option<Command> = None;
        loop {
            let cmd = match pending.take() {
                Some(c) => c,
                None => {
                    let wait = KEEPALIVE_INTERVAL.saturating_sub(self.last_touch.elapsed());
                    match rx.recv_timeout(wait) {
                        Ok(c) => c,
                        Err(RecvTimeoutError::Timeout) => {
                            self.tick();
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            };

            // A slow upload can let several readings queue up; only the
            // newest is worth rendering. Anything else found while
            // draining is handled next iteration, in order.
            let cmd = match cmd {
                Command::Show { .. } => {
                    let mut latest = cmd;
                    loop {
                        match rx.try_recv() {
                            Ok(c @ Command::Show { .. }) => latest = c,
                            Ok(other) => {
                                pending = Some(other);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    latest
                }
                other => other,
            };

            match cmd {
                Command::SetEnabled(enabled) => self.set_enabled(enabled),
                Command::Show { source, temp_c } => self.show(source, temp_c),
                Command::Shutdown => break,
            }
        }
        self.set_enabled(false);
    }

    /// Keep-alive deadline reached: re-commit the live frame, or retry
    /// device init if the panel is wanted but wasn't available earlier.
    fn tick(&mut self) {
        self.last_touch = Instant::now();
        match self.ctl.as_mut() {
            Some(ctl) => {
                if let Err(e) = ctl.commit() {
                    log::warn!("LCD keep-alive failed, will re-init: {}", e);
                    self.ctl = None;
                }
            }
            None if self.want_enabled => self.try_init(),
            None => {}
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.want_enabled = enabled;
        match (enabled, self.ctl.is_some()) {
            (true, false) => {
                self.init_failed_logged = false;
                self.try_init();
            }
            (false, true) => {
                if let Some(mut ctl) = self.ctl.take() {
                    ctl.release();
                }
                log::info!("LCD gauge disabled, panel returned to built-in display");
            }
            _ => {}
        }
    }

    fn try_init(&mut self) {
        match LcdController::init(self.resolution) {
            Ok(ctl) => {
                log::info!("LCD gauge enabled");
                self.ctl = Some(ctl);
                self.init_failed_logged = false;
                self.smoother.reset();
                self.last_rendered = None;
            }
            Err(e) if !self.init_failed_logged => {
                log::warn!("LCD unavailable, will keep retrying: {}", e);
                self.init_failed_logged = true;
            }
            Err(e) => log::debug!("LCD init retry failed: {}", e),
        }
    }

    fn show(&mut self, source: TempSource, temp_c: Option<f32>) {
        if self.ctl.is_none() {
            return;
        }
        if self.last_source != Some(source) {
            self.smoother.reset();
            self.last_source = Some(source);
        }
        let smoothed = self.smoother.update(temp_c);
        let key = (source, smoothed.map(|t| t.round() as i32));
        if self.last_rendered == Some(key) {
            return;
        }
        let frame = gauge::render(self.resolution, source, smoothed);
        let Some(ctl) = self.ctl.as_mut() else { return };
        match ctl.upload(&frame) {
            Ok(()) => {
                self.last_rendered = Some(key);
                self.last_touch = Instant::now();
            }
            Err(e) => {
                log::warn!("LCD gauge update failed, will re-init: {}", e);
                self.ctl = None;
                self.last_rendered = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Smoother;

    #[test]
    fn smoother_converges_toward_input() {
        let mut s = Smoother::new(0.5);
        assert_eq!(s.update(Some(40.0)), Some(40.0));
        assert_eq!(s.update(Some(50.0)), Some(45.0));
        assert_eq!(s.update(Some(50.0)), Some(47.5));
    }

    #[test]
    fn smoother_resets_on_missing_reading() {
        let mut s = Smoother::new(0.5);
        s.update(Some(40.0));
        assert_eq!(s.update(None), None);
        assert_eq!(s.update(Some(60.0)), Some(60.0));
    }
}
