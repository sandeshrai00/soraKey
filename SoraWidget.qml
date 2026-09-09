import QtQuick
import QtQuick.Controls
import QtQuick.Effects
import Quickshell
import Quickshell.Io
import qs.Ui
import qs.Commons
import "SoraKeyStore.js" as Model

// Sorakey panel — status, mute/volume, soundpacks, install controls.
// Polls `sorakey ctl status` (every second when open, 10s when closed); ctl/systemctl are one-shot.
Panel {
  id: root
  readonly property string pluginId: "io.github.sandeshrai00.sorakey"
  moduleName: root.pluginId
  ipcTarget: root.pluginId

  readonly property string home: Quickshell.env("HOME")
  readonly property string sorakeyBin: home + "/.local/bin/sorakey"
  readonly property string pluginDir: home + "/.config/omarchy/plugins/" + root.pluginId
  readonly property string setupPath: pluginDir + "/scripts/sora-install"

  // shell-managed service — survives panel rebuilds
  readonly property var service: bar?.shell?.firstPartyServiceFor(root.pluginId)
  readonly property string pluginVersion: service && service.manifest && service.manifest.version ? String(service.manifest.version) : ""
  property string pluginCommit: ""

  // plugin logo — bar follows foreground like other icons, hero wears the theme accent
  readonly property string barLogoSource: root.bar.foreground.hslLightness > 0.5 ? "assets/icon-bar-dark.svg" : "assets/icon-bar-light.svg"
  // hero logo follows the theme accent when on, plain foreground white/black when off
  property bool heroMatchTheme: true
  readonly property string logoModeFile: home + "/.config/sorakey/logo-color-mode"
  // timestamps of last popup close per settings dropdown (debounce trigger re-open race)
  property double barDropClosedAt: 0
  property double audioDropClosedAt: 0

  Connections {
    target: barSectionDrop
    function onPopupOpenChanged() { if (!barSectionDrop.popupOpen) root.barDropClosedAt = Date.now() }
  }
  Connections {
    target: audioDrop
    function onPopupOpenChanged() { if (!audioDrop.popupOpen) root.audioDropClosedAt = Date.now() }
  }

  property bool importing: service ? service.importing : false
  property string importStatus: {
    if (importing) return "Importing…"
    if (!service) return ""
    if (service.lastImportError) return service.lastImportError
    if (service.lastImportResult) return "Imported: " + service.lastImportResult
    return ""
  }

  function triggerImport() {
    if (!service) return
    // close first — dialog is below the overlay
    root.close()
    service.importSoundpack()
  }

  property bool exporting: service ? service.exporting : false
  property string exportStatus: {
    if (exporting) return "Exporting…"
    if (!service) return ""
    if (service.lastExportError) return service.lastExportError
    if (service.lastExportResult) return "Saved to " + service.lastExportResult
    return ""
  }

  function triggerExport() {
    if (!service) return
    root.close()
    service.exportLogs()
  }

  property string syncStatus: {
    if (!service) return ""
    if (service.lastBuildError) return "Update failed: " + service.lastBuildError
    if (service.lastSyncResult) return "Soundpacks updated: " + service.lastSyncResult
    return ""
  }

  // Single source of truth for daemon state — all status/packs/device
  // answers flow through the store; views bind to it, never guess.
  SoraAppStore { id: store }
  // one-tap keyboard-access enable flow (panel button → script → GUI approval)
  property bool captureBusy: false
  property bool terminalBusy: false
  property string captureStatus: ""
  property string deleteConfirmId: ""
  property bool deleting: false
  property string errorToast: ""
  // single persistent result slot: every result feed mirrors here, so the
  // panel shows the latest outcome until the next one (no auto-clear)
  property string lastResult: ""
  // single truncation point for every user-visible result/error string
  function shortText(s) { return String(s || "").slice(0, 500) }
  onImportStatusChanged: if (root.importStatus !== "") root.lastResult = root.shortText(root.importStatus)
  onExportStatusChanged: if (root.exportStatus !== "") root.lastResult = root.shortText(root.exportStatus)
  onSyncStatusChanged: if (root.syncStatus !== "") root.lastResult = root.shortText(root.syncStatus)
  onErrorToastChanged: if (root.errorToast !== "") root.lastResult = root.shortText(root.errorToast)
  onUpdateStatusChanged: if (root.updateStatus !== "") root.lastResult = root.shortText(root.updateStatus)
  onCaptureStatusChanged: if (root.captureStatus !== "") root.lastResult = root.shortText(root.captureStatus)
  property string pendingCtlCmd: ""
  Timer { id: clearErrorToast; interval: 5000; onTriggered: root.errorToast = "" }

  // Shared control heights: default button padding is 6 (too tight),
  // Enable sits at 12 as the primary action; everything else uses 10.
  readonly property int buttonYPadding: Style.space(10)
  // Rounded-corner floor: Style.cornerRadius mirrors Hyprland rounding,
  // which can be 0 (square desktop). Our controls stay friendly regardless.
  // Flipped from Settings ("Rounded corners"); persisted like heroMatchTheme.
  // Default off: fresh installs match the desktop theme until the user opts in.
  property bool roundedCorners: false
  readonly property string roundedModeFile: home + "/.config/sorakey/rounded-corners"
  readonly property int friendlyRadius: root.roundedCorners ? Math.max(Style.cornerRadius, 12) : Style.cornerRadius
  // In-panel trust explanation for the permission step. Static words only:
  // what is happening, why, what the button does, and the privacy promise.
  // Shown instead of healthHint when capture is blocked. Stays visible
  // DURING the enable run too, so the box never vanishes mid-approval —
  // the buttons flip to their loading state instead.
  // (Visibility key: store.showWhyBlock — owned by the store.)
  // enable-run phase text: approval dialog first, then the script's verify
  // loop (up to ~10s). Driven by a timer, cleared on process exit.
  property string capturePhase: ""
  Timer {
    id: capturePhaseTimer
    interval: 8000
    onTriggered: if (root.captureBusy) root.capturePhase = "Verifying access…"
  }
  Timer {
    id: terminalTimeout
    interval: 30000
    onTriggered: {
      if (root.terminalBusy) {
        root.terminalBusy = false
        root.capturePhase = ""
        capturePhaseTimer.stop()
        // No success signal arrived: the terminal never opened (missing
        // TERMINAL?) or the script never ran — say so, don't just go quiet.
        root.errorToast = "Terminal did not respond — check a terminal is installed."
        clearErrorToast.restart()
      }
    }
  }
  // post-success settling: the script exits 0 as soon as the rule works,
  // but the daemon only re-scans keyboards every ~5s, so status still
  // reports blocked for a few seconds after. Latch a finishing state
  // until the daemon itself clears inputError (15s cap), so the panel
  // never flashes idle Enable buttons mid-handoff. No double-taps.
  property bool captureSettling: false
  Timer {
    id: captureSettleTimer
    interval: 15000
    onTriggered: root.captureSettling = false
  }
  Connections {
    target: store
    function onInputErrorChanged() {
      if (store.inputError === "" && root.captureSettling) {
        root.captureSettling = false
        captureSettleTimer.stop()
      }
      if (store.inputError === "" && root.terminalBusy) {
        root.terminalBusy = false
        terminalTimeout.stop()
        capturePhaseTimer.stop()
        root.capturePhase = "Finishing up…"
        root.captureSettling = true
        captureSettleTimer.restart()
      }
    }
  }
  readonly property string whyLearnMoreUrl: "https://github.com/sandeshrai00/soraKey/blob/main/docs/keyboard-access.md"

  // one input while busy OR settling: buttons, spinner, phase text share it
  readonly property bool captureWorking: root.captureBusy || root.captureSettling || root.terminalBusy
  function enableCapture() {
    if (root.captureWorking || captureProc.running) return
    root.captureBusy = true
    root.captureStatus = ""
    root.capturePhase = "Waiting for approval…"
    capturePhaseTimer.restart()
    captureProc.command = ["/usr/bin/bash", root.pluginDir + "/scripts/sora-keyboard-access.sh"]
    captureProc.running = true
  }

  // Last-resort route for boxes without any approval dialog: open the
  // system terminal (whatever is installed) with the enable script in
  // sudo mode, so the password goes into the user's own terminal.
  // Quoting: the script path is quoted but --use-sudo stays OUTSIDE those
  // quotes (still inside the -c string) — quoting them together would make
  // bash look for a file literally named "... --use-sudo".
  function fixInTerminal() {
    if (root.captureWorking || root.terminalBusy) return
    var term = Quickshell.env("TERMINAL") || "xdg-terminal-exec"
    var script = root.pluginDir + "/scripts/sora-keyboard-access.sh"
    root.terminalBusy = true
    root.capturePhase = "Check your terminal…"
    capturePhaseTimer.restart()
    terminalTimeout.restart()
    Quickshell.execDetached([term, "--", "/usr/bin/bash", "-c",
      "\"" + script + "\" --use-sudo; echo; read -n1 -rp 'Press any key to close…'"])
  }

  property bool setupBusy: false
  // auto-install attempts live in store.setupRetries (max 3, then manual
  // Install button only). Manual taps never consume.
  property bool settingsOpen: false
  property bool uninstallArmed: false
  property bool uninstallBusy: false
  Timer { id: disarmUninstall; interval: 5000; onTriggered: root.uninstallArmed = false }
  onSettingsOpenChanged: {
    if (!settingsOpen) root.uninstallArmed = false
    if (settingsOpen && root.pluginCommit === "" && !commitProc.running)
      commitProc.running = true
  }

  property bool updateBusy: false
  property string updateStatus: ""

  // bar uses implicit size
  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  function sendCtl(obj) {
    if (!store.installed) return
    if (ctlProc.running) return
    pendingCtlCmd = String(obj && obj.cmd ? obj.cmd : "")
    ctlProc.command = [root.sorakeyBin, "ctl", JSON.stringify(obj)]
    ctlProc.running = true
  }

  function runService(args) {
    if (svcProc.running) return
    // daemon state is about to change: hold Checking… 3s like an install.
    store.beginHold()
    svcProc.command = ["systemctl", "--user"].concat(args)
    svcProc.running = true
  }

  function setMuted(on) {
    store.muted = on
    root.sendCtl({ cmd: "mute", muted: on })
  }

  function setVolume(v) {
    store.volume = v
    root.sendCtl({ cmd: "volume", value: v })
  }

  function setKeyboardPack(id) {
    store.keyboardPack = id
    root.sendCtl({ cmd: "keyboard_pack", id: id })
  }

  function setPerPackVolume(v) {
    store.perPackVolume = v
    if (store.keyboardPack) root.sendCtl({ cmd: "per_pack_volume", id: store.keyboardPack, value: v })
    else root.sendCtl({ cmd: "volume", value: v })
  }

  function resetVolume() {
    if (!store.keyboardPack) return
    root.sendCtl({ cmd: "reset_volume", id: store.keyboardPack })
  }

  function deletePack(id) {
    if (!id || root.deleting) return
    root.deleting = true
    root.sendCtl({ cmd: "delete_pack", id: id })
  }

  function pickRandomPack() {
    if (!store.keyboardPacks || store.keyboardPacks.length === 0) return
    var pool = store.keyboardPacks
    if (pool.length > 1 && store.keyboardPack) pool = pool.filter(function(id){ return id !== store.keyboardPack })
    var pick = pool[Math.floor(Math.random()*pool.length)]
    if (pick) root.setKeyboardPack(pick)
  }

  function startDaemon() {
    if (stopFlagProc.running) return
    // argv, no shell: spaces or quotes in $HOME can't break this.
    // The service action waits for the flag write (stopFlagProc.onExited):
    // firing systemctl first would race it and resurrect a stopped daemon.
    root.pendingSvcAction = ["start", "sorakey"]
    stopFlagProc.command = ["rm", "-f", root.home + "/.local/share/sorakey/stopped"]
    stopFlagProc.running = true
  }
  function stopDaemon() {
    // sticky stop — Service must not auto-restart what the user stopped.
    // The path travels as $1, never inside shell text.
    if (stopFlagProc.running) return
    root.pendingSvcAction = ["stop", "sorakey"]
    stopFlagProc.command = ["/usr/bin/bash", "-c", 'mkdir -p "$1" && printf stopped > "$1/stopped"', "_", root.home + "/.local/share/sorakey"]
    stopFlagProc.running = true
  }
  function restartDaemon() {
    if (stopFlagProc.running) return
    root.pendingSvcAction = ["restart", "sorakey"]
    stopFlagProc.command = ["rm", "-f", root.home + "/.local/share/sorakey/stopped"]
    stopFlagProc.running = true
  }
  function doUpdate() { if (root.updateBusy) return; root.updateBusy=true; root.updateStatus="Updating…"; updateProc.command=["omarchy","plugin","update",root.pluginId,"--yes"]; updateProc.running=true }

  function install() {
    if (setupBusy) return
    if (root.uninstallBusy || uninstallProc.running) return // never install mid-uninstall
    setupBusy = true
    store.resetForInstall()
    setupProc.command = ["/usr/bin/bash", root.setupPath]
    setupProc.running = true
  }

  function openCustomFolder() {
    var path = home + "/.local/share/sorakey/soundpacks"
    Quickshell.execDetached(["xdg-open", path])
  }

  readonly property string currentBarSection: {
    var cfg = root.bar && root.bar.shell ? root.bar.shell.shellConfig : null
    var layout = cfg && cfg.bar && cfg.bar.layout ? cfg.bar.layout : null
    if (!layout) return "right"
    var id = root.pluginId
    for (var s of ["left","center","right"]) {
      var arr = layout[s]
      if (!Array.isArray(arr)) continue
      for (var i=0;i<arr.length;i++) if (arr[i] && arr[i].id===id) return s
    }
    return "right"
  }

  function moveToSection(section) {
    if (["left","center","right"].indexOf(section)===-1) return
    // Verifiable move: execDetached can't report failure, so a Process runs
    // the enable and toasts when the bar icon refuses to move.
    if (!moveProc.running) {
      moveProc.command = ["omarchy","plugin","enable",root.pluginId,"--section",section]
      moveProc.running = true
    }
    // save choice — Omarchy resets to right on re-enable
    if (!sectionWrite.running) {
      sectionWrite.command = [root.sorakeyBin, "ctl", "{\"cmd\":\"set_bar_section\",\"section\":\"" + section + "\"}"]
      sectionWrite.running = true
    }
  }

  Process {
    id: moveProc
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode !== 0) {
        var detail = String(stderr.text || "").trim().split("\n").pop()
        root.errorToast = "Could not move bar icon" + (detail !== "" ? ": " + detail : ".")
        clearErrorToast.restart()
      }
    }
  }

  // restore saved bar section
  Process {
    id: sectionRead
    command: [root.sorakeyBin, "ctl", "{\"cmd\":\"get_bar_section\"}"]
    stdout: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      // A failed read (daemon down, corrupt output) keeps the current
      // section: never navigate on garbage.
      if (exitCode !== 0) return
      try {
        var resp = JSON.parse(String(stdout.text || "").trim())
        if (resp.ok && resp.section && resp.section !== root.currentBarSection)
          root.moveToSection(resp.section)
      } catch(e) {}
    }
  }

  Process {
    id: sectionWrite
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode !== 0) {
        root.errorToast = "Could not save bar position."
        clearErrorToast.restart()
      }
    }
  }

  // restore saved hero theme toggle ("1"/"0", legacy "theme"/"default")
  Process {
    id: logoRead
    command: ["cat", root.logoModeFile]
    stdout: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode !== 0) return
      var mode = String(stdout.text || "").trim()
      if (mode === "1" || mode === "theme") root.heroMatchTheme = true
      else if (mode === "0" || mode === "default") root.heroMatchTheme = false
    }
  }

  Process {
    id: logoWrite
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode !== 0) {
        root.errorToast = "Could not save theme choice."
        clearErrorToast.restart()
      }
    }
  }

  function setHeroMatchTheme(on) {
    root.heroMatchTheme = on
    if (!logoWrite.running) {
      logoWrite.command = ["sh", "-c", "mkdir -p \"$(dirname \"" + root.logoModeFile + "\")\" && printf '%s' \"" + (on ? "1" : "0") + "\" > \"" + root.logoModeFile + "\""]
      logoWrite.running = true
    }
  }

  // restore saved rounded-corners toggle ("1" = on, anything else = off)
  Process {
    id: roundedRead
    command: ["cat", root.roundedModeFile]
    stdout: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode !== 0) return
      var mode = String(stdout.text || "").trim()
      if (mode === "0") root.roundedCorners = false
      else if (mode === "1") root.roundedCorners = true
    }
  }

  Process {
    id: roundedWrite
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode !== 0) {
        root.errorToast = "Could not save corner choice."
        clearErrorToast.restart()
      }
    }
  }

  function setRoundedCorners(on) {
    root.roundedCorners = on
    if (!roundedWrite.running) {
      roundedWrite.command = ["sh", "-c", "mkdir -p \"$(dirname \"" + root.roundedModeFile + "\")\" && printf '%s' \"" + (on ? "1" : "0") + "\" > \"" + root.roundedModeFile + "\""]
      roundedWrite.running = true
    }
  }

  Process {
    id: commitProc
    command: ["git", "-C", root.pluginDir, "rev-parse", "--short", "HEAD"]
    stdout: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode === 0) root.pluginCommit = String(stdout.text || "").trim()
    }
  }

  Process {
    id: updateProc
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      root.updateBusy = false
      var err = String(stderr.text || "").trim()
      if (exitCode === 0) root.updateStatus = String(stdout.text || "").trim().split("\n").pop().replace(root.pluginId, "Sorakey")
      else root.updateStatus = err !== "" ? err : "Update failed."
      clearUpdateTimer.restart()
    }
  }



  Timer {
    id: clearUpdateTimer
    interval: 5000
    onTriggered: root.updateStatus = ""
  }

  // auto-select imported pack
  Connections {
    target: root.service
    function onPacksImported(packId) {
      root.refreshPacks()
      if (packId) root.setKeyboardPack("keyboard/" + packId)
    }
  }

  function remove() {
    // full wipe: the script removes binary, unit, packs, config, caches,
    // runtime files and the keyboard-access rule, then we unregister.
    if (root.uninstallBusy || uninstallProc.running) return
    if (setupBusy || setupProc.running) return // never uninstall mid-install
    root.uninstallBusy = true
    uninstallProc.command = ["/usr/bin/bash", root.pluginDir + "/scripts/sora-uninstall.sh", "--purge"]
    uninstallProc.running = true
  }

  function finishRemove(ok, err) {
    root.uninstallBusy = false
    if (ok) {
      root.uninstallArmed = false
      Quickshell.execDetached(["omarchy", "plugin", "remove", root.pluginId, "--yes"])
      store.installed = false
      store.running = false
    } else {
      root.uninstallArmed = false
      root.errorToast = root.shortText(err || "Uninstall failed.")
      clearErrorToast.restart()
    }
  }

  // Status answers go to the store (single source of truth); a just-up
  // daemon also reloads devices + packs here.
  function applyStatus(text) {
    if (store.applyStatus(text)) {
      root.refreshAudioDevices()
      root.refreshPacks()
    }
  }

  function refreshStatus() {
    if (statusProc.running) return
    statusProc.running = true
  }

  function refreshPacks() {
    if (!store.installed) return
    if (packsProc.running) return
    packsProc.running = true
  }

  function refreshAudioDevices() {
    if (!store.installed) return
    if (devicesProc.running) return
    devicesProc.running = true
  }

  function setAudioDevice(id) {
    // empty string = system default
    store.audioDeviceSelected = id
    root.sendCtl({ cmd: "select_device", id: id === "" ? null : id })
  }

  Component.onCompleted: {
    installCheck.running = true
    root.refreshStatus()
    // no refreshAudioDevices here: installCheck→installed fires it on boot,
    // and applyStatus reloads it on daemonJustUp — a third caller only
    // doubles the empty-fetch retries on slow daemons.
    sectionRead.running = true
    logoRead.running = true
    roundedRead.running = true
  }

  onOpenedChanged: {
      if (root.opened) {
        root.refreshStatus()
        root.refreshPacks()
        root.refreshAudioDevices()
        root.settingsOpen = false
        // clear the typing test box on every open
        Qt.callLater(function() { if (testType) testType.text = "" })
      }
   }

  // Detect (re)install without a daemon: the install is complete only
  // when BOTH the binary and the service unit exist. Binary-without-unit
  // (uninstall kept ~/.local/bin, manual unit delete) used to read as
  // "installed": setup never ran, auto-start skipped silently, and every
  // device query failed with "Device refresh failed". Now it re-installs.
  Process {
    id: installCheck
    command: ["/usr/bin/bash", "-c", 'test -x "$1" && test -f "$2"', "_", root.sorakeyBin, root.home + "/.config/systemd/user/sorakey.service"]
    onExited: function(exitCode) {
      store.installed = (exitCode === 0)
      if (store.installed) {
        root.refreshStatus()
        root.refreshAudioDevices()
      } else if (store.setupRetries < 3 && !setupBusy && !root.uninstallBusy && !uninstallProc.running) {
        // Auto-run setup after URL install (like Spotify). Bounded at 3 —
        // more failures mean something persistent; the manual Install
        // button (unlimited) takes over. installCheck's 5s recheck plus
        // this counter replace the old one-shot flag + retry timer.
        store.setupRetries += 1
        Qt.callLater(function(){ root.install() })
      }
    }
  }

  Process {
    id: statusProc
    command: [root.sorakeyBin, "ctl", "{\"cmd\":\"status\"}"]
    onExited: function(exitCode) {
      if (exitCode === 0) {
        // store owns the answers; a just-up daemon also reloads devices + packs here.
        if (store.applyStatus(stdout.text)) {
          root.refreshAudioDevices()
          root.refreshPacks()
        }
      } else {
        // Either not installed or stopped (daemon mid-restart, socket
        // gone, or answered not-ok): down immediately. Lifecycle changes
        // hold Checking… via installHold, so no bounding counter is
        // needed — the hold covers the restart window, and a truly dead
        // daemon resolves to the Start button on the next reveal.
        store.running = false
        if (!store.installHold && !store.statusKnown) store.statusKnown = true
      }
    }
    stdout: StdioCollector { waitForEnd: true }
  }

  Process {
    id: packsProc
    command: [root.sorakeyBin, "ctl", "{\"cmd\":\"packs\"}"]
    onExited: function(exitCode) {
      if (exitCode !== 0) {
        if (root.deleting) { root.deleting = false }
        return
      }
      store.applyPacks(stdout.text, root.deleting)
      if (root.deleting) {
        root.deleting = false
        root.deleteConfirmId = ""
      }
    }
    stdout: StdioCollector { waitForEnd: true }
  }

  // Audio output devices
  Process {
    id: devicesProc
    command: [root.sorakeyBin, "ctl", "{\"cmd\":\"audio_devices\"}"]
    running: false
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode, exitStatus) {
      var devs = store.parseDevices(String(stdout.text || ""))
      if (!devs) {
        // Stay silent and keep the existing list: daemonJustUp, panel-open
        // and Rescan re-fire this. The old 3× retry + toast is what flashed
        // "Device refresh failed" on healthy installs.
        return
      }
      // normalize to [{value,label}] + prepend System default
      var opts = devs.map(function(d){ return { value: String(d.id), label: String(d.name) } })
      opts.unshift({ value: "", label: "System default" })
      var ids = opts.map(function(o){ return o.value }).join("\n")
      var cur = store.audioDevices.map(function(o){ return o.value }).join("\n")
      if (ids !== cur) store.audioDevices = opts
      // saved device vanished from a good enumeration (unplugged/renamed):
      // fall back to System default instead of showing a raw id
      if (store.audioDeviceSelected !== "" && ids.split("\n").indexOf(store.audioDeviceSelected) === -1)
        root.setAudioDevice("")
    }
  }

  Process {
    id: ctlProc
    command: [root.sorakeyBin, "ctl", "{}"]
    onExited: function(exitCode) {
      // Process-level failure (daemon down, binary missing): background
      // polls stay silent (the status flow already resolves those to a
      // Start button); only explicit user commands toast.
      if (exitCode !== 0) {
        if (root.pendingCtlCmd !== "") {
          root.errorToast = root.pendingCtlCmd + ": daemon not responding."
          clearErrorToast.restart()
        }
        root.pendingCtlCmd = ""
        root.refreshStatus()
        return
      }
      root.refreshStatus()
      // parse the response once: substring matching ("ok":false / "deleted")
      // misses spaced JSON and misfires on error text containing the word
      var resp = null
      try { resp = JSON.parse(String(stdout.text || "").trim()) } catch(e) {}
      // show ctl failures the daemon reports
      if (resp && resp.ok === false) {
        root.errorToast = (root.pendingCtlCmd !== "" ? root.pendingCtlCmd + ": " : "") + String(resp.error || "command failed")
        clearErrorToast.restart()
      }
      root.pendingCtlCmd = ""
      if (resp && resp.deleted) {
        var delPretty = Model.prettyPackName(String(resp.deleted))
        var fb = resp.fallback ? String(resp.fallback) : ""
        if (fb) root.errorToast = "Deleted \"" + delPretty + "\" → \"" + Model.prettyPackName(fb) + "\""
        else root.errorToast = "Deleted \"" + delPretty + "\""
        clearErrorToast.restart()
      }
      // packsProc clears the deleting flags when the rescan lands
      if (root.deleting) root.refreshPacks()
    }
    stdout: StdioCollector { waitForEnd: true }
  }

  Process {
    id: svcProc
    command: ["true"]
    onExited: function(exitCode) {
      if (exitCode !== 0) {
        var detail = String(stderr.text || "").trim().split("\n").pop()
        root.errorToast = "Service command failed" + (detail !== "" ? ": " + detail : ".")
        clearErrorToast.restart()
      }
      root.refreshStatus()
    }
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
  }

  property var pendingSvcAction: []
  Process {
    id: stopFlagProc
    command: ["true"]
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      var args = root.pendingSvcAction
      root.pendingSvcAction = []
      if (exitCode !== 0) {
        var detail = String(stderr.text || "").trim().split("\n").pop()
        root.errorToast = "Stop flag update failed" + (detail !== "" ? ": " + detail : " — daemon action cancelled.")
        clearErrorToast.restart()
        return
      }
      if (args.length > 0) root.runService(args)
    }
  }

  // runs sora-uninstall.sh --purge in background (pkexec inside pops the GUI
  // approval for the rule removal, like the enable flow)
  Process {
    id: uninstallProc
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      var out = String(stdout.text || "").trim()
      var err = String(stderr.text || "").trim()
      if (exitCode === 0) root.finishRemove(true, "")
      else {
        var msg = err !== "" ? err.split("\n").pop() : (out !== "" ? out.split("\n").pop() : "Uninstall failed.")
        root.finishRemove(false, msg)
      }
    }
  }

  // runs sora-install in background
  Process {
    id: setupProc
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      root.setupBusy = false
      var out = String(stdout.text || "").trim()
      var err = String(stderr.text || "").trim()
      if (exitCode === 0) {
        store.noteInstalled(true)
        root.errorToast = ""
        store.setupRetries = 0
        // 3s Checking… starts now: covers daemon spawn + scan + packs.
        store.resetForInstall()
        root.refreshStatus()
        root.refreshPacks()
        root.refreshAudioDevices()
      } else {
        // Surface the failure instead of silently staying "Not installed".
        // No auto-retry here: installCheck's counter (max 3) already
        // re-arms it; manual Install taps stay unlimited.
        var msg = err !== "" ? err.split("\n").pop() : (out !== "" ? out.split("\n").pop() : "Install failed.")
        root.errorToast = root.shortText(msg)
        clearErrorToast.restart()
        installCheck.running = true
      }
    }
  }

  // one-tap keyboard-access enable (tailscale pkexec pattern): runs the
  // enable script, whose pkexec call pops the shell's GUI approval dialog.
  // Exit 0 = verified working, 2 = not approved (stay truthful + Retry),
  // 3 = no dialog on this box (offer the terminal route instead),
  // anything else = hard error shown in the result line.
  Process {
    id: captureProc
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      root.captureBusy = false
      root.capturePhase = ""
      capturePhaseTimer.stop()
      var out = String(stdout.text || "").trim()
      if (exitCode === 0) {
        root.captureStatus = "Keyboard sounds enabled."
        root.errorToast = ""
        // daemon lags the script by seconds (5s rescan): hold a finishing
        // state until status itself clears inputError (or the 15s cap).
        root.captureSettling = true
        root.capturePhase = "Finishing up…"
        captureSettleTimer.restart()
      } else if (exitCode === 2) {
        root.captureStatus = ""
        root.errorToast = ""
      } else if (exitCode === 3) {
        root.captureStatus = ""
        root.errorToast = ""
      } else {
        var err = String(stderr.text || "").trim()
        var msg = err !== "" ? err.split("\n").pop() : (out !== "" ? out.split("\n").pop() : "Could not enable — try again.")
        root.captureStatus = ""
        root.errorToast = root.shortText(msg)
        clearErrorToast.restart()
      }
      root.refreshStatus()
    }
  }

  // poll when open: status every second, packs every 30s (packs change only on
  // import/delete)
  Timer {
    interval: 1000
    repeat: true
    running: store.installed && root.opened
    onTriggered: root.refreshStatus()
  }
  Timer {
    interval: 30000
    repeat: true
    running: store.installed && root.opened
    onTriggered: root.refreshPacks()
  }
  // light poll when closed, 10s (open gets 1s + instant refresh on open)
  Timer {
    interval: 10000
    repeat: true
    running: store.installed && !root.opened
    onTriggered: root.refreshStatus()
  }

  // recheck binary for auto-setup
  Timer {
    interval: 5000
    repeat: true
    running: !store.installed && !setupBusy
    onTriggered: {
      installCheck.running = true
      root.refreshStatus()
    }
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    iconComponent: Component {
      Image {
        source: Qt.resolvedUrl(root.barLogoSource)
        sourceSize.width: 384
        anchors.fill: parent
        fillMode: Image.PreserveAspectFit
        smooth: true
        mipmap: true
        opacity: (store.running && store.muted) ? 0.5 : 1.0
      }
    }
    dimmed: !store.running
    active: store.running && store.muted
    tooltipText: "Sorakey — " + store.statusText + "\nRight-click: Mute\nCtrl+Alt+M: Global mute"
    onPressed: function(b) {
      if (b === Qt.RightButton) {
        if (store.running) root.setMuted(!store.muted)
      } else {
        root.toggle()
      }
    }
    onWheelMoved: function(delta) {
      if (!store.running) return
      var step = delta > 0 ? 5 : -5
      var cur = store.keyboardPack ? store.perPackVolume : store.volume
      var v = Math.max(0, Math.min(100, cur + step))
      if (store.keyboardPack) root.setPerPackVolume(v)
      else root.setVolume(v)
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    gap: Style.gapsOut
    contentWidth: panel.fittedContentWidth(Style.space(400))
    contentHeight: panel.fittedContentHeight(panelColumn.implicitHeight, Style.space(520))
    focusTarget: keyCatcher

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }

    ScrollView {
      id: scrollArea
      anchors.fill: parent
      clip: true
      ScrollBar.horizontal.policy: ScrollBar.AlwaysOff
      ScrollBar.vertical.policy: panelColumn.implicitHeight > height ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff

      Column {
        id: panelColumn
        width: scrollArea.availableWidth
        spacing: Style.space(12)

        // header
        Item {
          visible: !root.settingsOpen
          width: parent.width
          implicitHeight: Math.max(heroIcon.implicitHeight, heroLabels.implicitHeight, Math.max(muteSwitch.implicitHeight, settingsButton.implicitHeight))

          Image {
            id: heroIcon
            source: root.heroMatchTheme ? Qt.resolvedUrl("assets/icon-hero-dark.svg")
              : Qt.resolvedUrl(root.bar.foreground.hslLightness > 0.5 ? "assets/icon-hero-dark.svg" : "assets/icon-hero-light.svg")
            sourceSize.height: Style.font.display * 2.5
            fillMode: Image.PreserveAspectFit
            smooth: true
            mipmap: true
            opacity: store.muted ? 0.5 : 1.0
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            layer.enabled: true
            layer.effect: MultiEffect {
              colorization: 1.0
              colorizationColor: root.heroMatchTheme ? Color.accent : root.bar.foreground
            }
          }

          Column {
            id: heroLabels
            anchors.left: heroIcon.right
            anchors.leftMargin: Style.space(14)
            anchors.right: settingsButton.left
            anchors.rightMargin: Style.space(12)
            anchors.verticalCenter: parent.verticalCenter
            spacing: Style.space(2)

            Text {
              text: "Sorakey"
              color: root.heroMatchTheme ? Color.accent : root.bar.foreground
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.title
              font.bold: true
            }
            Row {
              spacing: Style.space(6)
              Rectangle {
                width: 8
                height: 8
                radius: 4
                anchors.verticalCenter: parent.verticalCenter
                color: (!store.installed || !store.running) ? Qt.darker(root.bar.foreground, 2.0)
                  : (store.muted ? Qt.darker(root.bar.foreground, 1.3)
                    : (root.heroMatchTheme ? Color.accent : root.bar.foreground))
              }
              Text {
                text: store.statusText
                color: root.heroMatchTheme ? Color.accent : root.bar.foreground
                opacity: 0.6
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.caption
                anchors.verticalCenter: parent.verticalCenter
              }
            }
          }

          ToggleSwitch {
            id: muteSwitch
            checked: store.muted
            enabled: store.running && store.inputError === ""
            rounded: root.roundedCorners
            foreground: root.bar.foreground
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            onToggled: root.setMuted(!store.muted)
            layer.enabled: root.heroMatchTheme
            layer.effect: MultiEffect {
              colorization: 1.0
              colorizationColor: Qt.lighter(Color.accent, 1.5)
            }
          }

          Button {
            id: settingsButton
            text: ""
            iconText: "󰒓"
            radius: root.friendlyRadius
            foreground: root.heroMatchTheme ? Color.accent : root.bar.foreground
            opacity: 0.8
            anchors.right: muteSwitch.left
            anchors.rightMargin: Style.space(8)
            anchors.verticalCenter: parent.verticalCenter
            tooltipText: "Settings"
            onClicked: root.settingsOpen = !root.settingsOpen
          
          }
        }

        // settings
        Column {
          visible: root.settingsOpen
          width: parent.width
          spacing: Style.space(8)
          Row {
            width: parent.width
            spacing: Style.space(8)
            Button {
              text: ""
              iconText: "←"
              radius: root.friendlyRadius
              foreground: root.bar.foreground
              tooltipText: "Back"
              onClicked: root.settingsOpen = false
            
            }
            Text {
              text: "Settings"
              color: root.bar.foreground
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.title
              font.bold: true
              anchors.verticalCenter: parent.verticalCenter
            }
          }
          PanelSeparator { foreground: root.bar.foreground }
          Column {
            width: parent.width
            spacing: Style.space(6)
            PanelSectionHeader { text: "GENERAL"; foreground: root.bar.foreground }
            Item {
              width: parent.width
              height: barSectionDrop.implicitHeight
SoraDropdown {
                id: barSectionDrop
                anchors.fill: parent
                value: root.currentBarSection
                roundedCorners: root.roundedCorners
                options: [{value:"left",label:"Left"},{value:"center",label:"Center"},{value:"right",label:"Right"}]
                foreground: Color.foreground
                popupBorder: Border.controlColor("normal", Color.foreground, Color.accent)
                rowHeight: Style.spacing.controlHeight + 8
                opacity: store.muted ? 0.5 : 1.0
                onChanged: function(v){ root.moveToSection(v) }
              }
              MouseArea {
                anchors.fill: parent
                hoverEnabled: false
                cursorShape: Qt.PointingHandCursor
                onPressed: function(mouse) {
                  if (barSectionDrop.popupOpen) barSectionDrop.close()
                  else if (Date.now() - root.barDropClosedAt > 300) barSectionDrop.open()
                  mouse.accepted = true
                }
              }
            }
            Item {
              width: parent.width
              height: Math.max(themeLabel.implicitHeight, themeSwitch.implicitHeight)
              Text {
                id: themeLabel
                text: "Match theme"
                color: root.bar.foreground
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.subtitle
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
              }
              ToggleSwitch {
                id: themeSwitch
                checked: root.heroMatchTheme
                rounded: root.roundedCorners
                foreground: root.bar.foreground
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                onToggled: root.setHeroMatchTheme(!root.heroMatchTheme)
              }
            }
            Item {
              width: parent.width
              height: Math.max(roundLabel.implicitHeight, roundSwitch.implicitHeight)
              Text {
                id: roundLabel
                text: "Rounded corners"
                color: root.bar.foreground
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.subtitle
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
              }
              ToggleSwitch {
                id: roundSwitch
                checked: root.roundedCorners
                rounded: root.roundedCorners
                foreground: root.bar.foreground
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                onToggled: root.setRoundedCorners(!root.roundedCorners)
              }
            }
            PanelSectionHeader { text: "AUDIO"; foreground: root.bar.foreground }
            Row {
              width: parent.width
              spacing: Style.space(8)
              Item {
                width: parent.width - rescanButton.width - parent.spacing
                height: audioDrop.implicitHeight
                SoraDropdown {
                  id: audioDrop
                  anchors.fill: parent
                  value: store.audioDeviceSelected
                  roundedCorners: root.roundedCorners
                  options: store.audioDevices
                  foreground: Color.foreground
                  popupBorder: Border.controlColor("normal", Color.foreground, Color.accent)
                  rowHeight: Style.spacing.controlHeight + 8
                  opacity: store.muted ? 0.5 : 1.0
                  onChanged: function(v){ root.setAudioDevice(v) }
                }
                MouseArea {
                  anchors.fill: parent
                  hoverEnabled: false
                  cursorShape: Qt.PointingHandCursor
                  onPressed: function(mouse) {
                    if (audioDrop.popupOpen) audioDrop.close()
                    else if (Date.now() - root.audioDropClosedAt > 300) audioDrop.open()
                    mouse.accepted = true
                  }
                }
              }
              Button {
                id: rescanButton
                text: "Rescan"
                radius: root.friendlyRadius
                verticalPadding: root.buttonYPadding
                foreground: root.bar.foreground
                selected: true
                tooltipText: "Rescan audio devices"
                onClicked: root.refreshAudioDevices()
              }
            }
            PanelSectionHeader { text: "SYSTEM"; foreground: root.bar.foreground }
            Row {
                width: parent.width - Style.space(24)
                anchors.horizontalCenter: parent.horizontalCenter
                spacing: Style.space(8)

              Button {
                text: root.exporting ? "Exporting…" : "Export Logs"
                iconText: root.exporting ? "󱑢" : ""
                radius: root.friendlyRadius
                iconSpinning: root.exporting
                foreground: root.bar.foreground
                selected: true
                width: (parent.width - Style.space(8)) / 2
                verticalPadding: root.buttonYPadding
                tooltipText: "Save a report of recent errors to a file"
                enabled: !root.exporting && store.installed
                onClicked: root.triggerExport()
              
              }

              Button {
                text: root.updateBusy ? "Updating…" : "Update"
                iconText: root.updateBusy ? "󰮭" : "󰮭"
                radius: root.friendlyRadius
                iconSpinning: root.updateBusy
                foreground: root.bar.foreground
                selected: true
                width: (parent.width - Style.space(8)) / 2
                verticalPadding: root.buttonYPadding
                tooltipText: "Update Sorakey plugin"
                enabled: !root.updateBusy
                onClicked: root.doUpdate()
              
              }
            }
            Text {
              visible: root.pluginVersion !== ""
              width: parent.width
              horizontalAlignment: Text.AlignHCenter
              text: root.pluginCommit !== "" ? "v" + root.pluginVersion + " · " + root.pluginCommit : "v" + root.pluginVersion
              color: root.bar.foreground
              opacity: 0.45
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
            }
            Text {
              visible: root.lastResult !== ""
              width: parent.width
              horizontalAlignment: Text.AlignHCenter
              text: root.lastResult
              color: root.bar.foreground
              opacity: 0.6
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
            }
            PanelSectionHeader { text: "DANGER"; foreground: root.bar.foreground }
            Button {
              text: root.uninstallBusy ? "Uninstalling…" : (root.uninstallArmed ? "Tap again to confirm" : "Uninstall Sorakey")
              iconText: "󰛌"
              radius: root.friendlyRadius
              selected: !root.uninstallArmed
              foreground: root.uninstallArmed ? "#ff6b6b" : root.bar.foreground
              bordered: root.uninstallArmed
              width: parent.width - Style.space(24)
              anchors.horizontalCenter: parent.horizontalCenter
              verticalPadding: root.buttonYPadding
              tooltipText: "Remove the plugin and stop the daemon"
              enabled: !setupBusy && !setupProc.running
              onClicked: {
                if (!root.uninstallArmed) { root.uninstallArmed = true; disarmUninstall.restart() }
                else { root.uninstallArmed = false; root.remove() }
              }
            
            }
          }
        }

        // install prompt
        Item {
          visible: !store.installed && !root.settingsOpen
          width: parent.width
          implicitHeight: installButton.implicitHeight

          Button {
            id: installButton
            text: setupBusy ? "Installing…" : "Install Sorakey"
            radius: root.friendlyRadius
            verticalPadding: root.buttonYPadding
              iconSpinning: setupBusy
            foreground: root.bar.foreground
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            enabled: !setupBusy && !root.uninstallBusy && !uninstallProc.running
            onClicked: root.install()
          
          }
        }

        // Checking placeholder: shown instead of guessing main vs
        // permission, so fresh installs never flash Image 1 before truth.
        // The installHold window keeps it up for a fixed 3s after install.
        Item {
          visible: store.installed && (store.installHold || !store.statusKnown) && !root.settingsOpen
          width: parent.width
          implicitHeight: checkingRow.implicitHeight
          Row {
            id: checkingRow
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: Style.space(8)
            Text {
              id: checkingSpinner
              text: "󰑐"
              color: root.bar.foreground
              opacity: 0.7
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.subtitle
              anchors.verticalCenter: parent.verticalCenter
              transformOrigin: Item.Center
              RotationAnimation on rotation {
                from: 0; to: 360; duration: 900; loops: Animation.Infinite; running: true
              }
            }
            Text {
              id: checkingText
              text: "Checking…"
              color: root.bar.foreground
              opacity: 0.85
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.subtitle
              font.bold: true
              anchors.verticalCenter: parent.verticalCenter
            }
          }
        }

        // controls — gated by knowledge AND hold: while Checking (hold or
        // unknown), only the placeholder above shows
        Column {
          visible: store.installed && !store.installHold && store.statusKnown && !root.settingsOpen
          width: parent.width
          spacing: Style.space(14)

          // health banner — the fix action is a button, never a command
          Column {
            visible: store.healthHint !== "" || store.showWhyBlock || root.captureBusy
            width: parent.width
            spacing: Style.space(6)
            PanelSectionHeader { text: "NEEDS ATTENTION"; foreground: root.bar.foreground }
            // blocked state: one plain line, one big button, one learn-more
            // link. Details live in docs/keyboard-access.md, not here.
            Column {
              visible: store.showWhyBlock
              width: parent.width
              spacing: Style.space(8)
              Text {
                width: parent.width
                text: "Sorakey: keyboard access needed"
                color: root.bar.foreground
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.subtitle
                font.bold: true
                wrapMode: Text.WordWrap
              }
              Text {
                width: parent.width
                text: "Sorakey listens for key presses to play sounds. One approval grants access to keyboards only."
                color: root.bar.foreground
                opacity: 0.8
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.caption
                wrapMode: Text.WordWrap
              }
              Column {
                visible: !root.captureWorking
                width: parent.width
                spacing: Style.space(8)
                Button {
                  width: parent.width
                  text: "Enable keyboard sounds"
                  iconText: ""
                  radius: root.friendlyRadius
                  foreground: root.bar.foreground
                  selected: true
                  fontSize: Style.font.subtitle
                  verticalPadding: Style.space(12)
                  enabled: !root.captureWorking
                  onClicked: root.enableCapture()
                }
                Button {
                  visible: store.inputError !== ""
                  width: parent.width
                  text: "Enable with terminal"
                  radius: root.friendlyRadius
                  foreground: root.bar.foreground
                  selected: true
                  verticalPadding: root.buttonYPadding
                  tooltipText: "Opens your terminal — approve there with sudo"
                  enabled: !root.captureWorking
                  onClicked: root.fixInTerminal()
                }
              }
              Column {
                visible: root.captureWorking
                width: parent.width
                spacing: Style.space(8)
                Row {
                  anchors.horizontalCenter: parent.horizontalCenter
                  spacing: Style.space(8)
                  Text {
                    text: "󰑐"
                    color: root.bar.foreground
                    opacity: 0.8
                    font.family: root.bar.fontFamily
                    font.pixelSize: Style.font.subtitle
                    anchors.verticalCenter: parent.verticalCenter
                    transformOrigin: Item.Center
                    RotationAnimation on rotation { from: 0; to: 360; duration: 900; loops: Animation.Infinite; running: true }
                  }
                  Text {
                    text: "Enabling…"
                    color: root.bar.foreground
                    font.family: root.bar.fontFamily
                    font.pixelSize: Style.font.subtitle
                    font.bold: true
                    anchors.verticalCenter: parent.verticalCenter
                  }
                }
                Text {
                  visible: root.capturePhase !== ""
                  width: parent.width
                  horizontalAlignment: Text.AlignHCenter
                  text: root.capturePhase
                  color: root.bar.foreground
                  opacity: 0.7
                  font.family: root.bar.fontFamily
                  font.pixelSize: Style.font.caption
                }
              }
              Text {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: "Learn more →"
                color: root.bar.foreground
                opacity: 0.7
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.caption
                font.underline: true
                MouseArea {
                  anchors.fill: parent
                  cursorShape: Qt.PointingHandCursor
                  onClicked: Quickshell.execDetached(["xdg-open", root.whyLearnMoreUrl])
                }
              }
            }
            Text {
              visible: store.healthHint !== ""
              width: parent.width
              text: store.healthHint
              color: "#ff6b6b"
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
            }
            Row {
              visible: !store.running
              width: parent.width
              spacing: Style.space(8)
              Button {
                width: parent.width
                text: "Start"
                radius: root.friendlyRadius
                verticalPadding: root.buttonYPadding
                foreground: root.bar.foreground
                selected: true
                onClicked: root.startDaemon()
              }
            }
            PanelSeparator { foreground: root.bar.foreground }
          }

          PanelSeparator { visible: store.captureReady; foreground: root.bar.foreground }

          // keyboard volume — per pack
          Column {
            visible: store.captureReady
            width: parent.width
            spacing: Style.space(6)

            Row {
              width: parent.width
              PanelSectionHeader {
                text: "KEYBOARD VOLUME: "
                foreground: root.bar.foreground
                anchors.verticalCenter: parent.verticalCenter
              }
              Item { width: 1 }
              Item {
                  implicitWidth: volLabel.implicitWidth
                  implicitHeight: Style.spacing.controlHeight
                  Text {
                    id: volLabel
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    text: Math.round(store.perPackVolume) + "%"
                    color: root.bar.foreground
                    opacity: volHover.containsMouse ? 1.0 : (store.keyboardPack !== "" ? 0.85 : 0.6)
                    font.family: root.bar.fontFamily
                    font.pixelSize: Style.font.body
                    font.bold: true
                  }
                  MouseArea {
                    id: volHover
                    anchors.fill: parent
                    anchors.margins: -Style.space(4)
                    visible: store.keyboardPack !== ""
                    cursorShape: Qt.PointingHandCursor
                    hoverEnabled: true
                    ToolTip.text: "Reset to pack default"
                    ToolTip.visible: containsMouse
                    ToolTip.delay: 400
                    onClicked: root.resetVolume()
                  }
                }
             }

            Item {
              width: parent.width
              implicitHeight: Style.spacing.controlHeight
              PanelSlider {
                id: kbSlider
                bar: root.bar
                anchors.fill: parent
                minimum: 0
                maximum: 100
                integer: true
                value: store.perPackVolume
                enabled: store.running && store.keyboardPack !== ""
                onReleased: root.setPerPackVolume(liveValue)
              }
            }
          }

          // Soundpacks
          PanelSeparator { visible: store.captureReady; foreground: root.bar.foreground }

          Column {
            visible: store.captureReady
            width: parent.width
            spacing: Style.space(8)

            PanelSectionHeader { text: "SOUNDPACKS"; foreground: root.bar.foreground }

            Text {
              visible: store.keyboardPack === "" && store.keyboardPacks.length === 0
              width: parent.width
              text: "Import Sound to get started"
              color: root.bar.foreground
              opacity: 0.6
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
            }
            Button {
              visible: store.keyboardPack === "" && store.keyboardPacks.length === 0
              text: root.importing ? "Importing…" : "Import Sound"
              radius: root.friendlyRadius
              verticalPadding: root.buttonYPadding
              foreground: root.bar.foreground
              selected: true
              enabled: !root.importing
              onClicked: root.triggerImport()
            }

            Row {
              width: parent.width
              spacing: Style.space(8)
              SoraPackPicker {
                id: kbPack
                width: parent.width
                value: store.keyboardPack
                roundedCorners: root.roundedCorners
                options: Model.packOptions(store.keyboardPacks)
                foreground: Color.foreground
                popupBorder: Border.controlColor("normal", Color.foreground, Color.accent)
                                opacity: store.muted ? 0.5 : 1.0
                rowHeight: Style.spacing.controlHeight + 8
                placeholderText: "Search packs…"
                deleteConfirmId: root.deleteConfirmId
                deleting: root.deleting
                toast: root.errorToast
                onChanged: function(v) { root.setKeyboardPack(v) }
                onDeleteRequested: function(v) { root.deleteConfirmId = v }
                onConfirmDelete: function(v) { root.deletePack(v) }
                onCancelDelete: function() { root.deleteConfirmId = "" }
              }
                          }

            
          }

            Row {
              visible: store.captureReady
              width: parent.width
              spacing: Style.space(8)
              Button {
                id: importButton
                width: (parent.width - Style.space(8) * 2 - 1) / 2
                text: root.importing ? "Importing…" : "Import Sound"
                radius: root.friendlyRadius
                verticalPadding: root.buttonYPadding
                foreground: root.bar.foreground
                opacity: root.importing ? 0.5 : 1.0
                enabled: !root.importing
                onClicked: root.triggerImport()

              }
              Rectangle {
                width: 1
                height: importButton.height
                anchors.verticalCenter: parent.verticalCenter
                color: Qt.rgba(root.bar.foreground.r, root.bar.foreground.g, root.bar.foreground.b, 0.12)
              }
              Button {
                id: openFolderButton
                width: (parent.width - Style.space(8) * 2 - 1) / 2
                text: "Open folder"
                radius: root.friendlyRadius
                verticalPadding: root.buttonYPadding
                foreground: root.bar.foreground
                opacity: 0.7
                onClicked: root.openCustomFolder()

              }
            }

          Row {
            visible: store.captureReady
            width: parent.width
            spacing: Style.space(8)
            Button {
                id: transportStop
                width: (parent.width - Style.space(16)) / 3
              text: store.running ? "Stop" : "Start"
              radius: root.friendlyRadius
              verticalPadding: root.buttonYPadding
              foreground: root.bar.foreground
              selected: true
              onClicked: store.running ? root.stopDaemon() : root.startDaemon()
            }
            Button {
                id: transportRestart
                width: (parent.width - Style.space(16)) / 3
              text: "Restart"
              radius: root.friendlyRadius
              verticalPadding: root.buttonYPadding
              foreground: root.bar.foreground
              selected: true
              tooltipText: "Restart sorakey"
              onClicked: root.restartDaemon()
            }
            Button {
                id: transportShuffle
                width: (parent.width - Style.space(16)) / 3
              text: "Random"
              radius: root.friendlyRadius
              verticalPadding: root.buttonYPadding
              foreground: root.bar.foreground
              selected: true
              enabled: store.running && store.keyboardPacks.length > 1
              onClicked: root.pickRandomPack()
            }
          }

          PanelSeparator { visible: store.captureReady; foreground: root.bar.foreground }

          // typing test — the daemon listens system-wide, so physical
          // keystrokes while the panel is open play through this box
          Column {
            visible: store.captureReady
            width: parent.width
            spacing: Style.space(6)
            PanelSectionHeader { text: "TEST TYPING"; foreground: root.bar.foreground }
            SoraTextField {
              id: testType
              foreground: root.bar.foreground
              roundedCorners: root.roundedCorners
              width: parent.width
              height: 56
              text: ""
              placeholderText: "Click here and type — hear keys"
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
            }
          }

        }
      }
    }
    }
  }
}