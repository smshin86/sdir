// Compiled with the accessibility and native input helpers from verify-macos-ui.swift.
func requireCondition(_ condition: Bool, _ message: String) throws {
    guard condition else { throw JourneyFailure(message) }
}

func mustWait(_ description: String, seconds: TimeInterval = 6, _ predicate: () -> Bool) throws {
    try requireCondition(waitFor(seconds) { predicate() ? true : nil } == true, description)
}

func row(_ ax: AXUIElement, _ name: String) -> AXUIElement? {
    window(ax).flatMap { win in
        searchAX(win) { element in hasText(element, name) && frame(element) != nil }
    }
}

func ancestors(_ node: AXUIElement) -> [AXUIElement] {
    var chain = [node]
    for _ in 0..<12 {
        guard let parent = asElement(attribute(chain.last!, kAXParentAttribute)) else { break }
        chain.append(parent)
    }
    return chain
}

func fixtureSelected(_ ax: AXUIElement, _ filename: String) -> Bool {
    guard let node = row(ax, filename) else { return false }
    return ancestors(node).contains { item in
        if attribute(item, kAXSelectedAttribute) as? Bool == true && hasText(item, filename) { return true }
        let items = attribute(item, kAXSelectedChildrenAttribute) as? [AXUIElement] ?? []
        return items.count == 1 && (hasText(items[0], filename) || searchAX(items[0]) { hasText($0, filename) } != nil)
    }
}

func pick(_ app: NSRunningApplication, _ ax: AXUIElement, _ filename: String) throws {
    guard let location = waitFor(6, { row(ax, filename).flatMap(frame) }),
          NSWorkspace.shared.frontmostApplication?.processIdentifier == app.processIdentifier else {
        throw JourneyFailure("Fixture row is not in the active app: \(filename)")
    }
    let point = CGPoint(x: location.midX, y: location.midY)
    var hit: AXUIElement?
    let result = AXUIElementCopyElementAtPosition(AXUIElementCreateSystemWide(), Float(point.x), Float(point.y), &hit)
    guard CGDisplayBounds(CGMainDisplayID()).contains(point), result == .success, let hit else {
        throw JourneyFailure("Accessibility could not identify the row click point for \(filename)")
    }
    var pid: pid_t = 0
    AXUIElementGetPid(hit, &pid)
    guard pid == app.processIdentifier, ancestors(hit).contains(where: { hasText($0, filename) }) else {
        throw JourneyFailure("Click point for \(filename) resolved to \(role(hit)), owner \(pid), expected \(app.processIdentifier)")
    }
    click(location)
    try mustWait("Click did not select \(filename)") {
        fixtureSelected(ax, filename)
    }
}

func typeStem(_ stem: String, in app: NSRunningApplication) throws {
    try requireCondition(NSWorkspace.shared.frontmostApplication?.processIdentifier == app.processIdentifier,
                         "The owned SDIR window lost foreground before text input")
    let inputCodes: [Character: CGKeyCode] = [
        "0": 29, "1": 18, "2": 19, "3": 20, "4": 21, "5": 23, "6": 22, "7": 26, "8": 28, "9": 25,
        "-": 27, "n": 45, "e": 14, "w": 13, "c": 8,
    ]
    for character in stem {
        guard let code = inputCodes[character] else { throw JourneyFailure("Invalid fixture character") }
        key(code)
    }
}

func captureWindow(pid: pid_t, output: String) {
    if let records = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]],
       let id = records.first(where: { ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid
           && ($0[kCGWindowLayer as String] as? NSNumber)?.intValue == 0 })?[kCGWindowNumber as String] as? NSNumber {
        let capture = Process()
        capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        capture.arguments = ["-x", "-l", id.stringValue, output]
        try? capture.run()
        if capture.isRunning { capture.waitUntilExit() }
    }
}

func verifyRename(_ app: NSRunningApplication, _ ax: AXUIElement, _ folder: URL, _ capture: String) throws {
    let manager = FileManager.default
    try mustWait("Seed file never appeared in the opened folder", seconds: 14) {
        row(ax, "readiness-fixture.txt") != nil
    }
    try manager.removeItem(at: folder.appendingPathComponent("readiness-fixture.txt"))
    let modes = ["return", "list_click", "refresh_delete", "escape", "collision_retry", "refresh_create", "target_removed"]
    for index in 0..<84 {
        let mode = modes[index % modes.count]
        let id = String(format: "%03d", index)
        let preceding = id + "-a.txt"
        let source = id + "-b.txt"
        let other = id + "-c.txt"
        let stem = id + "-new"
        let target = stem + ".txt"
        let oldPath = folder.appendingPathComponent(source)
        let checksCaret = mode == "refresh_delete" || mode == "refresh_create"
        let expectedName = checksCaret ? String(stem.dropLast()) + "x" + String(stem.suffix(1)) + ".txt" : target
        let targetPath = folder.appendingPathComponent(expectedName)
        let firstPath = folder.appendingPathComponent(preceding)
        let otherPath = folder.appendingPathComponent(other)
        let payload = Data(("SDIR fixture " + id).utf8)
        try Data("preceding".utf8).write(to: firstPath)
        try mustWait("Preceding row did not appear for \(id)") { row(ax, preceding) != nil }
        try payload.write(to: oldPath)
        try Data("other".utf8).write(to: otherPath)
        try pick(app, ax, source)
        key(36)
        if mode == "collision_retry" {
            let collisionStem = id + "-c"
            try typeStem(collisionStem, in: app)
            key(36)
            try mustWait("Collision feedback did not preserve the draft") {
                window(ax).flatMap { win in
                    searchAX(win) { hasText($0, "같은 이름") || hasText($0, "already exists") || hasText($0, "작업을 완료하지 못했습니다") }
                } != nil
            }
            try requireCondition(try Data(contentsOf: oldPath) == payload, "Collision changed source bytes")
            try requireCondition(try Data(contentsOf: otherPath) == Data("other".utf8), "Collision changed destination")
            for _ in collisionStem { key(123, flags: [.maskShift]) }
        }
        try typeStem(stem, in: app)
        if index == 0 { captureWindow(pid: app.processIdentifier, output: capture) }
        if checksCaret {
            key(123)
            if mode == "refresh_delete" {
                try manager.removeItem(at: firstPath)
                try mustWait("Preceding row stayed after its external removal") { row(ax, preceding) == nil }
            } else {
                let external = id + "-external.txt"
                try Data("external".utf8).write(to: folder.appendingPathComponent(external))
                try mustWait("External file did not appear during editing") { row(ax, external) != nil }
            }
            // Keep the keyboard on the same editor. The resulting path proves the insertion position.
            key(7)
        }
        switch mode {
        case "escape":
            key(53)
            try mustWait("Escape did not return to the original row") { row(ax, source) != nil }
            try requireCondition(try Data(contentsOf: oldPath) == payload && !manager.fileExists(atPath: targetPath.path),
                                 "Escape unexpectedly changed the file")
        case "target_removed":
            let moved = folder.appendingPathComponent(id + "-external.txt")
            try manager.moveItem(at: oldPath, to: moved)
            try mustWait("Externally moved target still appears editable") {
                row(ax, moved.lastPathComponent) != nil && row(ax, source) == nil
            }
            try requireCondition(try Data(contentsOf: moved) == payload && !manager.fileExists(atPath: targetPath.path),
                                 "External change lost file bytes or applied the stale rename")
        default:
            if mode == "list_click" { try pick(app, ax, other) } else { key(36) }
            try mustWait("Rename did not complete for \(id) (\(mode)); expected \(expectedName)") {
                !manager.fileExists(atPath: oldPath.path) && (try? Data(contentsOf: targetPath)) == payload
                    && row(ax, expectedName) != nil
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
guard arguments.count == 5,
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
    let beforeConfirm = URL(fileURLWithPath: arguments[4]).deletingLastPathComponent().appendingPathComponent("typed-first.png").path
    try verifyRename(app, ax, folder, beforeConfirm)
    try testFinderDrop(app, ax, finderSource, folder)
} catch {
    captureWindow(pid: pid, output: arguments[4])
    let observed = (try? FileManager.default.contentsOfDirectory(atPath: folder.path)) ?? []
    emit("rename_or_finder", "failed", "\(error); \(describeFocus(ax)); files=\(observed)")
    exit(1)
}
if activate(app) { key(12, flags: [.maskCommand]) }
