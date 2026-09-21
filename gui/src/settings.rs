//! GUI-only preferences (tray behavior, autostart). Deliberately NOT part
//! of the shared schema crate: the daemon never reads these, and they live
//! per-user in ~/.config rather than in the root-owned /etc config.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GuiSettings {
    /// Show the system tray icon at all. The other tray behaviors are
    /// meaningless (and guarded in QML) without it.
    pub tray_icon: bool,
    /// Closing the window hides to tray instead of quitting.
    pub close_to_tray: bool,
    /// Start with the window hidden, tray icon only.
    pub start_minimized: bool,
    /// Maintain a .desktop entry in ~/.config/autostart.
    pub autostart: bool,
}

impl Default for GuiSettings {
    fn default() -> Self {
        Self {
            tray_icon: true,
            close_to_tray: false,
            start_minimized: false,
            autostart: false,
        }
    }
}

fn config_home() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config")
        })
}

fn settings_path() -> PathBuf {
    config_home().join("nzxt-ctl").join("gui.toml")
}

fn autostart_path() -> PathBuf {
    config_home().join("autostart").join("nzxt-ctl-gui.desktop")
}

/// Missing or unreadable file just means defaults - GUI prefs are not
/// worth refusing to start over.
pub fn load() -> GuiSettings {
    load_from(&settings_path())
}

fn load_from(path: &Path) -> GuiSettings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(settings: &GuiSettings) -> Result<()> {
    save_to(settings, &settings_path())?;
    sync_autostart(settings.autostart, &autostart_path())
}

fn save_to(settings: &GuiSettings, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {:?}", dir))?;
    }
    let text = toml::to_string_pretty(settings).context("serializing GUI settings")?;
    std::fs::write(path, text).with_context(|| format!("writing GUI settings to {:?}", path))
}

/// The .desktop file is (re)written on every enable rather than only when
/// absent, so a moved binary self-heals the Exec path on the next save.
fn sync_autostart(enable: bool, path: &Path) -> Result<()> {
    if enable {
        let exe = std::env::current_exe().context("resolving current executable path")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {:?}", dir))?;
        }
        std::fs::write(path, desktop_entry(&exe))
            .with_context(|| format!("writing autostart entry to {:?}", path))
    } else {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing autostart entry {:?}", path)),
        }
    }
}

/// `Exec=` is quoted and escaped per the Desktop Entry spec so a binary
/// path containing spaces or shell-special characters still launches.
fn desktop_entry(exe: &Path) -> String {
    let escaped = exe
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=NZXT Control\n\
         Comment=NZXT Kraken pump/fan control\n\
         Exec=\"{}\"\n\
         Icon=cpu\n\
         Terminal=false\n\
         X-KDE-StartupNotify=false\n",
        escaped
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nzxt-ctl-gui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn settings_round_trip_and_missing_file_defaults() {
        let path = temp_dir().join("gui.toml");

        // Missing file -> defaults, not an error
        assert_eq!(load_from(&path), GuiSettings::default());

        let custom = GuiSettings {
            tray_icon: false,
            close_to_tray: true,
            start_minimized: true,
            autostart: true,
        };
        save_to(&custom, &path).unwrap();
        assert_eq!(load_from(&path), custom);

        // Corrupt file -> defaults, not an error
        std::fs::write(&path, "not [valid toml").unwrap();
        assert_eq!(load_from(&path), GuiSettings::default());
    }

    #[test]
    fn autostart_sync_creates_and_removes_entry() {
        let path = temp_dir().join("autostart").join("nzxt-ctl-gui.desktop");

        sync_autostart(true, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("[Desktop Entry]"));
        assert!(text.contains("Exec="));

        sync_autostart(false, &path).unwrap();
        assert!(!path.exists());
        // Disabling when already absent is not an error
        sync_autostart(false, &path).unwrap();
    }
}
