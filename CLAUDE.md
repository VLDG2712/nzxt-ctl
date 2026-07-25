# CLAUDE.md

Context for AI assistants working on this repo. Read this before making
changes — several things here were learned the hard way and are easy to
reintroduce.

## What this is

A Linux replacement for NZXT CAM: a root daemon that drives pump/fan
curves, plus a Qt6/QML + Kirigami GUI that talks to it over a Unix socket.
Target hardware is an NZXT Kraken 2023 Elite (`1e71:300c`, firmware 2.1.0)
on Arch Linux + KDE Plasma.

## Commands

```sh
cargo build --release      # both binaries
cargo test --workspace     # 12 tests, all should pass with no warnings
cargo build -p nzxt-ctl-gui   # GUI only (slow first time: compiles Qt bindings)
```

The GUI needs Qt6, Kirigami, CMake, extra-cmake-modules and Clang, plus
`qmake` on `PATH`. See INSTALL.md.

## Architecture

```
daemon/src/
  main.rs      control loop: read sensors -> failsafe check -> decide -> write pwm
  control.rs   decide_duty() - PURE function, unit tested, no hardware needed
  config.rs    TOML schema + validation
  hwmon.rs     sysfs I/O, device discovery by name, CPU/GPU temp reads
  ipc.rs       Unix socket server, JSON-lines protocol
gui/src/
  bridge.rs    cxx-qt QObject exposed to QML as the `DaemonBridge` singleton
  config.rs    MIRRORS daemon/src/config.rs - see "Schema duplication" below
  ipc_client.rs  MIRRORS the daemon's LiveState struct
gui/qml/       Main.qml + CurveEditor.qml
```

Data flow: GUI writes `/etc/nzxt-ctl/config.toml` directly, then sends
`ReloadConfig` over the socket; the daemon re-reads from disk on its next
loop iteration. The GUI never mutates daemon state directly.

## Non-obvious things that will bite you

**Hardware / sysfs**
- `pwmN_enable` must be `1` before `pwmN` writes do anything. At `0` writes
  are silently ignored — no error, no effect.
- Never hardcode a `hwmonN` path. The number changes across reboots
  (observed moving hwmon5 -> hwmon2). Always resolve by reading
  `/sys/class/hwmon/*/name`.
- `pwm1`/`fan1` = pump, `pwm2`/`fan2` = fan, `temp1_input` = coolant
  (millidegrees C).

**TOML**
- Bare keys after a `[table]` header parse as belonging to that table.
  `poll_interval_ms` must appear *before* the first `[section]` in the file
  or it silently ends up in the wrong place.

**cxx-qt / QML**
- `i18n()` does NOT work here. It needs `KLocalizedContext` installed on the
  QML engine, which requires linking KF6::I18n from C++. Without it, every
  `i18n("...")` silently returns an empty string — this rendered the entire
  UI blank-labelled once. Use plain string literals.
- `rust_mut()` requires `use cxx_qt::CxxQtType;` in scope.
- Non-`#[qproperty]` struct fields are reachable via `Deref` for reads, but
  writes go through `self.as_mut().rust_mut().field`.
- `Kirigami.Action`'s `text` does NOT do Qt mnemonic processing, so `&&`
  renders literally as `&&`. Write a single `&`.
- QML module URI is `com.local.nzxtctl`; the engine loads
  `qrc:/qt/qml/com/local/nzxtctl/qml/Main.qml`. New `.qml` files must be
  added to `gui/build.rs` or they won't be in the module.

**Permissions**
- Daemon runs as root (needs sysfs writes). GUI runs as the user. They share
  a dedicated `nzxt-ctl` group; the socket is `root:nzxt-ctl 0660`, falling
  back to `0666` with a warning if the group is missing (keeps `cargo run`
  working pre-install).
- Group membership is fixed at login. After `usermod -aG`, an existing
  session still has the old groups — needs a real re-login, or `newgrp
  nzxt-ctl` for a quick test. A `newgrp` shell is outside the desktop
  session, so xdg-desktop-portal denies it and the GUI loses system theming;
  that's expected and disappears after a proper re-login.

**Repo hygiene**
- `.claude/` and `.qmlls.ini` contain absolute home paths. They are
  gitignored and must stay that way.
- There is only ONE `Cargo.lock`, at the workspace root. Don't recreate
  `daemon/Cargo.lock`.
- `[profile.*]` belongs in the workspace root Cargo.toml; Cargo ignores it
  in member crates and warns.

## Schema duplication

`gui/src/config.rs` duplicates `daemon/src/config.rs` (and
`gui/src/ipc_client.rs` duplicates the daemon's `LiveState`). These are kept
in sync **by hand**. Adding a field to one and not the other has caused
breakage twice. If you touch either schema, update both.

Consolidating them into a shared crate is a known desirable refactor — see
PLAN.md.

## Testing conventions

- Hardware-independent logic goes in pure functions so it can be unit
  tested. `control.rs::decide_duty` is the model: the control loop reads
  sensors, then hands plain values to a pure decision function.
- Curve/JSON round-trip logic in `bridge.rs` is tested the same way.
- Anything touching real sysfs or a real socket is verified manually on
  hardware, not in tests.

## Safety

This drives cooling hardware. The failsafe (coolant >= `failsafe_temp_c`,
default 60 °C, floor of 40 °C enforced at load) forces both channels to 100%
and is checked unconditionally, before any mode branching. Do not add an
early return or a mode branch that can bypass it. If a curve's selected
temperature source is unavailable, the code deliberately holds the last duty
rather than substituting a different sensor.
