// helpers for bar + panel

// "keyboard/sugar65" -> "Sugar65" (hyphens/underscores become spaces,
// each word capitalized; glued vendor prefixes like "cherrymx" split first
// so user-imported packs with such names still read correctly)
function prettyPackName(id) {
  var s = String(id || "")
  var slash = s.lastIndexOf("/")
  if (slash >= 0) s = s.slice(slash + 1)
  s = s.replace(/[-_]+/g, " ")
  // known glued prefixes read wrong capitalized ("Cherrymx") — split them
  s = s.replace(/^cherrymx\b/i, "cherry mx").replace(/^gateron\b/i, "gateron ")
  s = s.replace(/\b\w/g, function (c) { return c.toUpperCase() })
  return s.replace(/\s+/g, " ").trim()
}

// ids -> [{value, label}] for Dropdown
function packOptions(ids) {
  var out = []
  if (!Array.isArray(ids)) return out
  for (var i = 0; i < ids.length; i++) {
    var id = String(ids[i] || "")
    if (id === "") continue
    out.push({ value: id, label: prettyPackName(id) })
  }
  return out
}

// parse `sorakey ctl status` — null on failure
function parseStatus(text) {
  try {
    var o = JSON.parse(String(text || "").trim())
    return (o && typeof o === "object") ? o : null
  } catch (e) {
    return null
  }
}

// parse `sorakey ctl packs`
function parsePacks(text) {
  var empty = { keyboard: [] }
  try {
    var o = JSON.parse(String(text || "").trim())
    if (!o || typeof o !== "object") return empty
    var kb = Array.isArray(o.keyboard) ? o.keyboard.filter(function(v){ return typeof v==="string" && v.length>0 }) : []
    return { keyboard: kb }
  } catch (e) {
    return empty
  }
}