use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    let builder = CxxQtBuilder::new_qml_module(
        QmlModule::new("com.local.nzxtctl")
            .qml_file("qml/Main.qml")
            .qml_file("qml/CurveEditor.qml")
            .qml_file("qml/SettingsPage.qml"),
    )
    .files(["src/bridge.rs"]);

    // GCC 16 added -Wsfinae-incomplete, which fires inside Qt's own
    // headers (qchar.h) on every cxx-qt translation unit - dozens of lines
    // of noise per build that have nothing to do with this crate.
    // `flag_if_supported` keeps clang and older GCC happy.
    // SAFETY: marked unsafe by cxx-qt-build only because the cc::Build
    // internals carry no stability guarantee; adding a warning flag is
    // the documented use of this hook.
    let builder = unsafe {
        builder.cc_builder(|cc| {
            cc.flag_if_supported("-Wno-sfinae-incomplete");
        })
    };

    builder.build();
}
