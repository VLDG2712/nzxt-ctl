//! StatusNotifierItem tray icon via ksni. Works on KDE Plasma, COSMIC,
//! XFCE, LXQt and others with an SNI host; GNOME needs the AppIndicator
//! extension (as with every SNI tray).
//!
//! ksni runs the D-Bus service on its own thread. Menu clicks are handed
//! back to the Slint event loop through `slint::invoke_from_event_loop`,
//! so the `on_action` callback runs on the UI thread and may touch the
//! window freely.

use crate::config::Mode;
use crate::model::MODES;
use ksni::blocking::{Handle, TrayMethods};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ToggleWindow,
    SetMode(Mode),
    Quit,
}

/// Everything the tray displays, pushed in from the UI thread.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayView {
    pub mode: Mode,
    pub window_visible: bool,
    pub tooltip: String,
}

struct KraTray {
    view: TrayView,
    on_action: fn(Action),
}

fn dispatch(f: fn(Action), a: Action) {
    // Fails only once the event loop has quit, when there's nothing left
    // to act on anyway.
    let _ = slint::invoke_from_event_loop(move || f(a));
}

impl ksni::Tray for KraTray {
    fn id(&self) -> String {
        // Must match the .desktop file / xdg app id so hosts that group
        // or configure tray entries per application recognise it.
        "nzxt-ctl-gui".into()
    }

    fn title(&self) -> String {
        "NZXT Control".into()
    }

    fn icon_name(&self) -> String {
        "cpu".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "NZXT Control".into(),
            description: self.view.tooltip.clone(),
            ..Default::default()
        }
    }

    // Left click toggles the window; right click opens the menu (that
    // part is the host's job).
    fn activate(&mut self, _x: i32, _y: i32) {
        dispatch(self.on_action, Action::ToggleWindow);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: if self.view.window_visible { "Hide window" } else { "Show window" }
                    .into(),
                activate: Box::new(|t: &mut Self| dispatch(t.on_action, Action::ToggleWindow)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            RadioGroup {
                selected: MODES.iter().position(|m| *m == self.view.mode).unwrap_or(2),
                select: Box::new(|t: &mut Self, i| {
                    if let Some(&m) = MODES.get(i) {
                        dispatch(t.on_action, Action::SetMode(m));
                    }
                }),
                options: MODES
                    .iter()
                    .map(|m| RadioItem { label: m.label().into(), ..Default::default() })
                    .collect(),
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| dispatch(t.on_action, Action::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub struct Tray {
    handle: Handle<KraTray>,
    last: TrayView,
}

impl Tray {
    /// Registers the icon. Fails when no SNI host is running (e.g. GNOME
    /// without the AppIndicator extension) - callers treat that as "no
    /// tray", which disables close-to-tray / start-minimized so the window
    /// can never become unreachable.
    pub fn spawn(view: TrayView, on_action: fn(Action)) -> anyhow::Result<Self> {
        let handle = KraTray { view: view.clone(), on_action }
            .disable_dbus_name(is_sandboxed())
            .spawn()?;
        Ok(Self { handle, last: view })
    }

    /// Pushes new state to the tray; a no-op when nothing changed, so it's
    /// cheap to call on every 1 s refresh.
    pub fn update(&mut self, view: TrayView) {
        if view == self.last {
            return;
        }
        self.last = view.clone();
        self.handle.update(|t| t.view = view);
    }

    pub fn is_alive(&self) -> bool {
        !self.handle.is_closed()
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        // Deregisters from the watcher so the icon disappears immediately
        // when the tray is switched off in Settings. Not waited on.
        let _ = self.handle.shutdown();
    }
}

/// Flatpak/Snap sandboxes can't own the well-known SNI bus name; ksni's
/// docs recommend skipping it there.
fn is_sandboxed() -> bool {
    std::path::Path::new("/.flatpak-info").exists() || std::env::var_os("SNAP").is_some()
}
