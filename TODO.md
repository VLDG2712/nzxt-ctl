# TODO

Ordered roughly by value-for-effort. See PLAN.md for the larger features.

## Verification still outstanding

- [ ] **Reboot persistence.** The systemd unit is `enabled` and was `active
      (running)`, but a real reboot has never been observed. Reboot, then
      `systemctl status nzxt-ctl.service` — expect `active (running)` with
      no manual step.
- [ ] **Failsafe under real heat.** Covered by unit tests only. Never
      triggered by actual coolant reaching 60 °C. Run a sustained CPU load,
      watch `journalctl -u nzxt-ctl -f` for the `FAILSAFE:` line, and
      confirm both channels jump to 100% and the GUI shows the red banner.
- [ ] **CPU sensor is the right one.** `discover_cpu_temp_sensor()` picks
      the first `temp*_input` on the matched hwmon device, which is usually
      but not necessarily "Package id 0". Cross-check against `sensors`
      output; if wrong, select by `tempN_label` instead.

## Gaps in what's already built

- [ ] **`silent_duty_pct` and `failsafe_temp_c` are not editable in the
      GUI.** They exist in the config and the daemon honours them, but the
      bridge exposes no qproperty for either, so they're TOML-only. Add
      two spinboxes and wire them through `bridge.rs`.
- [ ] **Curve validation errors surface late.** The GUI accepts any curve
      the spinboxes allow; a config the daemon rejects only shows up in the
      journal, not in the UI. `ChannelCurve::validate()` already exists in
      the daemon — mirror it in the bridge before writing the file.
- [ ] **No "revert to saved" in the GUI.** Editing curves and not saving
      leaves the UI out of sync with disk until restart.
- [ ] **Socket has no length limit on requests.** `handle_client` reads
      lines unbounded. Not a real concern given `0660` group-only access,
      but worth a cap if permissions are ever loosened.

## Cleanups

- [ ] **Deduplicate the config schema.** `gui/src/config.rs` and
      `daemon/src/config.rs` are maintained by hand in parallel. Extract a
      `common/` workspace crate. This has caused breakage twice.
- [ ] **Narrow the daemon off `User=root`.** It only needs write access to
      a handful of `pwm*` files. A udev rule granting the `nzxt-ctl` group
      write access to those specific attributes would let the unit drop to
      a normal user.
- [ ] Add CI (`cargo build`, `cargo test`, `cargo clippy`). Note the GUI
      needs Qt6 + Kirigami in the runner image.

## Feature work

- [ ] **Graphical curve editor** — drag-points on a plotted curve instead of
      numeric rows. The single largest UX gap versus CAM.
- [ ] **LCD screen control.** Currently untouched by this project.
      `liquidctl set lcd screen static <file.png>` works today. See PLAN.md
      for both routes.
- [ ] **RGB control** for the separate `1e71:2012` device. Completely
      unexplored — no HID descriptor pulled yet.
- [ ] Multi-GPU support: `read_nvidia_gpu_temp()` takes only the first GPU.
- [ ] AMD GPU temperature (currently NVIDIA/`nvidia-smi` only; AMD exposes
      temps via hwmon `amdgpu`, which would fit the existing sensor code).
