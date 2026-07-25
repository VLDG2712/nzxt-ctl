use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("com.local.nzxtctl")
            .qml_file("qml/Main.qml")
            .qml_file("qml/CurveEditor.qml"),
    )
    .files(["src/bridge.rs"])
    .build();
}
