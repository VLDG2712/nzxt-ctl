pub mod bridge;
pub mod config;
pub mod ipc_client;
pub mod settings;

// QApplication (widgets), not QGuiApplication: on KDE the tray icon's menu
// is provided by KStatusNotifierItem, which constructs a widgets QMenu and
// qFatals under a gui-only application. Verified by core dump - do not
// "simplify" this back.
use cxx_qt_lib::{QQmlApplicationEngine, QUrl};
use cxx_qt_lib_extras::QApplication;

fn main() {
    let mut app = QApplication::new();
    let mut engine = QQmlApplicationEngine::new();

    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from(
            "qrc:/qt/qml/com/local/nzxtctl/qml/Main.qml",
        ));
    }

    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
