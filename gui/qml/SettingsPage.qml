import QtQuick
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import com.local.nzxtctl

/*
 * GUI preferences: tray behavior and autostart. Every toggle writes its
 * bridge property and persists immediately - there is no save/cancel
 * cycle here, unlike the curve editors (settings are cheap to flip back,
 * curves drive hardware).
 */
Kirigami.ScrollablePage {
    title: "Settings"

    Kirigami.FormLayout {
        QQC2.Switch {
            Kirigami.FormData.label: "Tray icon:"
            checked: DaemonBridge.trayEnabled
            onToggled: {
                DaemonBridge.trayEnabled = checked;
                DaemonBridge.saveGuiSettings();
            }
        }

        QQC2.Switch {
            Kirigami.FormData.label: "Close to tray:"
            enabled: DaemonBridge.trayEnabled
            checked: DaemonBridge.closeToTray
            onToggled: {
                DaemonBridge.closeToTray = checked;
                DaemonBridge.saveGuiSettings();
            }
            QQC2.ToolTip.text: "Closing the window keeps the app running in the tray"
            QQC2.ToolTip.visible: hovered
            QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
        }

        QQC2.Switch {
            Kirigami.FormData.label: "Start minimized:"
            enabled: DaemonBridge.trayEnabled
            checked: DaemonBridge.startMinimized
            onToggled: {
                DaemonBridge.startMinimized = checked;
                DaemonBridge.saveGuiSettings();
            }
            QQC2.ToolTip.text: "Start with only the tray icon, no window"
            QQC2.ToolTip.visible: hovered
            QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
        }

        QQC2.Switch {
            Kirigami.FormData.label: "Start on login:"
            checked: DaemonBridge.autoStart
            onToggled: {
                DaemonBridge.autoStart = checked;
                DaemonBridge.saveGuiSettings();
            }
            QQC2.ToolTip.text: "Adds an autostart entry to ~/.config/autostart"
            QQC2.ToolTip.visible: hovered
            QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
        }

        QQC2.Label {
            font: Kirigami.Theme.smallFont
            opacity: 0.7
            text: "Tray-dependent options are disabled while the tray icon is off,\nsince a hidden window with no tray icon has no way back."
        }
    }
}
