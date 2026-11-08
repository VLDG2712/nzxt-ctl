pub mod config;
pub mod ipc_client;
pub mod model;
pub mod settings;
pub mod tray;

slint::include_modules!();

use model::{EditedCurve, Edits, LiveText};
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// Must match the .desktop file's basename: Wayland compositors (and the
/// X11 WM_CLASS) use it to find the app's name and icon for the dock.
const APP_ID: &str = "nzxt-ctl-gui";

struct App {
    ui: slint::Weak<AppWindow>,
    /// Last successfully loaded/saved config, kept so saving writes back a
    /// complete file (preserving fields the GUI doesn't expose) rather than
    /// reconstructing a partial one. `None` if it failed to load.
    cfg: Option<config::Config>,
    pump: Rc<VecModel<CurvePoint>>,
    fan: Rc<VecModel<CurvePoint>>,
    tray: Option<tray::Tray>,
}

thread_local! {
    // Everything here runs on the Slint event-loop thread; the tray's
    // D-Bus thread only reaches it through invoke_from_event_loop.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` with the app state. Callers must not trigger a nested
/// `with_app` from inside `f` (Slint setters don't re-enter callbacks
/// synchronously, so property writes are fine).
fn with_app<R>(f: impl FnOnce(&mut App, &AppWindow) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut guard = cell.borrow_mut();
        let app = guard.as_mut()?;
        let ui = app.ui.upgrade()?;
        Some(f(app, &ui))
    })
}

fn main() -> anyhow::Result<()> {
    // Creating the window initializes the Slint platform, which
    // set_xdg_app_id needs; it only has to happen before show().
    let ui = AppWindow::new()?;
    slint::set_xdg_app_id(APP_ID)?;

    let gui = settings::load();
    ui.set_tray_enabled(gui.tray_icon);
    ui.set_close_to_tray(gui.close_to_tray);
    ui.set_start_minimized(gui.start_minimized);
    ui.set_auto_start(gui.autostart);

    let pump = Rc::new(VecModel::<CurvePoint>::default());
    let fan = Rc::new(VecModel::<CurvePoint>::default());
    ui.set_pump_points(ModelRc::from(pump.clone()));
    ui.set_fan_points(ModelRc::from(fan.clone()));

    APP.with(|cell| {
        *cell.borrow_mut() = Some(App { ui: ui.as_weak(), cfg: None, pump, fan, tray: None });
    });

    with_app(|app, ui| match config::load() {
        Ok(cfg) => load_into_ui(app, ui, cfg),
        Err(e) => ui.set_status_message(format!("Failed to load config: {e:#}").into()),
    });
    with_app(sync_tray);
    refresh();

    wire_callbacks(&ui);

    // Polls the daemon over the Unix socket. Matches the daemon's own
    // 1 s control-loop cadence - no point refreshing faster than it
    // updates. Kept alive by this binding until main returns.
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), refresh);

    // Start minimized only when a tray icon actually exists - otherwise
    // there would be no way to bring the window up.
    let tray_up = with_app(|app, _| app.tray.is_some()).unwrap_or(false);
    if !(gui.start_minimized && tray_up) {
        ui.show()?;
    }
    with_app(update_tray);

    // Not slint::run_event_loop(): that returns as soon as the last
    // window is hidden, and hiding to tray must keep the app running.
    slint::run_event_loop_until_quit()?;

    // Drop the tray (deregistering the icon) before exiting.
    APP.with(|cell| cell.borrow_mut().take());
    Ok(())
}

fn wire_callbacks(ui: &AppWindow) {
    ui.on_save(|| {
        with_app(save);
    });
    ui.on_revert(|| {
        with_app(|app, ui| match config::load() {
            Ok(cfg) => {
                load_into_ui(app, ui, cfg);
                flash(ui, "Reverted to the saved config.");
            }
            // Keep the current (possibly edited) state - a failed revert
            // should not destroy the user's work on top of the error.
            Err(e) => ui.set_status_message(format!("Revert failed: {e:#}").into()),
        });
    });
    ui.on_gui_settings_changed(|| {
        with_app(|app, ui| {
            let gui = settings::GuiSettings {
                tray_icon: ui.get_tray_enabled(),
                close_to_tray: ui.get_close_to_tray(),
                start_minimized: ui.get_start_minimized(),
                autostart: ui.get_auto_start(),
            };
            // Success stays silent - the switch flipping is its own feedback.
            if let Err(e) = settings::save(&gui) {
                ui.set_status_message(format!("Failed to save GUI settings: {e:#}").into());
            }
            sync_tray(app, ui);
        });
    });

    ui.on_point_moved(|ch, i, temp, duty| {
        with_app(|app, ui| {
            let model = app.curve(ch);
            if let Ok(i) = usize::try_from(i) {
                if i < model.row_count() {
                    model.set_row_data(i, CurvePoint { temp, duty: duty.clamp(0, 100) });
                }
            }
            update_path(app, ui, ch);
        });
    });
    ui.on_point_added(|ch, temp, duty| {
        with_app(|app, ui| {
            app.curve(ch).push(CurvePoint { temp, duty: duty.clamp(0, 100) });
            update_path(app, ui, ch);
        });
    });
    ui.on_point_add_next(|ch| {
        with_app(|app, ui| {
            let (temp, duty) = model::next_point(&points_of(&app.curve(ch)));
            app.curve(ch).push(CurvePoint { temp, duty });
            update_path(app, ui, ch);
        });
    });
    ui.on_point_removed(|ch, i| {
        with_app(|app, ui| {
            let model = app.curve(ch);
            // Never remove the last point: an empty curve is rejected on save.
            if let Ok(i) = usize::try_from(i) {
                if i < model.row_count() && model.row_count() > 1 {
                    model.remove(i);
                }
            }
            update_path(app, ui, ch);
        });
    });

    ui.window().on_close_requested(|| {
        let to_tray = with_app(|app, ui| {
            ui.get_tray_enabled() && ui.get_close_to_tray() && app.tray.is_some()
        })
        .unwrap_or(false);
        if !to_tray {
            let _ = slint::quit_event_loop();
        }
        // Update the tray's "Show/Hide window" label after the hide lands.
        slint::Timer::single_shot(Duration::ZERO, || {
            with_app(update_tray);
        });
        CloseRequestResponse::HideWindow
    });
}

impl App {
    fn curve(&self, channel: i32) -> Rc<VecModel<CurvePoint>> {
        if channel == 0 { self.pump.clone() } else { self.fan.clone() }
    }
}

fn points_of(model: &VecModel<CurvePoint>) -> Vec<(f32, i32)> {
    model.iter().map(|p| (p.temp, p.duty)).collect()
}

fn set_points(model: &VecModel<CurvePoint>, points: &[(f32, i32)]) {
    model.set_vec(
        points.iter().map(|&(temp, duty)| CurvePoint { temp, duty }).collect::<Vec<_>>(),
    );
}

fn update_path(app: &App, ui: &AppWindow, channel: i32) {
    let path = SharedString::from(model::curve_path(&points_of(&app.curve(channel))));
    if channel == 0 { ui.set_pump_path(path) } else { ui.set_fan_path(path) }
}

/// Resets every editable property to `cfg`, discarding unsaved edits.
fn load_into_ui(app: &mut App, ui: &AppWindow, cfg: config::Config) {
    let e = Edits::from_config(&cfg);
    ui.set_mode_index(e.mode_index);
    ui.set_silent_duty(e.silent_duty);
    ui.set_failsafe_temp(e.failsafe_temp);
    ui.set_lcd_enabled(e.lcd_enabled);
    ui.set_lcd_source_index(e.lcd_source_index);
    ui.set_pump_source_index(e.pump.source_index);
    ui.set_fan_source_index(e.fan.source_index);
    set_points(&app.pump, &e.pump.points);
    set_points(&app.fan, &e.fan.points);
    app.cfg = Some(cfg);
    update_path(app, ui, 0);
    update_path(app, ui, 1);
    update_tray(app, ui);
}

fn collect_edits(app: &App, ui: &AppWindow) -> Edits {
    Edits {
        mode_index: ui.get_mode_index(),
        silent_duty: ui.get_silent_duty(),
        failsafe_temp: ui.get_failsafe_temp(),
        lcd_enabled: ui.get_lcd_enabled(),
        lcd_source_index: ui.get_lcd_source_index(),
        pump: EditedCurve { source_index: ui.get_pump_source_index(), points: points_of(&app.pump) },
        fan: EditedCurve { source_index: ui.get_fan_source_index(), points: points_of(&app.fan) },
    }
}

/// Writes the config, then asks the daemon to reload it.
fn save(app: &mut App, ui: &AppWindow) {
    let Some(base) = app.cfg.as_ref() else {
        ui.set_status_message("Cannot save: the config was never loaded successfully.".into());
        return;
    };
    let cfg = match model::build_config(base, &collect_edits(app, ui)) {
        Ok(c) => c,
        Err(e) => {
            ui.set_status_message(e.into());
            return;
        }
    };
    match write_and_reload(&cfg) {
        Ok(()) => {
            // Keep the in-memory copy in sync so a later save or tray mode
            // switch doesn't silently revert this one.
            app.cfg = Some(cfg);
            flash(ui, "Saved and applied.");
        }
        Err(SaveError::Reload(m)) => {
            app.cfg = Some(cfg);
            ui.set_status_message(m.into());
        }
        Err(SaveError::Write(m)) => ui.set_status_message(m.into()),
    }
    update_tray(app, ui);
}

enum SaveError {
    /// Nothing was written.
    Write(String),
    /// The file was written but the daemon didn't pick it up (it will on
    /// its next start).
    Reload(String),
}

fn write_and_reload(cfg: &config::Config) -> Result<(), SaveError> {
    config::save(cfg).map_err(|e| SaveError::Write(format!("Save failed: {e:#}")))?;
    ipc_client::request_reload()
        .map_err(|e| SaveError::Reload(format!("Saved, but the daemon reload failed: {e:#}")))
}

/// Shows a success message that clears itself; errors are set directly
/// with set_status_message and stay until the next action replaces them.
fn flash(ui: &AppWindow, message: &str) {
    ui.set_status_message(message.into());
    let weak = ui.as_weak();
    let message = SharedString::from(message);
    slint::Timer::single_shot(Duration::from_secs(4), move || {
        if let Some(ui) = weak.upgrade() {
            if ui.get_status_message() == message {
                ui.set_status_message(SharedString::new());
            }
        }
    });
}

fn refresh() {
    with_app(|app, ui| {
        match ipc_client::get_state() {
            Ok(state) => {
                let t = LiveText::from_state(&state);
                ui.set_liquid_temp(t.liquid.into());
                ui.set_cpu_temp(t.cpu.into());
                ui.set_gpu_temp(t.gpu.into());
                ui.set_pump_status(t.pump.into());
                ui.set_fan_status(t.fan.into());
                ui.set_failsafe_active(state.failsafe_active);
                ui.set_daemon_error(SharedString::new());
            }
            // Leave the last-known readings in place rather than blanking
            // them - the error banner tells the user they're stale.
            Err(e) => ui.set_daemon_error(format!("{e:#}").into()),
        }
        if app.tray.as_ref().is_some_and(|t| !t.is_alive()) {
            app.tray = None;
            ui.set_tray_available(false);
        }
        update_tray(app, ui);
    });
}

/// Creates or removes the tray icon to match the "Show tray icon" setting.
fn sync_tray(app: &mut App, ui: &AppWindow) {
    if !ui.get_tray_enabled() {
        app.tray = None;
        ui.set_tray_available(false);
        return;
    }
    if app.tray.is_none() {
        match tray::Tray::spawn(tray_view(app, ui), on_tray_action) {
            Ok(t) => app.tray = Some(t),
            Err(e) => eprintln!("nzxt-ctl-gui: no system tray available: {e}"),
        }
    }
    ui.set_tray_available(app.tray.is_some());
}

fn tray_view(app: &App, ui: &AppWindow) -> tray::TrayView {
    let mode = app.cfg.as_ref().map_or(config::Mode::Auto, |c| c.mode.active);
    let mut tooltip = format!("Liquid {}", ui.get_liquid_temp());
    if ui.get_failsafe_active() {
        tooltip.push_str(" — FAILSAFE ACTIVE");
    }
    tray::TrayView { mode, window_visible: ui.window().is_visible(), tooltip }
}

fn update_tray(app: &mut App, ui: &AppWindow) {
    let view = tray_view(app, ui);
    if let Some(t) = app.tray.as_mut() {
        t.update(view);
    }
}

fn on_tray_action(action: tray::Action) {
    match action {
        tray::Action::ToggleWindow => {
            with_app(|app, ui| {
                if ui.window().is_visible() {
                    let _ = ui.hide();
                } else {
                    let _ = ui.show();
                }
                update_tray(app, ui);
            });
        }
        tray::Action::SetMode(mode) => {
            with_app(|app, ui| apply_mode_from_tray(app, ui, mode));
        }
        tray::Action::Quit => {
            let _ = slint::quit_event_loop();
        }
    }
}

/// Writes only the mode, on top of the last *saved* config: uncommitted
/// curve edits in an open window are neither applied nor lost.
fn apply_mode_from_tray(app: &mut App, ui: &AppWindow, mode: config::Mode) {
    let Some(mut cfg) = app.cfg.clone() else {
        ui.set_status_message("Cannot switch mode: the config was never loaded.".into());
        return;
    };
    cfg.mode.active = mode;
    match write_and_reload(&cfg) {
        Ok(()) => {
            app.cfg = Some(cfg);
            ui.set_mode_index(model::mode_index(mode));
            flash(ui, &format!("Switched to {}.", mode.label()));
        }
        Err(SaveError::Reload(m)) => {
            app.cfg = Some(cfg);
            ui.set_mode_index(model::mode_index(mode));
            ui.set_status_message(m.into());
        }
        Err(SaveError::Write(m)) => ui.set_status_message(m.into()),
    }
    update_tray(app, ui);
}
