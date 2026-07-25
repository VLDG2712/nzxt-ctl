import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import com.local.nzxtctl

Kirigami.ApplicationWindow {
    id: root

    title: "NZXT Control"
    width: Kirigami.Units.gridUnit * 30
    height: Kirigami.Units.gridUnit * 40
    minimumWidth: Kirigami.Units.gridUnit * 22
    minimumHeight: Kirigami.Units.gridUnit * 20

    /// Parsed once at startup; the editors take their initial values from
    /// this and own their state thereafter.
    readonly property var curveData: JSON.parse(DaemonBridge.curvesJson)

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
            }
        }
    }
}
