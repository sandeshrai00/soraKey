import QtQuick
import "SoraKeyStore.js" as Model

// Single source of truth for Sorakey UI state. All daemon answers flow
// through here; views bind to the derived props only and never guess.
// Policy: after install or any daemon lifecycle change, Checking… holds a
// fixed 3s (max ~4s with poll jitter, always <7s) no matter what the
// daemon reports — answers are cached, the timer owns the reveal.
Item {
  id: store

  // ---- raw state (written only by apply* below) ----
  property bool installed: false
  property bool running: false
  property bool muted: false
  property real volume: 100
  property string keyboardPack: ""
  property var keyboardPacks: []
  property real perPackVolume: 100
  property string inputError: ""
  property var packLoaded: null
  property string packError: ""
  property string audioError: ""
  property var audioDevices: []
  property string audioDeviceSelected: ""
  property bool statusKnown: false
  // packs answered at least once with a non-empty list: gates the controls
  // so stale defaults (100%, empty picker) can never paint. Empty means
  // not knowledge — installer guarantees >=1 pack so a legit-empty hold
  // can't stick. ponytail: one length check, upgrade only if packs gain
  // required fields.
  property bool packsKnown: false
  // auto-install attempts consumed (max 3, then manual Install only)
  property int setupRetries: 0

  // ---- dumb hold: fixed 3s Checking, never ends early ----
  property bool installHold: false
  Timer {
    id: installHoldTimer
    interval: 3000
    onTriggered: { store.installHold = false }
  }
  function beginHold() {
    store.installHold = true
    installHoldTimer.restart()
  }

  // ---- derived (pure projections, no timers) ----
  readonly property string statusText: {
    if (!store.installed) return "Not installed"
    if (store.installHold) return "Checking…"
    if (!store.statusKnown) return "Checking…"
    if (!store.running) return "Stopped"
    if (store.inputError !== "") return "No keyboard access"
    if (store.packLoaded === false) return "Pack failed"
    if (store.keyboardPack === "" && store.keyboardPacks.length === 0) return "No soundpack"
    return store.muted ? "Muted" : "Playing"
  }
  readonly property bool showWhyBlock: store.inputError !== ""
  readonly property string healthHint: {
    if (store.showWhyBlock) return ""
    if (store.packLoaded === false && store.packError !== "") return "Soundpack failed: " + store.packError
    if (store.audioError !== "") return "Audio problem: " + store.audioError
    return ""
  }
  // Controls paint only when status AND packs are both known. Errors
  // (WhyBlock banner, Start button, Pack-failed) need no pack list, so
  // they render immediately after the hold like before.
  readonly property bool captureReady: store.installed && store.statusKnown && store.packsKnown && store.inputError === ""

  function noteInstalled(present) {
    store.installed = present
  }

  function resetForInstall() {
    store.statusKnown = false
    store.packsKnown = false
    store.beginHold()
  }

  // Returns true when the daemon just came (back) up.
  function applyStatus(text) {
    var o = Model.parseStatus(text)
    if (!o) return false
    if (o.ok === true) {
      var daemonJustUp = !store.running
      // guarded writes: identical poll answers must not fan out bindings at 1Hz
      if (store.running !== true) store.running = true
      if (store.installed !== true) store.installed = true
      var muted = o.muted === true
      if (store.muted !== muted) store.muted = muted
      if (typeof o.volume === "number" && store.volume !== o.volume) store.volume = o.volume
      if (typeof o.per_pack_volume === "number" && store.perPackVolume !== o.per_pack_volume) store.perPackVolume = o.per_pack_volume
      var pack = String(o.keyboard_pack || "")
      if (store.keyboardPack !== pack) store.keyboardPack = pack
      // Single-poll trust: the installHold window already covers the
      // daemon's startup pre-scan lie, so no confirm counter is needed.
      var ie = (typeof o.input_error !== "undefined")
        ? (o.input_error ? String(o.input_error) : "")
        : store.inputError
      if (ie !== "") {
        if (store.inputError !== ie) store.inputError = ie
      } else {
        if (store.inputError !== "") store.inputError = ""
      }
      // During the hold, cache only — the timer owns the reveal.
      if (!store.installHold && !store.statusKnown) store.statusKnown = true
      if (typeof o.pack_loaded !== "undefined") {
        var pl = (o.pack_loaded === true) ? true : ((o.pack_loaded === false) ? false : null)
        if (store.packLoaded !== pl) store.packLoaded = pl
      }
      if (typeof o.pack_error !== "undefined") {
        var pe = o.pack_error ? String(o.pack_error) : ""
        if (store.packError !== pe) store.packError = pe
      }
      if (typeof o.audio_error !== "undefined") {
        var ae = o.audio_error ? String(o.audio_error) : ""
        if (store.audioError !== ae) store.audioError = ae
      }
      if (typeof o.audio_device !== "undefined") {
        var dev = o.audio_device ? String(o.audio_device) : ""
        if (store.audioDeviceSelected !== dev) store.audioDeviceSelected = dev
      }
      return daemonJustUp
    }
    store.running = false
    // daemon answered but not ok: cache it; the hold still owns the reveal.
    if (!store.installHold && !store.statusKnown) store.statusKnown = true
    return false
  }

  // allowEmpty: only the user-initiated delete flow may legitimately
  // empty the list. Otherwise an empty answer means "daemon mid pack-sync
  // rm/cp window" — keep the old list instead of flashing an empty picker.
  function applyPacks(text, allowEmpty) {
    var p = Model.parsePacks(text)
    if (p.keyboard.length > 0 || allowEmpty) {
      store.keyboardPacks = p.keyboard
      if (p.keyboard.length > 0) store.packsKnown = true
    }
  }

  // Returns device list or null on failure/empty (caller keeps old list).
  function parseDevices(text) {
    var out = String(text || "").trim()
    try {
      var r = JSON.parse(out)
      if (r && r.ok && Array.isArray(r.devices) && r.devices.length > 0) return r.devices
    } catch (e) {}
    return null
  }
}
