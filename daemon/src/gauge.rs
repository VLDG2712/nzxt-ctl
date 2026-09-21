//! Renders the Kraken LCD gauge: a temperature as a big number on black,
//! with a dotted progress ring and small labels. The ring and accent
//! take a cool-blue -> amber -> hot-red color from where the value sits
//! in the source's range, so heat reads at a glance without a colored
//! background. No external assets beyond the embedded font.

use ab_glyph::{FontRef, PxScale};
use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_filled_circle_mut, draw_text_mut, text_size};
use nzxt_ctl_common::config::TempSource;

const FONT_BYTES: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

const BLACK: Rgba<u8> = Rgba([0, 0, 0, 255]);
const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);
const DIM: Rgba<u8> = Rgba([70, 70, 70, 255]);
const UNKNOWN: Rgba<u8> = Rgba([120, 120, 120, 255]);

/// Gauge range (min, max) per source. Bounds the ring and the color
/// ramp: the min is "idle", the max is "as hot as it should ever get".
fn gauge_range(source: TempSource) -> (f32, f32) {
    match source {
        TempSource::Liquid => (20.0, 60.0),
        TempSource::Cpu => (30.0, 95.0),
        TempSource::Gpu => (30.0, 90.0),
    }
}

fn source_label(source: TempSource) -> &'static str {
    match source {
        TempSource::Liquid => "LIQUID",
        TempSource::Cpu => "CPU",
        TempSource::Gpu => "GPU",
    }
}

/// Renders a `resolution`-sized RGBA frame showing `temp_c` for
/// `source`. Returns raw bytes ready for `LcdController::upload`
/// (4 bytes/px, row-major, alpha unused by the firmware).
pub fn render(resolution: (u32, u32), source: TempSource, temp_c: Option<f32>) -> Vec<u8> {
    let (w, h) = resolution;
    let mut img = RgbaImage::from_pixel(w, h, BLACK);
    let font = FontRef::try_from_slice(FONT_BYTES).expect("embedded font must parse");

    let (min, max) = gauge_range(source);
    let frac = temp_c.map(|t| ((t - min) / (max - min)).clamp(0.0, 1.0));
    let accent = frac.map(heat_color).unwrap_or(UNKNOWN);

    draw_ring(&mut img, frac, accent);

    let label = match temp_c {
        Some(t) => format!("{t:.0}"),
        None => "--".to_string(),
    };
    let value_scale = PxScale::from(h as f32 * 0.36);
    let small_scale = PxScale::from(h as f32 * 0.075);

    let (vw, vh) = text_size(value_scale, &font, &label);
    let vx = (w as i32 - vw as i32) / 2;
    let vy = (h as i32 - vh as i32) / 2 - (h as i32 / 40);
    draw_text_mut(&mut img, WHITE, vx, vy, value_scale, &font, &label);

    let source_text = source_label(source);
    let (sw, sh) = text_size(small_scale, &font, source_text);
    let sx = (w as i32 - sw as i32) / 2;
    let sy = vy - sh as i32 - (h as i32 / 40);
    draw_text_mut(&mut img, accent, sx, sy, small_scale, &font, source_text);

    let unit = "\u{B0}C";
    let (uw, _) = text_size(small_scale, &font, unit);
    let ux = (w as i32 - uw as i32) / 2;
    let uy = vy + vh as i32 + (h as i32 / 60);
    draw_text_mut(&mut img, UNKNOWN, ux, uy, small_scale, &font, unit);

    img.into_raw()
}

/// Blue (cool) -> amber -> red (hot) over `frac` in 0..=1. Linear
/// interpolation through three stops rather than a raw two-color blend,
/// so the midpoint reads as an unambiguous "getting warm" amber instead
/// of a muddy purple.
fn heat_color(frac: f32) -> Rgba<u8> {
    const STOPS: [(f32, [u8; 3]); 3] = [
        (0.0, [60, 140, 255]),  // cool blue
        (0.5, [255, 170, 40]),  // amber
        (1.0, [255, 60, 50]),   // hot red
    ];
    let (c0, c1) = if frac <= 0.5 {
        (STOPS[0], STOPS[1])
    } else {
        (STOPS[1], STOPS[2])
    };
    let local = (frac - c0.0) / (c1.0 - c0.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * local).round() as u8;
    Rgba([
        mix(c0.1[0], c1.1[0]),
        mix(c0.1[1], c1.1[1]),
        mix(c0.1[2], c1.1[2]),
        255,
    ])
}

/// A full 360-degree ring of dots, lit in `accent` proportionally to
/// `frac` starting from the top and going clockwise, the rest dim. Always
/// a complete circle so every frame reads as an intentional gauge, never
/// as a clipped or broken shape (imageproc has no arc primitive, and a
/// partial arc without a fixed anchor looked like a rendering bug on the
/// real panel). Colors are opaque and pre-chosen: imageproc overwrites
/// rather than alpha-blends, and the firmware ignores alpha anyway.
fn draw_ring(img: &mut RgbaImage, frac: Option<f32>, accent: Rgba<u8>) {
    let (w, h) = img.dimensions();
    let cx = w as i32 / 2;
    let cy = h as i32 / 2;
    // The round glass shows the full inscribed circle (radius 0.5); this
    // leaves a small margin for the dots plus bezel tolerance.
    let radius = (w.min(h) as f32 * 0.44) as i32;
    let dot_radius = (w as f32 * 0.012).max(2.0) as i32;

    const DOTS: usize = 48;
    let lit_dots = (frac.unwrap_or(0.0) * DOTS as f32).round() as usize;

    for i in 0..DOTS {
        let angle = (-90.0 + 360.0 * (i as f32 / DOTS as f32)).to_radians();
        let x = cx + (radius as f32 * angle.cos()) as i32;
        let y = cy + (radius as f32 * angle.sin()) as i32;
        let color = if i < lit_dots { accent } else { DIM };
        draw_filled_circle_mut(img, (x, y), dot_radius, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heat_color_hits_the_stops() {
        assert_eq!(heat_color(0.0), Rgba([60, 140, 255, 255]));
        assert_eq!(heat_color(0.5), Rgba([255, 170, 40, 255]));
        assert_eq!(heat_color(1.0), Rgba([255, 60, 50, 255]));
    }

    #[test]
    fn render_produces_a_full_black_backed_frame() {
        let frame = render((64, 64), TempSource::Cpu, Some(50.0));
        assert_eq!(frame.len(), 64 * 64 * 4);
        // Corner pixel is outside the ring and text: must be background.
        assert_eq!(&frame[..4], &[0, 0, 0, 255]);
    }
}
