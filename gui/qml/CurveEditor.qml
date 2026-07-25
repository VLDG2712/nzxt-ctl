import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami

/*
 * Editor for one channel's fan/pump curve: a temperature-source selector
 * plus an editable list of {temp_c, duty_pct} points.
 *
 * Points are held in a local ListModel rather than bound directly to the
 * Rust side, so edits stay uncommitted until the user presses Save (same
 * semantics as the previous GTK frontend).
 */
Kirigami.FormLayout {
    id: root

    /// {temp_source, points:[{temp_c, duty_pct}]} - the initial values,
    /// supplied by Main.qml from DaemonBridge.curvesJson.
    property var initialData: ({ temp_source: "liquid", points: [] })

    /// Serialises the current editor state back into the same JSON shape
    /// the Rust bridge expects.
    function toJson() {
        let points = [];
        for (let i = 0; i < pointModel.count; ++i) {
            const p = pointModel.get(i);
            points.push({ temp_c: p.temp_c, duty_pct: p.duty_pct });
        }
        return {
            temp_source: sourceCombo.currentValue,
            points: points
        };
    }

    ListModel { id: pointModel }

    Component.onCompleted: {
        const src = initialData.temp_source || "liquid";
        sourceCombo.currentIndex = sourceCombo.indexOfValue(src);
        const points = initialData.points || [];
        for (let i = 0; i < points.length; ++i) {
            pointModel.append({
                temp_c: points[i].temp_c,
                duty_pct: points[i].duty_pct
            });
        }
    }

    QQC2.ComboBox {
        id: sourceCombo
        Kirigami.FormData.label: "React to:"
        textRole: "label"
        valueRole: "value"
        model: [
            { label: "Liquid", value: "liquid" },
            { label: "CPU", value: "cpu" },
            { label: "GPU", value: "gpu" }
        ]
    }

    Kirigami.Separator {
        Kirigami.FormData.isSection: true
        Kirigami.FormData.label: "Curve points"
    }

    Repeater {
        model: pointModel

        RowLayout {
            spacing: Kirigami.Units.smallSpacing

            QQC2.SpinBox {
                from: 0
                to: 150
                value: model.temp_c
                editable: true
                onValueModified: pointModel.setProperty(index, "temp_c", value)
                QQC2.ToolTip.text: "Temperature (°C)"
                QQC2.ToolTip.visible: hovered
                QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
            }

            QQC2.Label { text: "°C  →" }

            QQC2.SpinBox {
                from: 0
                to: 100
                value: model.duty_pct
                editable: true
                onValueModified: pointModel.setProperty(index, "duty_pct", value)
                QQC2.ToolTip.text: "Duty cycle (%)"
                QQC2.ToolTip.visible: hovered
                QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
            }

            QQC2.Label { text: "%" }

            QQC2.ToolButton {
                icon.name: "list-remove"
                // A curve with no points would be rejected by the bridge,
                // so keep at least one row present.
                enabled: pointModel.count > 1
                onClicked: pointModel.remove(index)
                QQC2.ToolTip.text: "Remove this point"
                QQC2.ToolTip.visible: hovered
                QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
            }
        }
    }

    QQC2.Button {
        text: "Add point"
        icon.name: "list-add"
        onClicked: {
            // Seed from the last point so a new row starts somewhere
            // sensible rather than at 0 °C / 0 %.
            let temp = 20, duty = 50;
            if (pointModel.count > 0) {
                const last = pointModel.get(pointModel.count - 1);
                temp = Math.min(150, last.temp_c + 5);
                duty = Math.min(100, last.duty_pct + 10);
            }
            pointModel.append({ temp_c: temp, duty_pct: duty });
        }
    }
}
