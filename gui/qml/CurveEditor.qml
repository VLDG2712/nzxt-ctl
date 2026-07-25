import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami

/*
 * Editor for one channel's fan/pump curve: a temperature-source selector,
 * a plotted curve with draggable handles, and numeric rows (collapsed by
 * default) for precise entry.
 *
 * Points are held in a local ListModel rather than bound directly to the
 * Rust side, so edits stay uncommitted until the user presses Save (same
 * semantics as the previous GTK frontend). The bridge sorts points by
 * temperature on save and the canvas sorts a copy for drawing, so neither
 * view needs to keep the model itself sorted - dragged points may cross
 * freely.
 */
ColumnLayout {
    id: root
    spacing: Kirigami.Units.smallSpacing

    /// {temp_source, points:[{temp_c, duty_pct}]} - the initial values,
    /// supplied by Main.qml from DaemonBridge.curvesJson.
    property var initialData: ({ temp_source: "liquid", points: [] })

    /// Failsafe threshold, drawn as a reference line on liquid-sourced
    /// curves (it only ever fires on liquid temp). Negative hides it.
    property int failsafeTemp: -1

    readonly property int tempMax: 150

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

    /// (Re)seeds the editor. Called at startup with initialData and again
    /// by Main.qml after a revert, when the bridge's curvesJson has been
    /// reset from disk.
    function reload(data) {
        const src = (data && data.temp_source) || "liquid";
        sourceCombo.currentIndex = sourceCombo.indexOfValue(src);
        pointModel.clear();
        const points = (data && data.points) || [];
        for (let i = 0; i < points.length; ++i) {
            pointModel.append({
                temp_c: points[i].temp_c,
                duty_pct: points[i].duty_pct
            });
        }
        curveCanvas.requestPaint();
    }

    ListModel { id: pointModel }

    Component.onCompleted: reload(initialData)

    // Add/remove repaint automatically; per-point edits repaint explicitly
    // from their handlers, since ListModel.setProperty emits no signal an
    // outside observer can watch.
    Connections {
        target: pointModel
        function onCountChanged() { curveCanvas.requestPaint(); }
    }

    onFailsafeTempChanged: curveCanvas.requestPaint()

    RowLayout {
        spacing: Kirigami.Units.smallSpacing

        QQC2.Label { text: "React to:" }

        QQC2.ComboBox {
            id: sourceCombo
            textRole: "label"
            valueRole: "value"
            model: [
                { label: "Liquid", value: "liquid" },
                { label: "CPU", value: "cpu" },
                { label: "GPU", value: "gpu" }
            ]
            onActivated: curveCanvas.requestPaint()
        }
    }

    Canvas {
        id: curveCanvas
        Layout.fillWidth: true
        implicitHeight: Kirigami.Units.gridUnit * 10
        onWidthChanged: requestPaint()
        onHeightChanged: requestPaint()

        readonly property real padL: 40
        readonly property real padR: 12
        readonly property real padT: 10
        readonly property real padB: 24

        function xFor(t) { return padL + (t / root.tempMax) * (width - padL - padR); }
        function yFor(d) { return padT + (1 - d / 100) * (height - padT - padB); }
        function tempAt(x) {
            const t = (x - padL) / (width - padL - padR) * root.tempMax;
            return Math.max(0, Math.min(root.tempMax, t));
        }
        function dutyAt(y) {
            const d = (1 - (y - padT) / (height - padT - padB)) * 100;
            return Math.max(0, Math.min(100, d));
        }

        function sortedPoints() {
            let pts = [];
            for (let i = 0; i < pointModel.count; ++i) {
                const p = pointModel.get(i);
                pts.push({ t: p.temp_c, d: p.duty_pct, idx: i });
            }
            pts.sort((a, b) => a.t - b.t);
            return pts;
        }

        /// Model index of the handle within grab range of (mx, my), or -1.
        function handleAt(mx, my) {
            for (let i = 0; i < pointModel.count; ++i) {
                const p = pointModel.get(i);
                const dx = mx - xFor(p.temp_c);
                const dy = my - yFor(p.duty_pct);
                if (dx * dx + dy * dy <= 144) // 12 px grab radius
                    return i;
            }
            return -1;
        }

        onPaint: {
            const ctx = getContext("2d");
            ctx.reset();

            // Grid + axis labels
            ctx.lineWidth = 1;
            ctx.strokeStyle = Qt.alpha(Kirigami.Theme.textColor, 0.15);
            ctx.fillStyle = Kirigami.Theme.disabledTextColor;
            ctx.font = "10px sans-serif";
            for (let d = 0; d <= 100; d += 25) {
                const y = yFor(d);
                ctx.beginPath();
                ctx.moveTo(padL, y);
                ctx.lineTo(width - padR, y);
                ctx.stroke();
                ctx.fillText(d + "%", 6, y + 3);
            }
            for (let t = 0; t <= root.tempMax; t += 30) {
                const x = xFor(t);
                ctx.beginPath();
                ctx.moveTo(x, padT);
                ctx.lineTo(x, height - padB);
                ctx.stroke();
                ctx.fillText(t + "°", x - 6, height - 8);
            }

            // Failsafe reference line - only meaningful against liquid temp
            if (root.failsafeTemp >= 0 && sourceCombo.currentValue === "liquid") {
                const x = xFor(root.failsafeTemp);
                ctx.strokeStyle = Qt.alpha(Kirigami.Theme.negativeTextColor, 0.6);
                ctx.beginPath();
                ctx.moveTo(x, padT);
                ctx.lineTo(x, height - padB);
                ctx.stroke();
            }

            const pts = sortedPoints();
            if (pts.length === 0)
                return;

            // The interpolated line, extended flat to both edges to match
            // duty_for_temp's clamping behavior outside the point range.
            ctx.strokeStyle = Kirigami.Theme.highlightColor;
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.moveTo(xFor(0), yFor(pts[0].d));
            for (const p of pts)
                ctx.lineTo(xFor(p.t), yFor(p.d));
            ctx.lineTo(xFor(root.tempMax), yFor(pts[pts.length - 1].d));
            ctx.stroke();

            // Handles (drawn after the line so they sit on top)
            for (const p of pts) {
                ctx.beginPath();
                ctx.arc(xFor(p.t), yFor(p.d),
                        p.idx === dragArea.dragIndex ? 7 : 5, 0, 2 * Math.PI);
                ctx.fillStyle = Kirigami.Theme.highlightColor;
                ctx.fill();
            }
        }

        MouseArea {
            id: dragArea
            anchors.fill: parent
            acceptedButtons: Qt.LeftButton | Qt.RightButton
            property int dragIndex: -1

            onPressed: (mouse) => {
                const idx = curveCanvas.handleAt(mouse.x, mouse.y);
                if (mouse.button === Qt.RightButton) {
                    // A curve with no points would be rejected by the
                    // bridge, so never remove the last one.
                    if (idx >= 0 && pointModel.count > 1)
                        pointModel.remove(idx);
                    return;
                }
                dragIndex = idx;
                curveCanvas.requestPaint();
            }
            onPositionChanged: (mouse) => {
                if (dragIndex < 0)
                    return;
                pointModel.setProperty(dragIndex, "temp_c",
                                       Math.round(curveCanvas.tempAt(mouse.x)));
                pointModel.setProperty(dragIndex, "duty_pct",
                                       Math.round(curveCanvas.dutyAt(mouse.y)));
                curveCanvas.requestPaint();
            }
            onReleased: {
                dragIndex = -1;
                curveCanvas.requestPaint();
            }
            onDoubleClicked: (mouse) => {
                if (mouse.button !== Qt.LeftButton)
                    return;
                if (curveCanvas.handleAt(mouse.x, mouse.y) >= 0)
                    return;
                pointModel.append({
                    temp_c: Math.round(curveCanvas.tempAt(mouse.x)),
                    duty_pct: Math.round(curveCanvas.dutyAt(mouse.y))
                });
            }
        }
    }

    QQC2.Label {
        Layout.fillWidth: true
        font: Kirigami.Theme.smallFont
        opacity: 0.7
        text: "Drag points to shape the curve • double-click to add • right-click to remove"
    }

    QQC2.Switch {
        id: numericToggle
        text: "Numeric editor"
    }

    ColumnLayout {
        visible: numericToggle.checked
        spacing: Kirigami.Units.smallSpacing

        Repeater {
            model: pointModel

            RowLayout {
                spacing: Kirigami.Units.smallSpacing

                QQC2.SpinBox {
                    id: tempSpin
                    from: 0
                    to: root.tempMax
                    editable: true
                    onValueModified: {
                        pointModel.setProperty(index, "temp_c", value);
                        curveCanvas.requestPaint();
                    }
                    QQC2.ToolTip.text: "Temperature (°C)"
                    QQC2.ToolTip.visible: hovered
                    QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay

                    // A Binding element re-asserts on model changes even
                    // after user interaction has overwritten `value` -
                    // a plain `value: model.temp_c` binding would be
                    // severed the first time the user touches the spinbox,
                    // and canvas drags would stop showing up here.
                    Binding {
                        target: tempSpin
                        property: "value"
                        value: model.temp_c
                    }
                }

                QQC2.Label { text: "°C  →" }

                QQC2.SpinBox {
                    id: dutySpin
                    from: 0
                    to: 100
                    editable: true
                    onValueModified: {
                        pointModel.setProperty(index, "duty_pct", value);
                        curveCanvas.requestPaint();
                    }
                    QQC2.ToolTip.text: "Duty cycle (%)"
                    QQC2.ToolTip.visible: hovered
                    QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay

                    Binding {
                        target: dutySpin
                        property: "value"
                        value: model.duty_pct
                    }
                }

                QQC2.Label { text: "%" }

                QQC2.ToolButton {
                    icon.name: "list-remove"
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
                    temp = Math.min(root.tempMax, last.temp_c + 5);
                    duty = Math.min(100, last.duty_pct + 10);
                }
                pointModel.append({ temp_c: temp, duty_pct: duty });
            }
        }
    }
}
