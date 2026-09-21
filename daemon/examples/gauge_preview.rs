//! Dumps the rendered gauge to a local PNG so the render itself can be
//! inspected directly, separate from anything the USB pipeline or the
//! device does to it. (The per-device orientation rotation happens
//! inside `LcdController::upload`, not here.)
//!
//!     cargo run --release --example gauge_preview

use image::RgbaImage;
use nzxt_ctl_common::config::TempSource;
use nzxt_ctl_daemon::gauge;

const RESOLUTION: (u32, u32) = (640, 640);

fn main() -> anyhow::Result<()> {
    let (w, h) = RESOLUTION;
    let raw = gauge::render(RESOLUTION, TempSource::Cpu, Some(62.0));

    let img = RgbaImage::from_raw(w, h, raw).expect("valid buffer");
    img.save("/tmp/gauge_preview.png")?;
    println!("wrote /tmp/gauge_preview.png");
    Ok(())
}
