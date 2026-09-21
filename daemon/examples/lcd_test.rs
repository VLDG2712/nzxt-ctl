//! Manual hardware verification for the LCD module - not part of the
//! automated test suite (this needs a real Kraken attached). Run with:
//!
//!     cargo run --release --example lcd_test
//!
//! Renders a sequence of test temperatures to the gauge, then holds the
//! last one alive via the keep-alive commit for a while so you can watch
//! it NOT revert. Safe to run alongside the live systemd daemon - this
//! only touches the LCD subsystem (hidraw interface 1 + bulk interface
//! 0), never pwm/fan/hwmon.

use nzxt_ctl_common::config::TempSource;
use nzxt_ctl_daemon::{gauge, lcd::LcdController};
use std::time::Duration;

const RESOLUTION: (u32, u32) = (640, 640);

fn main() -> anyhow::Result<()> {
    env_logger::init();

    println!("Initializing LCD controller...");
    let mut ctl = LcdController::init(RESOLUTION)?;
    println!("Init OK.");

    for temp in [35.0, 50.0, 65.0, 85.0] {
        println!("Rendering {temp:.1}C...");
        let frame = gauge::render(RESOLUTION, TempSource::Cpu, Some(temp));
        ctl.upload(&frame)?;
        println!("Uploaded. Watch the LCD now.");
        std::thread::sleep(Duration::from_secs(4));
    }

    println!("Holding at 85C via keep-alive commits for 60s (should NOT revert)...");
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_secs(10));
        ctl.commit()?;
        println!("[{:5.1}s] re-committed", start.elapsed().as_secs_f32());
    }

    println!("Done.");
    Ok(())
}
