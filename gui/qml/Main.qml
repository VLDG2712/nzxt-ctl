import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
// Qualified: Qt.labs.platform's Menu/MenuItem would otherwise shadow the
// QtQuick.Controls ones.
import Qt.labs.platform as Platform
import org.kde.kirigami as Kirigami
import com.local.nzxtctl

Kirigami.ApplicationWindow {
    id: root

    title: "NZXT Control"
    width: Kirigami.Units.gridUnit * 30
    height: Kirigami.Units.gridUnit * 40
    minimumWidth: Kirigami.Units.gridUnit * 22
    minimumHeight: Kirigami.Units.gridUnit * 20

    // Start-minimized: the window stays unshown and only the tray icon
    // appears. Evaluated once here rather than kept as a live binding, so
    // flipping the setting later doesn't yank the open window away.
    visible: false
    Component.onCompleted: {
        if (!(DaemonBridge.startMinimized && DaemonBridge.trayEnabled))
            root.show();
    }

    onClosing: (close) => {
        if (DaemonBridge.closeToTray && trayIcon.available && DaemonBridge.trayEnabled) {
            close.accepted = false;
            root.hide();
        }
    }

    function toggleWindow() {
        if (root.visible) {
            root.hide();
        } else {
            root.show();
            root.raise();
            root.requestActivate();
        }
    }

    /// Applies a mode directly from the tray: only the mode is written -
    /// curvesJson still holds the last-saved curves, so uncommitted edits
    /// in an open window are neither applied nor lost.
    function applyModeFromTray(mode) {
        DaemonBridge.mode = mode;
        DaemonBridge.save();
    }

    /// Parsed once at startup; the editors take their initial values from
    /// this and own their state thereafter.
    readonly property var curveData: JSON.parse(DaemonBridge.curvesJson)

    Platform.SystemTrayIcon {
        id: trayIcon
        visible: DaemonBridge.trayEnabled
        icon.name: "cpu"
        tooltip: "NZXT Control — liquid " + DaemonBridge.liquidTemp
            + (DaemonBridge.failsafeActive ? " — FAILSAFE ACTIVE" : "")

        onActivated: (reason) => {
            // Left-click toggles the window; right-click is the menu.
            if (reason === Platform.SystemTrayIcon.Trigger)
                root.toggleWindow();
        }

        menu: Platform.Menu {
            Platform.MenuItem {
                text: root.visible ? "Hide window" : "Show window"
                onTriggered: root.toggleWindow()
            }
            Platform.MenuSeparator {}
            Platform.MenuItem {
                text: "Performance"
                checkable: true
                checked: DaemonBridge.mode === "performance"
                onTriggered: root.applyModeFromTray("performance")
            }
            Platform.MenuItem {
                text: "Silent"
                checkable: true
                checked: DaemonBridge.mode === "silent"
                onTriggered: root.applyModeFromTray("silent")
            }
            Platform.MenuItem {
                text: "Auto"
                checkable: true
                checked: DaemonBridge.mode === "auto"
                onTriggered: root.applyModeFromTray("auto")
            }
            Platform.MenuSeparator {}
            Platform.MenuItem {
                text: "Quit"
                onTriggered: Qt.quit()
            }
        }
    }

    // Polls the daemon over the Unix socket. Matches the daemon's own
    // 1 s control-loop cadence - no point refreshing faster than it
    // updates.
    Timer {
        interval: 1000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: DaemonBridge.refresh()
    }

    pageStack.initialPage: Kirigami.ScrollablePage {
        title: "NZXT Control"

        actions: [
            Kirigami.Action {
                text: "Save & Apply"
                icon.name: "document-save"
                onTriggered: {
                    DaemonBridge.curvesJson = JSON.stringify({
                        pump: pumpEditor.toJson(),
                        fan: fanEditor.toJson()
                    });
                    DaemonBridge.save();
                }
            },
            Kirigami.Action {
                text: "Revert"
                icon.name: "document-revert"
                onTriggered: {
                    DaemonBridge.revert();
                    // The curve editors own their state after seeding, so
                    // they must be re-seeded from the freshly loaded JSON.
                    const data = JSON.parse(DaemonBridge.curvesJson);
                    pumpEditor.reload(data.pump);
                    fanEditor.reload(data.fan);
                }
            },
            Kirigami.Action {
                text: "Hide to tray"
                icon.name: "window-minimize"
                visible: DaemonBridge.trayEnabled && trayIcon.available
                onTriggered: root.hide()
            },
            Kirigami.Action {
                text: "Settings"
                icon.name: "settings-configure"
                onTriggered: {
                    if (root.pageStack.layers.depth < 2)
                        root.pageStack.layers.push(settingsComponent);
                }
            }
        ]

        ColumnLayout {
            spacing: Kirigami.Units.largeSpacing

            Kirigami.InlineMessage {
                Layout.fillWidth: true
                type: Kirigami.MessageType.Error
                text: "Failsafe active: temperature ceiling reached, cooling forced to 100%"
                visible: DaemonBridge.failsafeActive
            }

            Kirigami.InlineMessage {
                Layout.fillWidth: true
                type: Kirigami.MessageType.Warning
                text: "Daemon unreachable: " + DaemonBridge.daemonError
                visible: DaemonBridge.daemonError !== ""
            }

            Kirigami.InlineMessage {
                Layout.fillWidth: true
                type: Kirigami.MessageType.Information
                text: DaemonBridge.statusMessage
                visible: DaemonBridge.statusMessage !== ""
            }

            // --- Mode selector ---
            Kirigami.Heading {
                text: "Mode"
                level: 2
            }

            QQC2.ButtonGroup { id: modeGroup }

            RowLayout {
                Layout.fillWidth: true
                spacing: 0

                QQC2.Button {
                    text: "Performance"
                    checkable: true
                    Layout.fillWidth: true
                    QQC2.ButtonGroup.group: modeGroup
                    checked: DaemonBridge.mode === "performance"
                    onClicked: DaemonBridge.mode = "performance"
                }
                QQC2.Button {
                    text: "Silent"
                    checkable: true
                    Layout.fillWidth: true
                    QQC2.ButtonGroup.group: modeGroup
                    checked: DaemonBridge.mode === "silent"
                    onClicked: DaemonBridge.mode = "silent"
                }
                QQC2.Button {
                    text: "Auto"
                    checkable: true
                    Layout.fillWidth: true
                    QQC2.ButtonGroup.group: modeGroup
                    checked: DaemonBridge.mode === "auto"
                    onClicked: DaemonBridge.mode = "auto"
                }
            }

            QQC2.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                font: Kirigami.Theme.smallFont
                text: DaemonBridge.mode === "auto"
                    ? "Curves below drive pump and fan speed."
                    : "Curves below are ignored while this mode is active."
            }

            Kirigami.FormLayout {
                Layout.fillWidth: true

                QQC2.SpinBox {
                    id: silentDutySpin
                    Kirigami.FormData.label: "Silent mode duty:"
                    from: 0
                    to: 100
                    editable: true
                    onValueModified: DaemonBridge.silentDuty = value
                    QQC2.ToolTip.text: "Fixed pump and fan duty (%) while in Silent mode"
                    QQC2.ToolTip.visible: hovered
                    QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay

                    // Binding elements survive user interaction, so a
                    // Revert still pushes the reloaded value back in.
                    Binding {
                        target: silentDutySpin
                        property: "value"
                        value: DaemonBridge.silentDuty
                    }
                }

                QQC2.SpinBox {
                    id: failsafeSpin
                    Kirigami.FormData.label: "Failsafe at:"
                    // The shared config validation refuses anything below
                    // 40 °C, so don't let the spinbox go there at all.
                    from: 40
                    to: 100
                    editable: true
                    onValueModified: DaemonBridge.failsafeTemp = value
                    QQC2.ToolTip.text: "Liquid temperature (°C) that forces both channels to 100%, in any mode"
                    QQC2.ToolTip.visible: hovered
                    QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay

                    Binding {
                        target: failsafeSpin
                        property: "value"
                        value: DaemonBridge.failsafeTemp
                    }
                }
            }

            // --- Live status ---
            Kirigami.Separator { Layout.fillWidth: true }

            Kirigami.Heading {
                text: "Live Status"
                level: 2
            }

            Kirigami.FormLayout {
                Layout.fillWidth: true

                QQC2.Label {
                    Kirigami.FormData.label: "Liquid:"
                    text: DaemonBridge.liquidTemp
                }
                QQC2.Label {
                    Kirigami.FormData.label: "CPU:"
                    text: DaemonBridge.cpuTemp
                }
                QQC2.Label {
                    Kirigami.FormData.label: "GPU:"
                    text: DaemonBridge.gpuTemp
                }
                QQC2.Label {
                    Kirigami.FormData.label: "Pump:"
                    text: DaemonBridge.pumpStatus
                }
                QQC2.Label {
                    Kirigami.FormData.label: "Fan:"
                    text: DaemonBridge.fanStatus
                }
            }

            // --- Curves ---
            Kirigami.Separator { Layout.fillWidth: true }

            Kirigami.Heading {
                text: "Pump Curve"
                level: 2
            }

            CurveEditor {
                id: pumpEditor
                Layout.fillWidth: true
                initialData: root.curveData.pump
                failsafeTemp: DaemonBridge.failsafeTemp
            }

            Kirigami.Separator { Layout.fillWidth: true }

            Kirigami.Heading {
                text: "Case Fan Curve"
                level: 2
            }

            CurveEditor {
                id: fanEditor
                Layout.fillWidth: true
                initialData: root.curveData.fan
                failsafeTemp: DaemonBridge.failsafeTemp
            }
        }
    }

    Component {
        id: settingsComponent
        SettingsPage {}
    }
}
