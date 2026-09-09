import QtQuick
import QtQuick.Controls as QQC
import qs.Commons
import qs.Ui
import "SoraKeyStore.js" as Model

// Searchable dropdown fork with inline delete. Search stays in popup.
Item {
  id: root

  property string label: ""
  property string value: ""
  property var options: []
  property string placeholderText: "Search…"
  property string emptyText: "No matches"
  property color foreground: Color.popups.text
  property color background: Color.popups.background
  property color popupBorder: Color.popups.border
  property color accent: Color.accent
  readonly property var popupBorderSpec: Border.localOrSurfaceSpec("popups", "border", popupBorder, Color.popups.border, Style.normalBorderWidth)
  property string fontFamily: Style.font.family
  property int rowHeight: Style.spacing.controlHeight
  property int popupRowHeight: Style.spacing.popupRowHeight
  property int popupMinHeight: Style.spacing.searchablePopupMinHeight
  property bool showLabel: true
  property bool hasCursor: false

  readonly property bool popupOpen: popup.opened
  property double lastClosedAt: 0
  function open() { popup.open() }
  function close() { popup.close() }
  function toggle() { popup.opened ? popup.close() : popup.open() }

  signal changed(string value)
  signal deleteRequested(string value)
  signal confirmDelete(string value)
  signal cancelDelete()

  property string deleteConfirmId: ""
  property bool deleting: false
  property string toast: ""
  // Rounded-corner floor (see Panel): Style.cornerRadius can be 0.
  // Follows Panel's Rounded-corners toggle; off = theme default.
  property bool roundedCorners: false
  readonly property int friendlyRadius: root.roundedCorners ? Math.max(Style.cornerRadius, 12) : Style.cornerRadius
  // Tall-button padding shared with Panel (see its buttonYPadding).
  readonly property int buttonYPadding: Style.space(10)
  function prettyForValue(v) {
    for (var i = 0; i < options.length; i++) if (optionValue(options[i]) === v) return optionLabel(options[i])
    return Model.prettyPackName(String(v))
  }

  function optionValue(o) {
    return (o && typeof o === "object") ? String(o.value) : String(o)
  }
  function optionLabel(o) {
    return (o && typeof o === "object") ? String(o.label) : String(o)
  }
  function optionPre(o) {
    return !!(o && typeof o === "object" && o.isPre)
  }
  readonly property bool hasPre: {
    for (var i = 0; i < options.length; i++) {
      if (optionPre(options[i])) return true
    }
    return false
  }
  function currentLabel() {
    for (var i = 0; i < options.length; i++) {
      if (optionValue(options[i]) === value) return optionLabel(options[i])
    }
    return value
  }

  property var filtered: options
  function recomputeFiltered() {
    var q = searchField.text.toLowerCase()
    if (!q) { filtered = options; return }
    // filter on label only
    filtered = options.filter(function(o){ return optionLabel(o).toLowerCase().indexOf(q) !== -1 })
  }

  onOptionsChanged: recomputeFiltered()

  implicitWidth: Style.spacing.searchableDropdownWidth
  implicitHeight: showLabel && label !== "" ? rowHeight + Style.spacing.huge : rowHeight

  Column {
    anchors.fill: parent
    spacing: Style.spacing.labelGap

    Text {
      textFormat: Text.PlainText
      visible: root.showLabel && root.label !== ""
      text: root.label
      color: Qt.darker(root.foreground, 1.4)
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
      font.bold: true
    }

    BorderSurface {
      id: trigger
      width: parent.width
      height: root.rowHeight
      radius: root.friendlyRadius

      readonly property bool _focused: trigger.activeFocus
      readonly property bool _hot: triggerHover.hovered || root.hasCursor
      readonly property var _borderSpec: Border.controlSpec(trigger._focused ? "focus" : (trigger._hot ? "hover-cursor" : "normal"), root.foreground, root.accent)

      color: Style.controlFill(trigger._focused, trigger._hot, root.foreground, root.accent)
      borderSpec: _borderSpec

      activeFocusOnTab: true

      HoverHandler { id: triggerHover }

      Keys.onPressed: function(event) {
        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter
            || event.key === Qt.Key_Space || event.key === Qt.Key_Down) {
          popup.opened ? popup.close() : popup.open()
          event.accepted = true
        } else if (event.key === Qt.Key_Escape && popup.opened) {
          popup.close(); event.accepted = true
        }
      }

      Text {
        textFormat: Text.PlainText
        anchors.left: parent.left
        anchors.right: chevron.left
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: trigger.borderLeft + Style.spacing.controlPaddingX
        anchors.rightMargin: trigger.borderRight + Style.spacing.md
        text: root.currentLabel() || root.placeholderText
        color: root.currentLabel() ? root.foreground : Qt.darker(root.foreground, 1.5)
        font.family: root.fontFamily
        font.pixelSize: Style.font.body
        elide: Text.ElideRight
      }

      Text {
        id: chevron
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.rightMargin: trigger.borderRight + Style.spacing.controlGap
        text: "󰅀"
        color: Qt.darker(root.foreground, 1.2)
        font.family: root.fontFamily
        font.pixelSize: Style.font.body
      }

      MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onPressed: function(mouse) {
          trigger.forceActiveFocus()
          if (popup.opened) popup.close()
          else if (Date.now() - root.lastClosedAt > 300) popup.open()
          mouse.accepted = true
        }
      }

      QQC.Popup {
        id: popup
        x: 0
        y: trigger.height + Style.spacing.xxs
        width: trigger.width
        property int confirmH: root.deleteConfirmId !== "" ? confirmFooter.height + 1 : 0
        property int toastH: root.toast !== "" ? toastFooter.height + 1 : 0
        implicitHeight: Math.max(root.popupMinHeight,
                                 Math.min(resultList.contentHeight + Style.space(50) + confirmH + toastH,
                                          root.popupRowHeight * 6 + 5 * Style.spacing.labelGap + Style.space(50) + confirmH + toastH))
        padding: Style.spacing.hairline
        leftPadding: Border.left(root.popupBorderSpec) + Style.spacing.hairline
        rightPadding: Border.right(root.popupBorderSpec) + Style.spacing.hairline
        topPadding: Border.top(root.popupBorderSpec) + Style.spacing.hairline
        bottomPadding: Border.bottom(root.popupBorderSpec) + Style.spacing.hairline
        focus: true

        background: BorderSurface {
          color: root.background
          borderSpec: root.popupBorderSpec
          radius: root.friendlyRadius
        }

        onOpened: {
          searchField.text = ""
          root.recomputeFiltered()
          Qt.callLater(function() { searchField.forceActiveFocus() })
        }
        onClosed: {
          searchField.text = ""
          root.lastClosedAt = Date.now()
          // keep confirmId — Panel clears it
        }

        contentItem: Column {
          spacing: 0

          Item {
            id: searchHeader
            width: parent.width
            height: root.popupRowHeight + Style.spacing.controlPaddingX

              SoraTextField {
                id: searchField
                anchors.fill: parent
                roundedCorners: root.roundedCorners
              anchors.margins: Style.spacing.md
              placeholderText: root.placeholderText
              foreground: root.foreground
              accent: root.accent
              font.family: root.fontFamily
              font.pixelSize: Style.font.body

              onTextChanged: {
                root.recomputeFiltered()
                if (resultList.count > 0) resultList.currentIndex = 0
              }

              Keys.onPressed: function(event) {
                if (event.key === Qt.Key_Escape) {
                  popup.close(); event.accepted = true
                } else if (event.key === Qt.Key_Down) {
                  if (resultList.count > 0) {
                    resultList.currentIndex = 0
                    resultList.forceActiveFocus()
                  }
                  event.accepted = true
                } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                  if (resultList.count > 0) {
                    resultList.currentIndex = 0
                    resultList.selectCurrent()
                  }
                  event.accepted = true
                }
              }
            }
          }

          Rectangle {
            width: parent.width
            height: 1
            color: Util.alpha(root.foreground, 0.10)
          }

          Item {
            id: preLegend
            visible: root.hasPre
            width: parent.width
            height: visible ? Style.font.caption + Style.spacing.sm : 0
            Text {
              textFormat: Text.PlainText
              anchors.centerIn: parent
              text: "(pre) = preinstalled"
              color: Qt.darker(root.foreground, 1.4)
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
            }
          }

          Item {
            id: listContainer
            width: parent.width
            height: popup.height - searchHeader.height - 1 - (preLegend.visible ? preLegend.height : 0) - (confirmFooter.visible ? confirmFooter.height + 1 : 0) - (toastFooter.visible ? toastFooter.height + 1 : 0) - Style.spacing.xxs

            Text {
              textFormat: Text.PlainText
              anchors.centerIn: parent
              visible: resultList.count === 0
              text: root.emptyText
              color: Qt.darker(root.foreground, 1.6)
              font.family: root.fontFamily
              font.pixelSize: Style.font.body
            }

            ListView {
              id: resultList
              anchors.fill: parent
              spacing: root.roundedCorners ? Style.spacing.sm : Style.spacing.labelGap
              clip: true
              boundsBehavior: Flickable.StopAtBounds
              model: root.filtered
              currentIndex: -1
              keyNavigationEnabled: false

              function selectCurrent() {
                if (root.deleteConfirmId !== "") { root.deleteConfirmId = ""; root.cancelDelete(); return }
                if (currentIndex < 0 || currentIndex >= root.filtered.length) return
                var v = root.optionValue(root.filtered[currentIndex])
                root.value = v
                root.changed(v)
                popup.close()
              }

              Keys.priority: Keys.BeforeItem
              Keys.onPressed: function(event) {
                if (event.key === Qt.Key_Escape) {
                  if (root.deleteConfirmId !== "") { root.deleteConfirmId = ""; root.cancelDelete(); event.accepted = true; return }
                  popup.close(); event.accepted = true
                } else if (event.key === Qt.Key_Down || event.text === "j") {
                  if (resultList.currentIndex >= resultList.count - 1) {
                    event.accepted = true; return
                  }
                  resultList.currentIndex = resultList.currentIndex + 1
                  event.accepted = true
                } else if (event.key === Qt.Key_Up || event.text === "k") {
                  if (resultList.currentIndex <= 0) {
                    searchField.forceActiveFocus()
                    event.accepted = true; return
                  }
                  resultList.currentIndex = resultList.currentIndex - 1
                  event.accepted = true
                } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                  resultList.selectCurrent(); event.accepted = true
                }
              }

              // Rounded highlight pills (intentional divergence from the
              // shell's square-row original — friendlier list, matches the
              // rounded trigger/popup around it).
              delegate: Rectangle {
                required property var modelData
                required property int index
                width: resultList.width
                height: Math.max(root.popupRowHeight, rowContent.implicitHeight + Style.spacing.rowPaddingX)
                radius: root.friendlyRadius
                clip: true
                color: index === resultList.currentIndex
                  ? Style.hoverFillFor(root.foreground, root.accent)
                  : "transparent"

                Row {
                  id: rowContent
                  anchors.left: parent.left
                  anchors.right: delBtn.left
                  anchors.verticalCenter: parent.verticalCenter
                  anchors.leftMargin: Style.spacing.controlPaddingX
                  anchors.rightMargin: Style.spacing.md
                  spacing: Style.spacing.xxs

                  Column {
                    width: parent.width
                    spacing: Style.spacing.xxs
                    anchors.verticalCenter: parent.verticalCenter

                    Row {
                      width: parent.width
                      spacing: Style.spacing.xs
                      Text {
                        textFormat: Text.PlainText
                        text: root.optionLabel(modelData)
                        color: index === resultList.currentIndex ? Style.hoverStateColor(root.foreground, root.accent) : root.foreground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.body
                        elide: Text.ElideRight
                        width: parent.width - (preBadge.visible ? preBadge.width + Style.spacing.xs : 0)
                      }
                      Text {
                        id: preBadge
                        textFormat: Text.PlainText
                        visible: root.optionPre(modelData)
                        text: "(pre)"
                        color: index === resultList.currentIndex ? Style.hoverStateColor(root.foreground, root.accent) : Qt.darker(root.foreground, 1.5)
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.caption
                        anchors.verticalCenter: parent.verticalCenter
                      }
                    }
                  }
                }

                Text {
                  id: delBtn
                  anchors.right: parent.right
                  anchors.verticalCenter: parent.verticalCenter
                  anchors.rightMargin: Style.spacing.controlPaddingX
                  text: "✕"
                  color: index === resultList.currentIndex ? Style.hoverStateColor(root.foreground, root.accent) : Qt.darker(root.foreground, 1.3)
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.body
                  opacity: (index === resultList.currentIndex || root.deleteConfirmId === root.optionValue(modelData)) ? 0.9 : 0
                  visible: (index === resultList.currentIndex || root.deleteConfirmId === root.optionValue(modelData))

                  MouseArea {
                    anchors.fill: parent
                    anchors.margins: -Style.spacing.xs
                    cursorShape: Qt.PointingHandCursor
                    hoverEnabled: true
                    onClicked: {
                      var v = root.optionValue(modelData)
                      root.deleteRequested(v)
                      mouse.accepted = true
                    }
                  }
                }

                MouseArea {
                  anchors.left: parent.left
                  anchors.right: delBtn.left
                  anchors.top: parent.top
                  anchors.bottom: parent.bottom
                  hoverEnabled: true
                  cursorShape: Qt.PointingHandCursor
                  onPositionChanged: resultList.currentIndex = parent.index
                  onClicked: resultList.selectCurrent()
                }
              }
            }
          }

          // confirm footer
          Rectangle {
            id: confirmSep
            visible: root.deleteConfirmId !== ""
            width: parent.width
            height: 1
            color: Util.alpha(root.foreground, 0.10)
          }
          Column {
            id: confirmFooter
            visible: root.deleteConfirmId !== ""
            width: parent.width
            spacing: Style.spacing.xs
            property string pretty: root.prettyForValue(root.deleteConfirmId)
            Item { width: 1; height: Style.spacing.xs }
            Row {
              width: parent.width - Style.spacing.md*2
              anchors.horizontalCenter: parent.horizontalCenter
spacing: Style.spacing.xs
               Text {
                 text: "⚠"
                 color: "#ff6b6b"
                font.family: root.fontFamily
                font.pixelSize: Style.font.body
              }
              Text {
                textFormat: Text.PlainText
                text: "Delete \"" + parent.parent.pretty + "\"?"
                color: root.foreground
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
                elide: Text.ElideRight
                width: parent.width - 20
                wrapMode: Text.WordWrap
              }
            }
            Row {
              width: parent.width - Style.spacing.md*2
              anchors.horizontalCenter: parent.horizontalCenter
              spacing: Style.spacing.sm
              Button {
                text: root.deleting ? "Deleting…" : "Delete"
                iconText: ""
                iconSpinning: root.deleting
                foreground: "#ff6b6b"
                radius: root.friendlyRadius
                verticalPadding: root.buttonYPadding
                enabled: !root.deleting
                onClicked: root.confirmDelete(root.deleteConfirmId)
              }
              Button {
                text: "Cancel"
                foreground: root.foreground
                radius: root.friendlyRadius
                verticalPadding: root.buttonYPadding
                enabled: !root.deleting
                onClicked: { root.deleteConfirmId = ""; root.cancelDelete() }
              }
            }
            Item { width: 1; height: Style.spacing.xs }
           }
          Column {
            id: toastFooter
            visible: root.toast !== ""
            width: parent.width
            spacing: Style.spacing.xs
            Rectangle {
              width: parent.width
              height: 1
              color: Util.alpha(root.foreground, 0.10)
            }
            Text {
              textFormat: Text.PlainText
              width: parent.width - Style.spacing.md*2
              anchors.horizontalCenter: parent.horizontalCenter
              text: root.toast
              color: root.foreground
              opacity: 0.7
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
              horizontalAlignment: Text.AlignHCenter
            }
            Item { width: 1; height: Style.spacing.xs }
          }
         }
       }
     }
   }
 }
