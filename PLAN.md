# PLAN

Roadmap and design notes for the larger pieces of work. Small items live in
TODO.md.

## Where the project stands

Working and verified on hardware:

- Pump/fan curves with per-curve temperature source (coolant/CPU/GPU)
- Performance / Silent / Auto modes — all three confirmed to apply the
  expected PWM duty on real hardware
- Coolant-temperature failsafe (unit tested; not yet triggered by real heat)
- Config hot-reload: GUI writes TOML, daemon reloads on request
- systemd unit installed and running; group-based permissions instead of
  world-writable socket
- Kirigami GUI following the system Plasma theme

Deliberately not built yet: LCD control, RGB control, graphical curve
editing.

## 1. Shared schema crate

**Problem.** `gui/src/config.rs` and `daemon/src/config.rs` are duplicates
maintained by hand, as are the daemon's `LiveState` and the GUI's copy. Two
past breakages came from adding a field to one side only.

**Approach.** Add a `common/` member crate holding `Config`, `ChannelCurve`,
`CurvePoint`, `Mode`, `TempSource`, `LiveState`, and the IPC request/response
enums. Both binaries depend on it. Keep `validate()` and `duty_for_temp()`
there too so the GUI can validate before writing rather than discovering
problems in the daemon's log.

Do this before adding more config fields — every new field currently doubles
the work and the risk.

## 2. Graphical curve editor

The biggest UX gap. Today's editor is numeric spinbox rows; CAM offers
drag-points on a plotted curve.

**Approach.** A QML `Canvas` (or `QQuickPaintedItem`) in `CurveEditor.qml`
drawing axes, the interpolated line, and draggable handles per point. The
data model is already the right shape — a `ListModel` of `{temp_c,
duty_pct}` that serialises to the JSON the bridge expects — so this is
presentation-layer work and needs no Rust changes. Keep the numeric rows
available as an alternate view; they're better for precise entry.

Constraints worth respecting: clamp drag to 0–150 °C / 0–100 %, keep points
sorted by temperature, and don't let the user delete the last point (the
bridge rejects empty curves).

## 3. LCD screen control

The Kraken's display is currently untouched. Two routes:

**Route A — shell out to `liquidctl` (low effort).** Works today:
`liquidctl set lcd screen static <file.png>`. The daemon would render a
gauge image with the `image`/`imageproc` crates and invoke liquidctl
periodically. Downsides: a Python process spawn per update, so this only
suits low refresh rates.

**Route B — speak the protocol directly (high effort).** Reverse-engineered
notes from USB captures, cross-checked against liquidctl's own source
(GPL-3.0 — this project is GPL-3.0, so derivation is compatible):

- Bulk OUT endpoint `0x02` on interface 0 (vendor-specific, no kernel
  driver). Interface 1 is HID and belongs to `nzxt_kraken3`.
- 12-byte magic preamble `12 FA 01 E8 AB CD EF 98 76 54 32 10`, constant
  across all captured frames.
- Then 8 bytes: `[mode, 0x00, 0x00, 0x00]` + length as LE32. The observed
  CAM capture used mode `0x09` with length 1,228,800 (640x640 RGB888), a
  mode liquidctl does not implement (it uses `0x02` RGBA or `0x06` RGB565).
- **Raw bulk writes alone are not enough.** The firmware needs HID
  choreography on interface 1 around the transfer: `0x36,0x03` handshake ->
  bucket query `0x30,0x04,i` -> setup `0x32,0x01,...` -> write-start
  `0x36,0x01,bucket` -> *bulk data* -> write-finish `0x36,0x02` -> and
  critically the switch/commit `0x38,0x01,mode,bucket`. Without the commit
  the watchdog reverts to the cached image after a timeout.
- Do NOT send a zero-length packet after the bulk data.

Route A first. Route B only if per-frame latency actually matters.

Note if attempting more USB captures: USBPcap caps bulk transfers at ~65 KB,
so a full 1.2 MB frame cannot be captured that way regardless of snaplen.

## 4. RGB controller

The `1e71:2012` "NZXT RGB Controller" is a separate USB device with a single
HID interface and no kernel driver. Entirely unexplored — not even a HID
descriptor dump yet. `liquidctl` supports some NZXT RGB devices and is the
obvious place to look first before any capture work.

## 5. Hardening

- Move the daemon off `User=root` via a udev rule granting the `nzxt-ctl`
  group write access to the specific `pwm*`/`pwm*_enable` attributes.
- Consider `ProtectSystem=strict` / `PrivateTmp` on the unit once the
  filesystem needs are pinned down.
