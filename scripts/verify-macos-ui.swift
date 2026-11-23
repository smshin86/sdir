import AppKit
import ApplicationServices
import Carbon
import Foundation

struct JourneyFailure: Error, CustomStringConvertible {
    let description: String
    init(_ message: String) { description = message }
}

func emit(_ journey: String, _ status: String, _ detail: String) {
    let result = ["journey": journey, "status": status, "detail": detail]
    let data = try! JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
    print(String(decoding: data, as: UTF8.self))
    fflush(stdout)
}

func waitFor<T>(_ seconds: TimeInterval, _ find: () -> T?) -> T? {
    let deadline = Date().addingTimeInterval(seconds)
    repeat {
        if let value = find() { return value }
        RunLoop.current.run(until: Date().addingTimeInterval(0.06))
    } while Date() < deadline
    return nil
}

func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}

func asElement(_ value: CFTypeRef?) -> AXUIElement? {
    guard let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
    return unsafeBitCast(value, to: AXUIElement.self)
}

func axValue(_ element: AXUIElement, _ name: String) -> AXValue? {
    guard let value = attribute(element, name), CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    return unsafeBitCast(value, to: AXValue.self)
}

func frame(_ element: AXUIElement) -> CGRect? {
    guard let position = axValue(element, kAXPositionAttribute), let size = axValue(element, kAXSizeAttribute) else { return nil }
    var point = CGPoint.zero
    var dimensions = CGSize.zero
    guard AXValueGetValue(position, .cgPoint, &point), AXValueGetValue(size, .cgSize, &dimensions), dimensions.width > 0, dimensions.height > 0 else { return nil }
    return CGRect(origin: point, size: dimensions)
}

func window(_ app: AXUIElement) -> AXUIElement? {
    if let focused = asElement(attribute(app, kAXFocusedWindowAttribute)) { return focused }
    return (attribute(app, kAXWindowsAttribute) as? [AXUIElement])?.first
}

func searchAX(_ root: AXUIElement, matches: (AXUIElement) -> Bool) -> AXUIElement? {
    var queue = [root]
    var index = 0
    while index < queue.count && index < 2_000 {
        let element = queue[index]
        index += 1
        if matches(element) { return element }
        if let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] { queue.append(contentsOf: children) }
    }
    return nil
}

func hasText(_ element: AXUIElement, _ text: String) -> Bool {
    [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute, kAXHelpAttribute].contains { name in
        (attribute(element, name) as? String)?.precomposedStringWithCanonicalMapping.contains(text) == true
    }
}

func key(_ code: CGKeyCode, flags: CGEventFlags = []) {
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)!
        event.flags = flags
        event.post(tap: .cghidEventTap)
        Thread.sleep(forTimeInterval: 0.05)
    }
    RunLoop.current.run(until: Date().addingTimeInterval(0.06))
}

func activate(_ app: NSRunningApplication) -> Bool {
    _ = app.activate(options: [.activateIgnoringOtherApps])
    return waitFor(4) { NSWorkspace.shared.frontmostApplication?.processIdentifier == app.processIdentifier ? true : nil } == true
}

func findWindowServerFrame(_ pid: pid_t) -> CGRect? {
    let records = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    return records.compactMap { record -> CGRect? in
        guard (record[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid,
              (record[kCGWindowLayer as String] as? NSNumber)?.int32Value == 0,
              let bounds = record[kCGWindowBounds as String] as? [String: Any],
              let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary),
              rect.width >= 900, rect.height >= 500 else { return nil }
        return rect
    }.first
}

func inputSourceID(_ source: TISInputSource) -> String {
    guard let raw = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else { return "" }
    return Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue() as String
}

func role(_ element: AXUIElement) -> String {
    attribute(element, kAXRoleAttribute) as? String ?? ""
}

func searchField(_ ax: AXUIElement) -> AXUIElement? {
    guard let appWindow = window(ax), let close = searchAX(appWindow, matches: { hasText($0, "검색 닫기") }), let closeFrame = frame(close) else { return nil }
    return searchAX(appWindow) { element in
        guard role(element) == kAXTextFieldRole || role(element) == "AXTextArea" else { return false }
        if attribute(element, "AXDOMIdentifier") as? String == "global-query" { return true }
        if let placeholder = attribute(element, "AXPlaceholderValue") as? String,
           placeholder.contains("파일명") || placeholder.contains("Enter로 검색") { return true }
        guard let rect = frame(element) else { return false }
        return rect.minY >= closeFrame.maxY - 4 && rect.minY <= closeFrame.maxY + 80 && rect.maxX >= closeFrame.maxX - 95
    }
}

func describeFocus(_ ax: AXUIElement) -> String {
    guard let element = asElement(attribute(ax, kAXFocusedUIElementAttribute)) else { return "no focused accessibility element" }
    let placeholder = attribute(element, "AXPlaceholderValue") as? String ?? ""
    return "role=\(role(element)), placeholder=\(placeholder)"
}

func click(_ rect: CGRect) {
    let point = CGPoint(x: rect.midX, y: rect.midY)
    mouse(.mouseMoved, point)
    mouse(.leftMouseDown, point)
    Thread.sleep(forTimeInterval: 0.09)
    mouse(.leftMouseUp, point)
}

func testIME(_ app: NSRunningApplication, _ ax: AXUIElement) throws {
    guard activate(app) else { throw JourneyFailure("Could not make SDIR the foreground application") }
    guard let toggle = waitFor(12, {
        window(ax).flatMap { appWindow in
            searchAX(appWindow, matches: { role($0) == kAXButtonRole && hasText($0, "통합 검색") })
        }
    }) else { throw JourneyFailure("The native window did not expose the rendered search button") }
    key(3, flags: [.maskCommand, .maskShift])
    var field = waitFor(4) { searchField(ax) }
    if let mounted = field {
        let focused = attribute(mounted, kAXFocusedAttribute) as? Bool == true
        emit("search_shortcut", focused ? "passed" : "failed", focused ? "Command-Shift-F opened and focused the rendered search field" : "Command-Shift-F opened the search panel; \(describeFocus(ax))")
    } else {
        emit("search_shortcut", "failed", "No Spotlight field observed after Command-Shift-F; \(describeFocus(ax))")
        guard let toggleFrame = frame(toggle) else { throw JourneyFailure("The search button had no clickable frame") }
        let isOpen = window(ax).flatMap { searchAX($0, matches: { hasText($0, "검색 닫기") }) } != nil
        if !isOpen { click(toggleFrame) }
        field = waitFor(6) { searchField(ax) }
    }
    guard let field, let fieldFrame = frame(field) else { throw JourneyFailure("Could not locate the global search field after opening the panel") }
    click(fieldFrame)
    guard let original = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue(),
          let sources = TISCreateInputSourceList(nil, true)?.takeRetainedValue() else {
        throw JourneyFailure("macOS input sources are unavailable")
    }
    var korean: TISInputSource?
    for index in 0..<CFArrayGetCount(sources) {
        guard let address = CFArrayGetValueAtIndex(sources, index) else { continue }
        let source = Unmanaged<TISInputSource>.fromOpaque(address).takeUnretainedValue()
        if inputSourceID(source) == "com.apple.inputmethod.Korean.2SetKorean" { korean = source; break }
    }
    guard let korean else { throw JourneyFailure("Korean 2-set input source is unavailable") }
    defer { TISSelectInputSource(original) }
    key(18)
    key(19)
    guard waitFor(4, { (attribute(field, kAXValueAttribute) as? String) == "12" ? true : nil }) == true else {
        throw JourneyFailure("The search field did not receive the baseline physical number keys")
    }
    key(123)
    let enabled = TISEnableInputSource(korean)
    let selected = TISSelectInputSource(korean)
    guard selected == noErr, waitFor(5, {
        TISCopyCurrentKeyboardInputSource().map { inputSourceID($0.takeRetainedValue()) == inputSourceID(korean) } == true ? true : nil
    }) == true else { throw JourneyFailure("Korean input source could not be selected (enable \(enabled), select \(selected))") }
    let koreanKeys: [CGKeyCode] = [5, 40, 1, 15, 46, 3]
    for code in koreanKeys { key(code) }
    key(49)
    key(51)
    key(20)
    let expected = "1한글32"
    guard waitFor(6, {
        (attribute(field, kAXValueAttribute) as? String)?.precomposedStringWithCanonicalMapping == expected ? true : nil
    }) == true else {
        throw JourneyFailure("Physical Korean keys did not commit at the expected cursor position; input is \(attribute(field, kAXValueAttribute) as? String ?? "unavailable")")
    }
    emit("korean_ime", "passed", "Korean 2-set physical keys committed 1한글32 in the Spotlight field, preserving insertion position")
}

func mouse(_ type: CGEventType, _ point: CGPoint) {
    let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)!
    if type != .mouseMoved { event.setIntegerValueField(.mouseEventClickState, value: 1) }
    event.flags = []
    event.post(tap: .cghidEventTap)
}

func testFinderDrop(_ app: NSRunningApplication, _ ax: AXUIElement, _ source: URL, _ destination: URL) throws {
    guard activate(app), let appWindow = window(ax), let appFrame = frame(appWindow) else {
        throw JourneyFailure("Could not locate the active SDIR window")
    }
    if let close = searchAX(appWindow, matches: { hasText($0, "검색 닫기") }) {
        AXUIElementPerformAction(close, kAXPressAction as CFString)
    }
    NSWorkspace.shared.activateFileViewerSelecting([source])
    guard let finder = waitFor(8, { NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.finder").first }), activate(finder) else {
        throw JourneyFailure("Finder did not activate")
    }
    let finderAX = AXUIElementCreateApplication(finder.processIdentifier)
    let title = source.lastPathComponent
    guard let finderWindow = waitFor(8, { window(finderAX) }) else { throw JourneyFailure("Finder window did not appear") }
    var size = CGSize(width: 520, height: min(430, appFrame.height - 55))
    var point = CGPoint(x: appFrame.minX, y: appFrame.minY + 32)
    AXUIElementSetAttributeValue(finderWindow, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &size)!)
    AXUIElementSetAttributeValue(finderWindow, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &point)!)
    guard let sourceFrame = waitFor(8, {
        searchAX(finderWindow, matches: { hasText($0, title) && frame($0) != nil }).flatMap(frame)
    }), let finderFrame = frame(finderWindow) else { throw JourneyFailure("Finder accessibility did not expose the selected fixture file") }
    let start = CGPoint(x: sourceFrame.midX, y: sourceFrame.midY)
    let finish = CGPoint(x: appFrame.maxX - 42, y: appFrame.minY + appFrame.height * 0.66)
    guard finderFrame.contains(start), !finderFrame.contains(finish), CGDisplayBounds(CGMainDisplayID()).contains(finish) else {
        throw JourneyFailure("The Finder source and SDIR drop coordinates are not both visible")
    }
    mouse(.mouseMoved, start)
    Thread.sleep(forTimeInterval: 0.15)
    mouse(.leftMouseDown, start)
    Thread.sleep(forTimeInterval: 0.22)
    for step in 1...32 {
        let progress = CGFloat(step) / 32
        mouse(.leftMouseDragged, CGPoint(x: start.x + (finish.x - start.x) * progress, y: start.y + (finish.y - start.y) * progress))
        Thread.sleep(forTimeInterval: 0.045)
    }
    mouse(.leftMouseUp, finish)
    let copy = destination.appendingPathComponent(title)
    guard let original = try? Data(contentsOf: source), waitFor(10, { (try? Data(contentsOf: copy)) == original ? true : nil }) == true else {
        throw JourneyFailure("Finder drag did not produce the expected file in the SDIR destination")
    }
    guard (try? Data(contentsOf: source)) == original else { throw JourneyFailure("The original file changed after Finder drop") }
    let rowVisible = waitFor(8) {
        window(ax).flatMap { searchAX($0, matches: { hasText($0, title) }) }
    } != nil
    emit("finder_drop", "passed", "Dragged a Finder file into SDIR and verified the source and copied bytes; destination row visible=\(rowVisible)")
}

let args = CommandLine.arguments
let pid = pid_t(args[1])!
let source = URL(fileURLWithPath: args[2])
let destination = URL(fileURLWithPath: args[3])
guard let windowFrame = waitFor(20, { findWindowServerFrame(pid) }), let app = NSRunningApplication(processIdentifier: pid) else {
    emit("window_launch", "failed", "No visible SDIR window owned by the installed executable appeared")
    exit(1)
}
emit("window_launch", "passed", "Visible SDIR window observed by its process ID (\(Int(windowFrame.width)) x \(Int(windowFrame.height)))")
let trusted = AXIsProcessTrusted()
let post = CGPreflightPostEventAccess()
guard trusted && post else {
    let permissions = "Runner permissions: accessibility=\(trusted), event posting=\(post)"
    emit("korean_ime", "blocked", permissions)
    emit("finder_drop", "blocked", permissions)
    exit(0)
}

let ax = AXUIElementCreateApplication(pid)
do { try testIME(app, ax) } catch { emit("korean_ime", "failed", String(describing: error)) }
do { try testFinderDrop(app, ax, source, destination) } catch { emit("finder_drop", "failed", String(describing: error)) }
if activate(app) { key(12, flags: [.maskCommand]) }
