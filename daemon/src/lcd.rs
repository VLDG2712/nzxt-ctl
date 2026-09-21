//! Speaks the Kraken's LCD protocol directly (no liquidctl/Python
//! dependency at runtime). Reverse-engineered via a live USB capture and
//! cross-checked against liquidctl's own source/docs (GPL-3.0, same
//! license as this project) - see PLAN.md section 3 for the full writeup.
//!
//! Two backends, matching the device's own two interfaces:
//! - Interface 1 (HID, owned by the `nzxt_kraken3` kernel driver): small
//!   64-byte control commands over `/dev/hidrawN`, plain file I/O.
//! - Interface 0 (vendor-specific bulk, no kernel driver): the actual
//!   image bytes, over USB bulk endpoint 0x02 via `nusb`.
//!
//! KEY FINDING: a one-shot upload (even with a correct commit) is reverted
//! by the firmware after ~30s if nothing LCD-specific happens again - a
//! dead-man's-switch, not a protocol bug. Resending just the 4-byte commit
//! (no image re-upload) well under that window keeps it alive indefinitely
//! - confirmed for 90s straight. See `keepalive()`.

use anyhow::{Context, Result};
use nusb::transfer::{Bulk, Out};
use nusb::MaybeFuture;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

const VENDOR_ID: u16 = 0x1E71;
const PRODUCT_ID: u16 = 0x300C;

/// Bulk data magic preamble, constant across all captured frames.
const BULK_MAGIC: [u8; 12] = [
    0x12, 0xFA, 0x01, 0xE8, 0xAB, 0xCD, 0xEF, 0x98, 0x76, 0x54, 0x32, 0x10,
];

/// Plain RGBA (4 bytes/px, alpha byte unused) - confirmed rendering
/// correctly on real hardware (firmware 2.01), despite being the "old"
/// format path in liquidctl. See PLAN.md for why the fancier documented
/// modes (0x06/0x08/0x09) turned out to be unnecessary.
const DATA_TYPE_RGBA: u8 = 0x02;

/// Two buckets, double-buffered: each new frame is uploaded into the
/// bucket that is NOT currently on screen, then the display is switched
/// to it. Re-uploading into the live bucket is rejected by the device
/// (error byte 0x09), and the earlier workaround - wipe everything and
/// recreate one bucket per frame - had to switch the panel to its
/// built-in screen first, so every update flashed the firmware's own
/// liquid-temp display for a moment.
const BUCKETS: [u8; 2] = [0, 1];

/// Bucket size in KB for a 640x640 RGBA frame (1600KB), taken directly
/// from a real liquidctl run's create-bucket call captured on the wire.
/// The two buckets sit back to back from address 0, which is where
/// liquidctl itself places a bucket after a delete-all.
const BUCKET_SIZE_KB: u16 = 0x0641;

/// Comfortably under the observed ~30s watchdog window (90s confirmed
/// safe at a 10s interval; using a bit more margin here since this runs
/// unattended rather than under active observation).
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

pub struct LcdController {
    hid: File,
    resolution: (u32, u32),
    /// Byte length of the last uploaded frame's data, needed to reserve
    /// bucket memory on init. Fixed for the lifetime of the controller -
    /// this project always renders at `resolution`, so this never changes.
    frame_len: usize,
    /// Device-configured orientation as 90-degree steps (0-3), read at
    /// init via the `0x30 0x01` query. The firmware applies this to its
    /// own built-in screens but NOT to uploaded images, so we pre-rotate
    /// every frame to compensate - see `rotate_for_device`.
    orientation_quarter_turns: u8,
    /// Bucket currently on screen, if any frame has been committed yet.
    active_bucket: Option<u8>,
}

/// Rotates a square RGBA frame so it displays upright for the given
/// device orientation: `orientation` clockwise quarter turns, the same
/// transform liquidctl applies (`rotate(orientation * -90)` in PIL's
/// counter-clockwise convention). Verified on real hardware for
/// orientation 3 (270°): an unrotated buffer's top edge lands on the
/// physical left, one clockwise turn put it at the bottom, and three
/// clockwise turns (= one counter-clockwise) put it upright.
fn rotate_for_device(rgba: &[u8], size: u32, orientation_quarter_turns: u8) -> Vec<u8> {
    use image::imageops::{rotate180, rotate270, rotate90};
    let turns_cw = orientation_quarter_turns % 4;
    if turns_cw == 0 {
        return rgba.to_vec();
    }
    let img = image::RgbaImage::from_raw(size, size, rgba.to_vec())
        .expect("frame length already validated against resolution");
    match turns_cw {
        1 => rotate90(&img).into_raw(),
        2 => rotate180(&img).into_raw(),
        _ => rotate270(&img).into_raw(),
    }
}

/// Finds the hidraw device for the Kraken's HID interface (interface 1,
/// owned by `nzxt_kraken3`) by matching HID_ID in sysfs, the same
/// "never trust a fixed device number" approach as hwmon discovery.
fn discover_hidraw_path() -> Result<PathBuf> {
    let root = PathBuf::from("/sys/class/hidraw");
    let entries = fs::read_dir(&root).with_context(|| format!("reading {:?}", root))?;

    // HID_ID=0003:00001E71:0000300C - bus:vendor:product, each zero-padded
    // to 8 hex digits regardless of the real ID's width.
    let want = format!("0003:{:08X}:{:08X}", VENDOR_ID, PRODUCT_ID);

    for entry in entries {
        let entry = entry?;
        let uevent_path = entry.path().join("device/uevent");
        let Ok(uevent) = fs::read_to_string(&uevent_path) else {
            continue;
        };
        let matches = uevent
            .lines()
            .find_map(|l| l.strip_prefix("HID_ID="))
            .map(|id| id.eq_ignore_ascii_case(&want))
            .unwrap_or(false);
        if matches {
            let name = entry.file_name();
            let dev_path = PathBuf::from("/dev").join(&name);
            log::info!("discovered Kraken hidraw device at {:?}", dev_path);
            return Ok(dev_path);
        }
    }

    anyhow::bail!(
        "no hidraw device found for {:04x}:{:04x} under {:?} - is the device connected?",
        VENDOR_ID,
        PRODUCT_ID,
        root
    );
}

/// Sends a 64-byte HID report (command bytes, zero-padded) and reads
/// packets until one matches `expect_header` (the response's first two
/// bytes). Unnumbered reports - the command byte is byte 0, no separate
/// report-ID prefix (confirmed via USB capture).
///
/// The plain "write then read the next packet" approach fails under real
/// use: the `nzxt_kraken3` kernel driver's own hwmon polling (this
/// project's own daemon reads liquid temp/RPM once a second) shares the
/// same hidraw report stream, so an unrelated packet can arrive between
/// our write and our intended response - confirmed on real hardware
/// (a stray packet that was clearly mid-command garbage, not our `0x37`
/// reply, caused a spurious "device refused" failure). liquidctl has the
/// same problem and solves it the same way: filter by expected header,
/// discard anything else.
fn hid_command(hid: &mut File, cmd: &[u8], expect_header: [u8; 2]) -> Result<[u8; 64]> {
    anyhow::ensure!(cmd.len() <= 64, "HID command longer than one report");
    let mut report = [0u8; 64];
    report[..cmd.len()].copy_from_slice(cmd);
    hid.write_all(&report).context("writing HID report")?;

    // 32 was not enough in practice - the shared hidraw stream (this
    // project's own hwmon polling included) can bury our response deeper
    // than that under real, sustained load. Each attempt is a cheap
    // 64-byte read, so even the full budget resolves in well under a
    // second - correctness matters more than shaving attempts here.
    const MAX_ATTEMPTS: u32 = 200;
    for attempt in 1..=MAX_ATTEMPTS {
        let mut response = [0u8; 64];
        hid.read_exact(&mut response).context("reading HID response")?;
        if response[0] == expect_header[0] && response[1] == expect_header[1] {
            if attempt > 1 {
                log::debug!("matched HID response for {:02x?} on attempt {attempt}", expect_header);
            }
            return Ok(response);
        }
        log::debug!(
            "skipping unrelated HID packet while waiting for {:02x?}: {:02x?}...",
            expect_header,
            &response[..4]
        );
    }
    anyhow::bail!(
        "no HID response matching {:02x?} after {MAX_ATTEMPTS} packets - is another process also talking to the device?",
        expect_header
    )
}

/// True if byte 14 of the response is 0x01 - the device's own
/// success/fail convention for bucket and mode-switch operations.
fn response_ok(response: &[u8; 64]) -> bool {
    response[14] == 0x01
}

/// Logs the full raw response on failure - temporary while this protocol
/// is still being nailed down on real hardware (see PLAN.md section 3).
fn ensure_ok(response: &[u8; 64], what: &str) -> Result<()> {
    if response_ok(response) {
        Ok(())
    } else {
        log::error!("{what} - raw response: {:02x?}", response);
        anyhow::bail!("{what}")
    }
}

impl LcdController {
    /// Opens the device and reserves a dedicated bucket sized for
    /// `resolution` RGBA frames. Best-effort: callers should treat any
    /// error here as "LCD unavailable this run" and continue without it,
    /// same pattern as CPU/GPU sensor discovery in hwmon.rs.
    pub fn init(resolution: (u32, u32)) -> Result<Self> {
        let hidraw_path = discover_hidraw_path()?;
        let mut hid = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&hidraw_path)
            .with_context(|| format!("opening {:?}", hidraw_path))?;

        let (w, h) = resolution;
        let frame_len = (w as usize) * (h as usize) * 4;

        // Byte 26 of the 0x31 0x01 response is the orientation, as an
        // index into [0, 90, 180, 270] degrees - i.e. already the
        // quarter-turn count we need. The device applies this only to
        // its own built-in screens, not to uploaded images, so
        // `rotate_for_device` has to compensate on our side.
        let lcd_info = hid_command(&mut hid, &[0x30, 0x01], [0x31, 0x01])?;
        let orientation_quarter_turns = lcd_info[26];
        log::info!(
            "LCD orientation: {} degrees",
            (orientation_quarter_turns as u32 % 4) * 90
        );

        let mut ctl = Self {
            hid,
            resolution,
            frame_len,
            orientation_quarter_turns,
            active_bucket: None,
        };

        let data_size_kb = ctl.frame_len.div_ceil(1024) as u32;
        anyhow::ensure!(
            data_size_kb <= BUCKET_SIZE_KB as u32,
            "frame ({data_size_kb}KB) exceeds the bucket size ({BUCKET_SIZE_KB}KB) - \
             only the resolution this was captured against ({:?}) is supported for now",
            ctl.resolution
        );

        // Clean slate once at startup: switch to the built-in liquid-temp
        // display (safe, always-valid fallback - a bucket left on screen
        // by a previous run can't be deleted while live), then wipe all
        // 16 buckets. Bucket bookkeeping on the device has proven
        // unreliable to query after interrupted runs (spurious 0x04
        // rejections), so trusting nothing and rebuilding from scratch is
        // the robust choice.
        let _ = hid_command(&mut ctl.hid, &[0x38, 0x01, 0x02, 0x00], [0x39, 0x01]);
        for i in 0..16u8 {
            let _ = hid_command(&mut ctl.hid, &[0x32, 0x02, i], [0x33, 0x02]);
        }

        Ok(ctl)
    }

    /// (Re)creates `bucket` at its fixed memory slot. Deleting first is
    /// harmless if it doesn't exist, and required if it holds a previous
    /// frame - the device won't start a transfer into an occupied bucket.
    fn prepare_bucket(&mut self, bucket: u8) -> Result<()> {
        let _ = hid_command(&mut self.hid, &[0x32, 0x02, bucket], [0x33, 0x02]);

        let addr = (bucket as u16 * BUCKET_SIZE_KB).to_le_bytes();
        let size = BUCKET_SIZE_KB.to_le_bytes();
        let resp = hid_command(
            &mut self.hid,
            &[
                0x32,
                0x01,
                bucket,
                bucket + 1,
                addr[0],
                addr[1],
                size[0],
                size[1],
                0x01,
            ],
            // NOT 0x33 0x02 despite what the shipped protocol doc claims
            // for this command - confirmed against a real capture of a
            // known-good liquidctl run. Delete uses 0x33 0x02; create
            // uses 0x33 0x01. Docs can be wrong; captures don't lie.
            [0x33, 0x01],
        )?;
        ensure_ok(&resp, "device rejected bucket setup for LCD gauge")
    }

    /// Uploads a new frame and commits it as the active image. `rgba`
    /// must be exactly `width * height * 4` bytes (the alpha byte is
    /// present but ignored by the firmware in this mode).
    pub fn upload(&mut self, rgba: &[u8]) -> Result<()> {
        self.upload_paced(rgba, &|_| Duration::ZERO)
    }

    /// `upload` with a caller-chosen pause after each protocol step (keyed
    /// by step name, each logged at debug level) - a hardware-debugging
    /// aid for pinning down which step the firmware reacts to visibly.
    /// `upload` is this with zero pauses.
    pub fn upload_paced(&mut self, rgba: &[u8], pause: &dyn Fn(&str) -> Duration) -> Result<()> {
        let step = |name: &str| {
            log::debug!("upload step: {name}");
            std::thread::sleep(pause(name));
        };
        anyhow::ensure!(
            rgba.len() == self.frame_len,
            "frame size {} does not match expected {} for {:?}",
            rgba.len(),
            self.frame_len,
            self.resolution
        );

        let (w, h) = self.resolution;
        anyhow::ensure!(w == h, "LCD frames are assumed square, got {:?}", self.resolution);
        let rotated = rotate_for_device(rgba, w, self.orientation_quarter_turns);
        let rotated = rotated.as_slice();

        let target = match self.active_bucket {
            Some(b) if b == BUCKETS[0] => BUCKETS[1],
            _ => BUCKETS[0],
        };
        step("begin (previous frame should be on screen)");
        self.prepare_bucket(target)?;
        step("bucket deleted+created");

        hid_command(&mut self.hid, &[0x36, 0x03], [0x37, 0x03])?; // cancel any stale transfer
        step("cancel 36 03");
        let resp = hid_command(&mut self.hid, &[0x36, 0x01, target], [0x37, 0x01])?;
        ensure_ok(&resp, "device refused to start LCD data transfer")?;
        step("start transfer 36 01");

        let mut header = Vec::with_capacity(BULK_MAGIC.len() + 8);
        header.extend_from_slice(&BULK_MAGIC);
        header.push(DATA_TYPE_RGBA);
        header.extend_from_slice(&[0, 0, 0]);
        header.extend_from_slice(&(rotated.len() as u32).to_le_bytes());
        self.bulk_write(&header, rotated)?;
        step("bulk data sent");

        let resp = hid_command(&mut self.hid, &[0x36, 0x02], [0x37, 0x02])?;
        ensure_ok(&resp, "device refused to end LCD data transfer")?;
        step("end transfer 36 02");

        self.switch_to(target)?;
        self.active_bucket = Some(target);
        step("switched to new bucket");
        Ok(())
    }

    /// Hands the panel back to the firmware's built-in liquid-temperature
    /// screen. Best-effort: stopping the keep-alive would get there on its
    /// own within ~30s anyway, this just makes it immediate.
    pub fn release(&mut self) {
        let _ = hid_command(&mut self.hid, &[0x38, 0x01, 0x02, 0x00], [0x39, 0x01]);
    }

    /// Resends just the 4-byte commit for the already-uploaded bucket, no
    /// image data. This alone prevents the firmware's ~30s revert-to-
    /// default - confirmed empirically (90s straight, zero reverts).
    /// Call this on a timer well under that window; see
    /// [`KEEPALIVE_INTERVAL`].
    pub fn commit(&mut self) -> Result<()> {
        match self.active_bucket {
            Some(b) => self.switch_to(b),
            None => Ok(()),
        }
    }

    fn switch_to(&mut self, bucket: u8) -> Result<()> {
        let resp = hid_command(&mut self.hid, &[0x38, 0x01, 0x04, bucket], [0x39, 0x01])?;
        ensure_ok(&resp, "device rejected LCD mode-switch commit")
    }

    /// The header MUST go out as its own USB transfer (a lone 20-byte short
    /// packet), followed by the pixel data as a separate transfer with no
    /// trailing zero-length packet - exactly what liquidctl does and what
    /// the reference capture shows. An earlier version concatenated both
    /// into one stream, so the first 512-byte packet carried the header
    /// plus 492 bytes (123 pixels) of image data; the firmware treats that
    /// whole first packet as the header and the image came out shifted by
    /// 123px, which masqueraded as a "bezel isn't centered" problem.
    fn bulk_write(&self, header: &[u8], data: &[u8]) -> Result<()> {
        let device_info = nusb::list_devices()
            .wait()
            .context("listing USB devices")?
            .find(|d| d.vendor_id() == VENDOR_ID && d.product_id() == PRODUCT_ID)
            .context("Kraken USB device disappeared")?;
        let device = device_info.open().wait().context("opening Kraken USB device")?;
        let interface = device
            .claim_interface(0)
            .wait()
            .context("claiming Kraken bulk interface")?;
        let mut writer = interface
            .endpoint::<Bulk, Out>(0x02)
            .context("opening Kraken bulk OUT endpoint")?
            .writer(16 * 1024);

        writer.write_all(header).context("bulk-writing LCD header")?;
        writer.flush_end().context("flushing LCD header")?;

        // Plain flush, not flush_end: the frame length is an exact multiple
        // of the 512-byte packet size and flush_end would append a
        // zero-length packet, which the device does not want.
        writer.write_all(data).context("bulk-writing LCD frame")?;
        writer.flush().context("flushing LCD frame")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::rotate_for_device;

    // 2x2 frame, one distinct byte value per pixel (all four channels equal):
    //   A B
    //   C D
    fn frame(px: [u8; 4]) -> Vec<u8> {
        px.iter().flat_map(|&v| [v; 4]).collect()
    }

    #[test]
    fn orientation_zero_is_identity() {
        let f = frame([1, 2, 3, 4]);
        assert_eq!(rotate_for_device(&f, 2, 0), f);
    }

    #[test]
    fn orientation_90_rotates_one_quarter_turn_clockwise() {
        // Clockwise quarter turn: top-left moves to top-right.
        //   A B      C A
        //   C D  ->  D B
        assert_eq!(rotate_for_device(&frame([1, 2, 3, 4]), 2, 1), frame([3, 1, 4, 2]));
    }

    #[test]
    fn orientation_180_flips_both_axes() {
        assert_eq!(rotate_for_device(&frame([1, 2, 3, 4]), 2, 2), frame([4, 3, 2, 1]));
    }

    #[test]
    fn orientation_270_rotates_counter_clockwise() {
        //   A B      B D
        //   C D  ->  A C
        assert_eq!(rotate_for_device(&frame([1, 2, 3, 4]), 2, 3), frame([2, 4, 1, 3]));
    }
}
