# nzxt-ctl
I was missing this app on linux so me and claude did this!
A native Linux fan/pump controller for NZXT Kraken AIO coolers — a daemon
plus a Qt/Kirigami desktop app, as a replacement for NZXT CAM.

Built and tested against a **Kraken 2023 Elite** (`1e71:300c`, firmware
2.1.0) on Arch Linux with KDE Plasma.

## Features

- Temperature-driven pump and fan curves with linear interpolation, edited
  as a draggable curve or as precise numeric rows
- Three modes: **Performance** (100%), **Silent** (fixed low duty, duty %
  editable in the GUI), **Auto** (curve-driven)
- Each curve independently picks its temperature source: coolant, CPU, or GPU
- Safety failsafe: if coolant temperature reaches a configured ceiling
  (default 60 °C, editable in the GUI down to a 40 °C floor), both channels
  are forced to 100% regardless of mode
- Live dashboard: coolant/CPU/GPU temperature, pump/fan RPM and duty
- System tray icon with a mode-changer menu; optional close/start-to-tray
  and launch-on-login
- Runs at boot via systemd, with config hot-reload and revert-to-saved from
  the GUI

## How it works

Control goes through the in-tree `nzxt_kraken3` kernel driver's hwmon
interface rather than raw USB, so no custom protocol code is needed:

- `pwm1`/`fan1` — pump, `pwm2`/`fan2` — fan, `temp1_input` — coolant
- `pwmN_enable` must be `1` before `pwmN` writes take effect (writes are
  silently ignored otherwise)
- The `hwmonN` number changes across reboots, so the device is always
  resolved by scanning `/sys/class/hwmon/*/name` for `kraken2023elite`

CPU temperature comes from `coretemp`/`k10temp`/`zenpower`; GPU temperature
from `nvidia-smi` (optional — if absent, only that reading is unavailable).

## Architecture

```
common/   config schema, validation and IPC wire types, shared by both
          binaries below
daemon/   background service (root) - reads sensors, applies curves,
          serves live state over a Unix socket
gui/      Qt6/QML + Kirigami desktop app (regular user) - dashboard and
          curve editor, talks to the daemon over that socket
config/   sample configuration
systemd/  service unit + sysusers group definition
```

The daemon needs root to write `pwm*` under sysfs; the GUI runs as your
normal user. They share access through a dedicated `nzxt-ctl` group rather
than a world-writable socket. The IPC protocol is JSON lines over
`/run/nzxt-ctl/daemon.sock`.

## Installation

On Arch, via the AUR: `yay -S nzxt-ctl-git` (or any AUR helper). For a
manual build/install, see [INSTALL.md](INSTALL.md).

## Configuration

`/etc/nzxt-ctl/config.toml` — see [config/default.toml](config/default.toml)
for a documented example. The GUI writes this file and asks the daemon to
reload it, so hand-editing and using the GUI are interchangeable.

Note that in TOML, bare keys after a `[table]` header belong to that table,
so `poll_interval_ms` must appear *before* the first `[section]`.

## Status and limitations

Working and verified on hardware: curve control, all three modes, config
hot-reload, boot persistence via systemd (including a real reboot with a
cold driver bind, not just a warm restart).

Not implemented:

- **LCD screen control** — the Kraken's display is untouched. `liquidctl`
  handles this today (`liquidctl set lcd screen static image.png`) despite
  listing this PID as unsupported.
- **RGB control** for the separate `1e71:2012` RGB controller device
- The failsafe override is covered by unit tests but has not been triggered
  by real sustained heat
- CPU temperature uses the first `temp*_input` on the matched hwmon device,
  which is usually but not always the package sensor

## License

GPL-3.0 — see [LICENSE](LICENSE).

## Building

```sh
cargo build --release
cargo test --workspace
```

Requires a Rust toolchain plus Qt6, Kirigami, CMake, extra-cmake-modules and
Clang for the GUI (see [INSTALL.md](INSTALL.md#0-dependencies)).
