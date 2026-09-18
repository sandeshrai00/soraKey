import QtQuick
import QtQuick.Controls as QQC
import qs.Commons
import qs.Ui

// Single-line text input with the kit's focus + selection styling. Inherits
// from Qt Quick Controls TextField so the underlying type's API (text,
// placeholderText, accepted, editingFinished, validator, ...) is available
// to callers without re-exposing each property.
//
// Defaults bind to qs.Commons.Color so a caller with no theme overrides
// just works; foreground / accent can be overridden per instance.
// activeFocus and mouse hover use the same hover-cursor defaults, so text
// inputs match Button, Toggle, and Dropdown.
//
// Sizing is driven by font.pixelSize + the theme's input padding. The
// default 30px implicitHeight fits dialog forms.
// Sorakey fork of Omarchy's shell TextField: identical except the background
// radius uses a friendly floor (Style.cornerRadius mirrors Hyprland rounding
// and can be 0). Follows the Rounded-corners toggle; off = theme default.
// Re-sync with shell/Ui/TextField.qml on shell updates.
TextField {
  id: root

  property bool roundedCorners: false
  readonly property int friendlyRadius: root.roundedCorners ? Math.max(Style.cornerRadius, 12) : Style.cornerRadius

  property color foreground: Color.foreground
  property color accent: Color.accent

  readonly property bool _focused: activeFocus
  readonly property bool _hot: hovered
  readonly property var _borderSpec: Border.controlSpec(_focused ? "focus" : (_hot ? "hover-cursor" : "normal"), root.foreground, root.accent)

  echoMode: TextInput.Normal
  font.family: Style.font.family
  font.pixelSize: Style.font.body
  color: foreground
  selectionColor: Style.selectionFillFor(foreground, accent)
  selectedTextColor: foreground
  placeholderTextColor: Qt.darker(foreground, 1.6)

  leftPadding: Style.spacing.controlPaddingX + Border.left(_borderSpec)
  rightPadding: Style.spacing.controlPaddingX + Border.right(_borderSpec)
  topPadding: Style.spacing.inputPaddingY + Border.top(_borderSpec)
  bottomPadding: Style.spacing.inputPaddingY + Border.bottom(_borderSpec)

  background: BorderSurface {
    color: Style.controlFill(root._focused, root._hot, root.foreground, root.accent)
    borderSpec: root._borderSpec
    radius: root.friendlyRadius
  }
}
