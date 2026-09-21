//! Which upload timing makes the firmware flash its built-in screen?
//! A paced run (6s after EVERY step) showed no flash at all, while the
//! normal zero-pause upload flashes - so it's a timing interaction
//! between consecutive steps. This runs a series of uploads, each with a
//! pause inserted after ONE specific step only, announcing each; note
//! which ones flash.
//!
//!     cargo run --release --example flash_probe

use nzxt_ctl_common::config::TempSource;
use nzxt_ctl_daemon::{gauge, lcd::LcdController};
use std::time::Duration;

const RESOLUTION: (u32, u32) = (640, 640);

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let mut ctl = LcdController::init(RESOLUTION)?;
    let mut temp = 30.0;
    let frame = |t: &mut f32| {
        *t += 5.0;
        gauge::render(RESOLUTION, TempSource::Cpu, Some(*t))
    };

    ctl.upload(&frame(&mut temp))?;
    println!("Baseline frame up. Each test in 6s...");
    std::thread::sleep(Duration::from_secs(6));

    let tests: [(&str, &str, u64); 4] = [
        ("A", "end transfer 36 02", 500),
        ("B", "bulk data sent", 500),
        ("C", "start transfer 36 01", 500),
        ("D", "bucket deleted+created", 500),
    ];
    for (label, after_step, ms) in tests {
        println!("Test {label}: {ms}ms pause after '{after_step}' only -> uploading now");
        ctl.upload_paced(&frame(&mut temp), &|name| {
            if name == after_step {
                Duration::from_millis(ms)
            } else {
                Duration::ZERO
            }
        })?;
        std::thread::sleep(Duration::from_secs(6));
    }

    println!("Test E: plain zero-pause upload (control, expected to flash)");
    ctl.upload(&frame(&mut temp))?;
    std::thread::sleep(Duration::from_secs(6));

    println!("Done.");
    Ok(())
}
