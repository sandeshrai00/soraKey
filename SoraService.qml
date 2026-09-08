import QtQuick
import Quickshell
import Quickshell.Io

Item {
  id: root

  visible: false
  width: 0
  height: 0

  property var shell: null
  property var manifest: null
  readonly property string pluginDir: manifest && manifest.__sourceDir
    ? String(manifest.__sourceDir)
    : Quickshell.env("HOME") + "/.config/omarchy/plugins/io.github.sandeshrai00.sorakey"

  readonly property string pluginId: manifest && manifest.id
    ? String(manifest.id) : "io.github.sandeshrai00.sorakey"

  property bool importing: false
  property string lastImportResult: ""
  property string lastImportError: ""
  property bool exporting: false
  property string lastExportResult: ""
  property string lastExportError: ""
  property string lastSyncResult: ""
  // sticky stop: true if the user explicitly stopped the daemon (Panel writes the flag).
  // NOTE: Qt has no fileExists() — the previous readonly binding silently
  // evaluated false forever, auto-starting the daemon after every user Stop.
  // The flag is read once here at startup (the only moment it gates anything).
  property bool stoppedFlag: false

  signal packsImported(string packId)
  function notify(title, msg) { Quickshell.execDetached(["notify-send","-a","Sorakey", title, msg]); clearImportTimer.restart() }

  // Detached pickers (reload-proof): the file dialog used to run as a
  // direct child of this service, and every plugin reload ("Local plugin
  // changed, reloading" — dev-sync, updates, editor saves) SIGTERMs
  // direct children, murdering the picker mid-dialog with no trace.
  // Now the picker is double-forked (scripts/sorakey-detached) and
  // reports via ~/.cache/sorakey/<kind>-result, which this service polls.
  // <kind>-result.open means "dialog may still be open", so a restarted
  // service resumes polling instead of losing the result.
  readonly property string pickCacheDir: Quickshell.env("HOME") + "/.cache/sorakey"
  property string pickKind: "" // "import" | "export" | "" (idle)
  property int pickTicks: 0    // 1s polls; 300 = 5 min timeout
  function pickResultFile(kind) { return root.pickCacheDir + "/" + kind + "-result" }

  function startPick(kind, script) {
    if (root.pickKind !== "") return
    if (!pluginDir) {
      if (kind === "import") root.lastImportError = "Service pluginDir is empty (manifest not injected)"
      else root.lastExportError = "Service pluginDir is empty"
      return
    }
    if (kind === "import") {
      if (root.importing) return
      root.importing = true
      root.lastImportError = ""
      root.lastImportResult = ""
    } else {
      if (root.exporting) return
      root.exporting = true
      root.lastExportError = ""
      root.lastExportResult = ""
    }
    var result = root.pickResultFile(kind)
    root.pickKind = kind
    root.pickTicks = 0
    Quickshell.execDetached(["/usr/bin/bash", root.pluginDir + "/scripts/sorakey-detached",
      result, "/usr/bin/env", "GTK_USE_PORTAL=0", "/usr/bin/python3",
      root.pluginDir + "/scripts/" + script, "--result-file", result])
    pickTimer.restart()
  }

  function importSoundpack() { root.startPick("import", "sora-pack-import.py") }
  function exportLogs() { root.startPick("export", "sora-export-logs.py") }

  function handleImportLine(last) {
    if (last.startsWith("OK:")) {
      root.lastImportResult = last.substring(3).trim()
      root.lastImportError = ""
      root.packsImported(root.lastImportResult)
      root.notify("Soundpack imported", root.lastImportResult)
    } else if (last.startsWith("ERROR:")) {
      var msg = last.substring(6).trim()
      if (msg === "Cancelled" || msg.toLowerCase().indexOf("cancel") !== -1) {
        root.lastImportError = ""
        root.lastImportResult = ""
        return
      }
      root.lastImportError = msg
      root.lastImportResult = ""
      root.notify("Import failed", msg)
    } else {
      root.lastImportError = "Import failed — try again."
      root.lastImportResult = ""
      root.notify("Import failed", root.lastImportError)
    }
  }

  function handleExportLine(last) {
    if (last.startsWith("OK:")) {
      root.lastExportResult = last.substring(3).trim()
      root.lastExportError = ""
      root.notify("Logs exported", root.lastExportResult)
    } else if (last.startsWith("ERROR:")) {
      var msg = last.substring(6).trim()
      if (msg === "Cancelled" || msg.toLowerCase().indexOf("cancel") !== -1) {
        root.lastExportError = ""
        root.lastExportResult = ""
        return
      }
      root.lastExportError = msg
      root.lastExportResult = ""
      root.notify("Export failed", msg)
    } else {
      root.lastExportError = "Export failed — try again."
      root.lastExportResult = ""
      root.notify("Export failed", root.lastExportError)
    }
  }

  function finishPick() {
    var kind = root.pickKind
    root.pickKind = ""
    root.importing = false
    root.exporting = false
    pickTimer.stop()
    if (kind !== "") Quickshell.execDetached(["/usr/bin/rm", "-f",
      root.pickResultFile(kind), root.pickResultFile(kind) + ".open",
      root.pickResultFile(kind) + ".pid"])
  }

  function pickTimeout() {
    if (root.pickKind === "import") {
      root.lastImportError = "Picker timed out — try again."
      root.lastImportResult = ""
      root.notify("Import failed", root.lastImportError)
    } else if (root.pickKind === "export") {
      root.lastExportError = "Picker timed out — try again."
      root.lastExportResult = ""
      root.notify("Export failed", root.lastExportError)
    }
    root.finishPick()
  }

  Timer { id: clearImportTimer; interval: 10000; onTriggered: { root.lastImportResult = ""; root.lastExportResult = "" } }

  Timer {
    id: pickTimer
    interval: 1000
    repeat: true
    running: false
    onTriggered: {
      if (root.pickKind === "" || pickRead.running) return
      root.pickTicks += 1
      if (root.pickTicks > 300) { root.pickTimeout(); return }
      // WAITING = dialog still open; DEAD = picker process gone with no
      // result (crash/kill) — fail fast instead of idling to the timeout.
      pickRead.command = ["/usr/bin/bash", "-c",
        'if [ -s "$1" ]; then cat "$1";' +
        ' elif [ -f "$2" ] && kill -0 "$(cat "$2")" 2>/dev/null; then echo WAITING;' +
        ' else echo DEAD; fi',
        "_", root.pickResultFile(root.pickKind), root.pickResultFile(root.pickKind) + ".pid"]
      pickRead.running = true
    }
  }

  Process {
    id: pickRead
    stdout: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      var lines = String(stdout.text || "").trim().split("\n")
      var line = lines[lines.length - 1]
      if (line === "" || line === "WAITING") return
      // A failed poll script proves nothing about the dialog, but its
      // output is untrustworthy — treat exactly like DEAD below.
      if (line === "DEAD" || exitCode !== 0) {
        if (root.pickKind === "import") {
          root.lastImportError = "Picker closed unexpectedly — try again."
          root.lastImportResult = ""
          root.notify("Import failed", root.lastImportError)
        } else if (root.pickKind === "export") {
          root.lastExportError = "Picker closed unexpectedly — try again."
          root.lastExportResult = ""
          root.notify("Export failed", root.lastExportError)
        } else return
        root.finishPick()
        return
      }
      if (root.pickKind === "import") root.handleImportLine(line)
      else if (root.pickKind === "export") root.handleExportLine(line)
      else return
      root.finishPick()
    }
  }

  // enable daemon when plugin is on — mirrors onDestruction teardown
  Process {
    id: startProc
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
  }

  // one-shot sticky-stop read: gates the auto-start below.
  Process {
    id: stopFlagRead
    stdout: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      root.stoppedFlag = (exitCode === 0)
      if (!startProc.running && !root.stoppedFlag) {
        startProc.command = ["/usr/bin/bash", "-c",
          'test -x "$HOME/.local/bin/sorakey" && test -f "$HOME/.config/systemd/user/sorakey.service" && systemctl --user enable --now sorakey || exit 0']
        startProc.running = true
      }
      freshnessCheck.running = true
    }
  }

  Component.onCompleted: {
    // sticky stop: only auto-start if the user hasn't explicitly stopped the daemon.
    // Guard: on first enable the binary/unit don't exist yet (Panel auto-install
    // creates them) — enabling a missing unit only spams a failure, so skip it.
    // The flag read completes first (stopFlagRead.onExited starts startProc).
    stopFlagRead.command = ["/usr/bin/test", "-f",
      Quickshell.env("HOME") + "/.local/share/sorakey/stopped"]
    stopFlagRead.running = true
    // resume a picker left in flight by a plugin reload: the dialog runs
    // detached and survives, so keep polling for its result file instead
    // of stranding it (markers older than 10 min are stale crashes).
    resumePoll.command = ["/usr/bin/bash", "-c",
      'for k in import export; do m="$1/$k-result.open"; r="$1/$k-result";' +
      ' if [ -f "$m" ]; then' +
      ' if [ -n "$(find "$m" -mmin +10 2>/dev/null)" ]; then rm -f "$m" "$r" "$r.pid";' +
      ' else echo "$k"; fi; fi; done',
      "_", root.pickCacheDir]
    resumePoll.running = true
  }

  Process {
    id: resumePoll
    stdout: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      // A failed resume scan resumes nothing: never adopt a kind from
      // garbage output.
      if (exitCode !== 0) return
      var kind = String(stdout.text || "").trim().split("\n").pop()
      if (kind !== "import" && kind !== "export") return
      if (kind === "import" && !root.importing) root.importing = true
      if (kind === "export" && !root.exporting) root.exporting = true
      root.pickKind = kind
      root.pickTicks = 0
      pickTimer.restart()
    }
  }

  // after update, rebuild or fetch prebuilt and restart if needed
  Process {
    id: freshnessCheck
    command: ["/usr/bin/bash", root.pluginDir + "/scripts/sora-build.sh"]
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode !== 0) return
      var out = String(stdout.text || "").trim()
      var lines = out.split("\n")
      var line = lines[lines.length - 1]
      console.info("sorakey freshness: " + line)
      if (line.indexOf("up to date") !== -1) return
      // pack sync piggybacks the freshness run: same toast pattern as
      // import/export (scripts print, QML notifies — scripts run headless).
      if (line.indexOf("soundpacks synced") !== -1) {
        root.lastSyncResult = line
        root.notify("Soundpacks updated", line)
      }
      Quickshell.execDetached(["systemctl", "--user", "restart", "sorakey"])
    }
  }

  Component.onDestruction: {
    // only shell instance owns daemon lifecycle — panel copies come and go
    if (!root.shell) return
    // stop on disable/remove/reload
    Quickshell.execDetached(["systemctl", "--user", "stop", "sorakey"])
    Quickshell.execDetached(["systemctl", "--user", "disable", "sorakey"])
    // exact-path match (the daemon's cmdline is its ExecStart): -x would hit
    // any other process that happens to be named "sorakey"; the daemon writes
    // no PID file so there is nothing tighter than this
    Quickshell.execDetached(["pkill", "-f", Quickshell.env("HOME") + "/.local/bin/sorakey"])
  }
}
