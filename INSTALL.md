# Installing nzxt-ctl

On Arch, `yay -S nzxt-ctl-git` (or any AUR helper) does everything below
for you. This guide is for building and installing by hand instead.

## 0. Dependencies

The daemon needs no runtime dependencies beyond the `nzxt_kraken3` kernel
module (in-tree; loads automatically). The GUI is Qt6/QML + Kirigami:

```sh
sudo pacman -S --needed qt6-base qt6-declarative kirigami extra-cmake-modules cmake clang
```

`cmake`, `extra-cmake-modules` and `clang` are build-time only (used by
`cxx-qt-build`); `qmake` must be on `PATH`. On a KDE Plasma system all of
these are typically already installed.

## 1. Build

```sh
cargo build --release
```

Produces `target/release/nzxt-ctl-daemon` and `target/release/nzxt-ctl-gui`.

## 2. Install the daemon binary + systemd unit

```sh
sudo install -Dm755 target/release/nzxt-ctl-daemon /usr/local/bin/nzxt-ctl-daemon
sudo install -Dm644 systemd/nzxt-ctl.service /etc/systemd/system/nzxt-ctl.service
sudo systemctl daemon-reload
```

## 3. Create the shared group and add yourself to it

The daemon runs as root (it needs to write `/sys/class/hwmon/.../pwm*`), but
the GUI runs as your regular user. A dedicated `nzxt-ctl` group lets your
user read/write the config file and talk to the daemon's IPC socket without
either being world-writable.

```sh
sudo systemd-sysusers systemd/nzxt-ctl-sysusers.conf
sudo usermod -aG nzxt-ctl "$USER"
```

**Log out and back in** (or run `newgrp nzxt-ctl` in the shell you'll launch
the GUI from) for the new group membership to take effect.

## 4. Fix config file ownership

```sh
sudo mkdir -p /etc/nzxt-ctl
sudo cp -n config/default.toml /etc/nzxt-ctl/config.toml
sudo chgrp nzxt-ctl /etc/nzxt-ctl /etc/nzxt-ctl/config.toml
sudo chmod 775 /etc/nzxt-ctl
sudo chmod 664 /etc/nzxt-ctl/config.toml
```

## 5. Start the daemon

```sh
sudo systemctl enable --now nzxt-ctl.service
systemctl status nzxt-ctl.service
```

The daemon creates `/run/nzxt-ctl/daemon.sock`. If the `nzxt-ctl` group
exists (step 3), it chowns the socket to `root:nzxt-ctl` with mode `0660`.
If the group is missing, it falls back to a world-writable `0666` socket
and logs a warning — check `journalctl -u nzxt-ctl` if the GUI can't
connect.

## 6. Verify

```sh
journalctl -u nzxt-ctl -f          # watch startup + control loop logs
ls -l /run/nzxt-ctl/daemon.sock    # expect srw-rw---- root:nzxt-ctl
cat /sys/class/hwmon/*/name | grep kraken   # confirm the device is found
```

Launch `nzxt-ctl-gui` (after re-logging in for the group membership to
apply) and confirm:
- Live dashboard shows temps/RPM updating.
- Changing the mode / editing a curve and clicking **Save** succeeds
  without a permissions error, and the daemon picks it up (check the
  journal for a "config reloaded" line).

## 7. Confirm boot persistence

Reboot, then check:

```sh
systemctl status nzxt-ctl.service
```

It should be `active (running)` with no manual intervention.
