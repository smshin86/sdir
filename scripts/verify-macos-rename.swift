// Compiled with the accessibility and native input helpers from verify-macos-ui.swift.
func requireCondition(_ condition: Bool, _ message: String) throws {
    guard condition else { throw JourneyFailure(message) }
}

func mustWait(_ description: String, seconds: TimeInterval = 6, _ predicate: () -> Bool) throws {
    try requireCondition(waitFor(seconds) { predicate() ? true : nil } == true, description)
}

func click(_ point: CGPoint) {
    mouse(.mouseMoved, point)
    mouse(.leftMouseDown, point)
    mouse(.leftMouseUp, point)
}

func typeText(_ text: String) {
    var units = Array(text.utf16)
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
        event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: &units)
        event.post(tap: .cghidEventTap)
    }
}

func focusedRename(_ ax: AXUIElement) -> AXUIElement? {
    guard let field = asElement(attribute(ax, kAXFocusedUIElementAttribute)),
          hasText(field, "파일 또는 폴더 이름") else { return nil }
    return field
}

func row(_ ax: AXUIElement, _ name: String) -> AXUIElement? {
    window(ax).flatMap { win in
        searchAX(win) { element in hasText(element, name) && frame(element) != nil }
    }
}

func editValue(_ ax: AXUIElement) -> String? {
    focusedRename(ax).flatMap { attribute($0, kAXValueAttribute) as? String }
}

func selectedRange(_ ax: AXUIElement) -> CFRange? {
    guard let field = focusedRename(ax), let value = axValue(field, kAXSelectedTextRangeAttribute) else { return nil }
    var range = CFRange(location: 0, length: 0)
    guard AXValueGetValue(value, .cfRange, &range) else { return nil }
    return range
}

func pick(_ app: NSRunningApplication, _ ax: AXUIElement, _ filename: String) throws {
    guard let location = waitFor(6, { row(ax, filename).flatMap(frame) }),
          NSWorkspace.shared.frontmostApplication?.processIdentifier == app.processIdentifier,
          CGDisplayBounds(CGMainDisplayID()).contains(CGPoint(x: location.midX, y: location.midY)) else {
        throw JourneyFailure("Fixture row is not in the active app: \(filename)")
    }
    click(CGPoint(x: location.midX, y: location.midY))
}

func putName(_ ax: AXUIElement, _ name: String) throws {
    key(0, flags: [.maskCommand])
    typeText(name)
    try mustWait("Field did not receive \(name); observed \(editValue(ax) ?? "<none>")") {
        editValue(ax) == name
    }
}

func verifyRename(_ app: NSRunningApplication, _ ax: AXUIElement, _ folder: URL) throws {
    let manager = FileManager.default
    try mustWait("Seed file never appeared in the opened folder", seconds: 14) {
        row(ax, "readiness-fixture.txt") != nil
    }
    try manager.removeItem(at: folder.appendingPathComponent("readiness-fixture.txt"))
    let modes = ["return", "list_click", "refresh_delete", "escape", "collision_retry", "refresh_create", "target_removed", "unicode"]
    for index in 0..<80 {
        let mode = modes[index % modes.count]
        let id = String(format: "%03d", index)
        let preceding = id + "-a.txt"
        let source = id + "-b.txt"
        let other = id + "-c.txt"
        let target = id + (mode == "unicode" ? "-한글.txt" : "-new.txt")
        let oldPath = folder.appendingPathComponent(source)
        let targetPath = folder.appendingPathComponent(target)
        let firstPath = folder.appendingPathComponent(preceding)
        let otherPath = folder.appendingPathComponent(other)
        let payload = Data(("SDIR fixture " + id).utf8)
        try Data("preceding".utf8).write(to: firstPath)
        try mustWait("Preceding row did not appear for \(id)") { row(ax, preceding) != nil }
        try payload.write(to: oldPath)
        try Data("other".utf8).write(to: otherPath)
        try pick(app, ax, source)
        key(36)
        try mustWait("Return did not open the editor for \(source)") { editValue(ax) == source }
        if mode == "collision_retry" {
            try putName(ax, other)
            key(36)
            try mustWait("Collision feedback did not preserve the draft") {
                editValue(ax) == other && window(ax).flatMap { win in
                    searchAX(win) { hasText($0, "같은 이름") || hasText($0, "already exists") || hasText($0, "작업을 완료하지 못했습니다") }
                } != nil
            }
            try requireCondition(try Data(contentsOf: oldPath) == payload, "Collision changed source bytes")
            try requireCondition(try Data(contentsOf: otherPath) == Data("other".utf8), "Collision changed destination")
        }
        try putName(ax, target)
        if mode == "refresh_delete" || mode == "refresh_create" {
            key(123)
            guard let cursor = selectedRange(ax) else { throw JourneyFailure("AX did not expose the edit caret") }
            if mode == "refresh_delete" {
                try manager.removeItem(at: firstPath)
                try mustWait("Preceding row stayed after its external removal") { row(ax, preceding) == nil }
            } else {
                let external = id + "-external.txt"
                try Data("external".utf8).write(to: folder.appendingPathComponent(external))
                try mustWait("External file did not appear during editing") { row(ax, external) != nil }
            }
            try requireCondition(editValue(ax) == target && selectedRange(ax)?.location == cursor.location
                                 && selectedRange(ax)?.length == cursor.length, "Refresh changed the draft, caret or focus")
        }
        switch mode {
        case "escape":
            key(53)
            try mustWait("Escape left the editor open") { focusedRename(ax) == nil }
            try requireCondition(try Data(contentsOf: oldPath) == payload && !manager.fileExists(atPath: targetPath.path),
                                 "Escape unexpectedly changed the file")
        case "target_removed":
            let moved = folder.appendingPathComponent(id + "-external.txt")
            try manager.moveItem(at: oldPath, to: moved)
            try mustWait("Externally moved target still appears editable") {
                row(ax, moved.lastPathComponent) != nil && focusedRename(ax) == nil
            }
            try requireCondition(try Data(contentsOf: moved) == payload && !manager.fileExists(atPath: targetPath.path),
                                 "External change lost file bytes or applied the stale rename")
        default:
            if mode == "list_click" { try pick(app, ax, other) } else { key(36) }
            try mustWait("Rename did not complete for \(id) (\(mode)); draft \(editValue(ax) ?? "<none>")") {
                !manager.fileExists(atPath: oldPath.path) && (try? Data(contentsOf: targetPath)) == payload
                    && focusedRename(ax) == nil && row(ax, target) != nil
            }
            try requireCondition(try Data(contentsOf: otherPath) == Data("other".utf8), "Unrelated file was modified")
        }
        emit("rename_\(id)", "passed", mode)
        for item in try manager.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil) {
            try manager.removeItem(at: item)
        }
        try mustWait("An old fixture row lingered: \(id)") { row(ax, other) == nil }
    }
}

let arguments = CommandLine.arguments
guard arguments.count == 4,
      let pid = pid_t(arguments[1]),
      waitFor(20, { findWindowServerFrame(pid) }) != nil,
      let app = NSRunningApplication(processIdentifier: pid),
      AXIsProcessTrusted(), CGPreflightPostEventAccess(), activate(app) else {
    emit("window_launch", "blocked", "Visible SDIR window or native event permissions unavailable")
    exit(1)
}
let ax = AXUIElementCreateApplication(pid)
let folder = URL(fileURLWithPath: arguments[2])
let finderSource = URL(fileURLWithPath: arguments[3])
do {
    try verifyRename(app, ax, folder)
    try testFinderDrop(app, ax, finderSource, folder)
} catch {
    emit("rename_or_finder", "failed", String(describing: error))
    exit(1)
}
if activate(app) { key(12, flags: [.maskCommand]) }
