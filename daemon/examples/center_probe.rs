//! Calibration pattern for centering and orientation: a dim grid of dots
//! every 40px across the 640x640 buffer, a bright white marker at the
//! geometric center (320,320), and four colored markers at the edge
//! midpoints - red=top, green=bottom, blue=left, yellow=right.
//!
//! On a correctly working pipeline the white dot sits dead-center in the
//! round glass and the colors are where their names say. Holds the
//! pattern via keep-alive until Ctrl-C.
//!
//!     cargo run --release --example center_probe

use image::{Rgba, RgbaImage};
use imageproc::drawing::draw_filled_circle_mut;
use nzxt_ctl_daemon::lcd::LcdController;

const RESOLUTION: (u32, u32) = (640, 640);
const GRID_STEP: i32 = 40;

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let (w, h) = RESOLUTION;

    let mut img = RgbaImage::from_pixel(w, h, Rgba([20, 20, 20, 255]));

    let mut x = 0;
    while x < w as i32 {
        let mut y = 0;
        while y < h as i32 {
            draw_filled_circle_mut(&mut img, (x, y), 3, Rgba([120, 120, 120, 255]));
            y += GRID_STEP;
        }
        x += GRID_STEP;
    }

    let (cx, cy) = (w as i32 / 2, h as i32 / 2);
    draw_filled_circle_mut(&mut img, (cx, cy), 14, Rgba([255, 255, 255, 255]));
    draw_filled_circle_mut(&mut img, (cx, 10), 16, Rgba([230, 30, 30, 255]));
    draw_filled_circle_mut(&mut img, (cx, h as i32 - 10), 16, Rgba([30, 200, 30, 255]));
    draw_filled_circle_mut(&mut img, (10, cy), 16, Rgba([40, 80, 220, 255]));
    draw_filled_circle_mut(&mut img, (w as i32 - 10, cy), 16, Rgba([220, 200, 30, 255]));

    img.save("/tmp/center_probe.png")?;
    println!("Saved /tmp/center_probe.png for reference.");
    println!("White = buffer center (320,320). Edge markers: red=top, green=bottom, blue=left, yellow=right.");

    println!("Uploading to LCD...");
    let mut ctl = LcdController::init(RESOLUTION)?;
    ctl.upload(&img.into_raw())?;
    println!("Uploaded. Holding via keep-alive until Ctrl-C...");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(10));
        ctl.commit()?;
    }
}
