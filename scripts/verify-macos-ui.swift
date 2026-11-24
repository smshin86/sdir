import AppKit
import ApplicationServices
import Carbon
import Foundation
import WebKit

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

struct IMEUnavailable: Error, CustomStringConvertible {
    let description: String
    init(_ detail: String) { description = detail }
}

func inputString(_ source: TISInputSource, _ property: CFString) -> String {
    guard let raw = TISGetInputSourceProperty(source, property) else { return "" }
    return Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue() as String
}

func inputFlag(_ source: TISInputSource, _ property: CFString) -> Bool {
    guard let raw = TISGetInputSourceProperty(source, property) else { return false }
    return CFBooleanGetValue(Unmanaged<CFBoolean>.fromOpaque(raw).takeUnretainedValue())
}

func inputSources(includeInstalled: Bool) -> [TISInputSource] {
    guard let array = TISCreateInputSourceList(nil, includeInstalled)?.takeRetainedValue() else { return [] }
    return (0..<CFArrayGetCount(array)).compactMap { index in
        guard let raw = CFArrayGetValueAtIndex(array, index) else { return nil }
        return Unmanaged<TISInputSource>.fromOpaque(raw).takeUnretainedValue()
    }
}

func currentSourceID() -> String {
    TISCopyCurrentKeyboardInputSource().map { inputSourceID($0.takeRetainedValue()) } ?? ""
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

func typeKoreanSample() {
    let koreanKeys: [CGKeyCode] = [5, 40, 1, 15, 46, 3]
    for code in koreanKeys { key(code) }
    key(49)
    key(51)
    key(20)
}

func textEditControl(folder: URL, source: TISInputSource, sourceID: String) throws -> String {
    let fixture = folder.appendingPathComponent("SDIR Korean IME Control.txt")
    try "12".write(to: fixture, atomically: true, encoding: .utf8)
    let opener = Process()
    opener.executableURL = URL(fileURLWithPath: "/usr/bin/open")
    opener.arguments = ["-n", "-a", "/System/Applications/TextEdit.app", fixture.path]
    try opener.run()
    guard waitFor(6, { opener.isRunning ? nil : opener.terminationStatus }) == 0 else {
        throw IMEUnavailable("Could not launch the TextEdit control document")
    }
    guard let editor = waitFor(8, {
        NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.TextEdit").first { app in
            let root = AXUIElementCreateApplication(app.processIdentifier)
            return window(root).map { hasText($0, "SDIR Korean IME Control") } ?? false
        }
    }), activate(editor) else { throw IMEUnavailable("The TextEdit control document did not become active") }
    let ax = AXUIElementCreateApplication(editor.processIdentifier)
    guard let field = waitFor(5, {
        window(ax).flatMap { root in
            searchAX(root, matches: { role($0) == "AXTextArea" && attribute($0, kAXValueAttribute) as? String == "12" })
        }
    }), let fieldFrame = frame(field) else { throw IMEUnavailable("Could not identify the TextEdit control field") }
    click(fieldFrame)
    key(124, flags: [.maskCommand])
    key(123)
    TISSelectInputSource(source)
    guard waitFor(4, { currentSourceID() == sourceID && attribute(field, kAXFocusedAttribute) as? Bool == true ? true : nil }) == true else {
        throw IMEUnavailable("The TextEdit control could not confirm both focus and Korean input selection")
    }
    typeKoreanSample()
    guard let output = waitFor(6, {
        let value = (attribute(field, kAXValueAttribute) as? String)?.precomposedStringWithCanonicalMapping
        return value?.hasPrefix("1") == true && value?.hasSuffix("32") == true ? value : nil
    }) else { throw IMEUnavailable("The TextEdit control produced no comparable physical-key result") }
    return output
}

final class WebKitIMEProbe: NSObject, WKScriptMessageHandler {
    private let output: URL
    private var events: [Any] = []
    init(output: URL) { self.output = output }
    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        guard events.count < 128 else { return }
        events.append(message.body)
        if let data = try? JSONSerialization.data(withJSONObject: events) { try? data.write(to: output, options: .atomic) }
    }
}

@MainActor func runWebKitProbe(output: URL) {
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    let window = NSWindow(contentRect: NSRect(x: 60, y: 80, width: 660, height: 260), styleMask: [.titled, .closable], backing: .buffered, defer: false)
    window.title = "SDIR WebKit IME Control"
    let probe = WebKitIMEProbe(output: output)
    let config = WKWebViewConfiguration()
    config.websiteDataStore = .nonPersistent()
    config.userContentController.add(probe, name: "imeProbe")
    let web = WKWebView(frame: window.contentView!.bounds, configuration: config)
    window.contentView = web
    window.makeKeyAndOrderFront(nil)
    app.activate(ignoringOtherApps: true)
    web.loadHTMLString("""
        <!doctype html><html lang="ko"><meta charset="utf-8">
        <style>body{padding:24px}input{font-size:26px;width:420px}</style>
        <input aria-label="WebKit IME Probe" id="probe" value="12" spellcheck="false">
        <script>
        const input = document.querySelector('#probe');
        for (const type of ['keydown','compositionstart','compositionupdate','compositionend','beforeinput','input']) {
          input.addEventListener(type, e => window.webkit.messageHandlers.imeProbe.postMessage({type,key:e.key||'',value:input.value,composing:!!e.isComposing}));
        }
        </script></html>
        """, baseURL: nil)
    withExtendedLifetime((window, probe)) { app.run() }
}

func webKitControl(folder: URL, source: TISInputSource, sourceID: String) throws -> String {
    let events = folder.appendingPathComponent("webkit-ime-events.json")
    let process = Process()
    process.executableURL = URL(fileURLWithPath: CommandLine.arguments[0])
    process.arguments = ["--webkit-ime-probe", events.path]
    try process.run()
    defer {
        if process.isRunning { process.terminate() }
        _ = waitFor(3) { process.isRunning ? nil : true }
    }
    guard let app = waitFor(8, { NSRunningApplication(processIdentifier: process.processIdentifier) }), activate(app) else {
        throw JourneyFailure("The isolated WKWebView control did not become active")
    }
    let ax = AXUIElementCreateApplication(app.processIdentifier)
    guard let field = waitFor(9, {
        window(ax).flatMap { w in searchAX(w, matches: {
            role($0) == kAXTextFieldRole && attribute($0, kAXValueAttribute) as? String == "12"
        }) }
    }), let rect = frame(field) else { throw JourneyFailure("The WKWebView control input did not render") }
    click(rect)
    key(124, flags: [.maskCommand])
    key(123)
    let selected = TISSelectInputSource(source)
    guard waitFor(4, { currentSourceID() == sourceID && attribute(field, kAXFocusedAttribute) as? Bool == true ? true : nil }) == true else {
        throw JourneyFailure("WKWebView control could not confirm input source and field focus (select \(selected))")
    }
    typeKoreanSample()
    let value = waitFor(4, {
        let value = (attribute(field, kAXValueAttribute) as? String)?.precomposedStringWithCanonicalMapping
        return value?.hasPrefix("1") == true && value?.hasSuffix("32") == true ? value : nil
    }) ?? (attribute(field, kAXValueAttribute) as? String ?? "unavailable")
    let data = (try? Data(contentsOf: events)) ?? Data()
    let records = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] ?? []
    let types = records.compactMap { $0["type"] as? String }
    let composition = types.filter { $0.hasPrefix("composition") }.count
    let input = types.filter { $0 == "input" }.count
    return "value=\(value), composition events=\(composition), input events=\(input)"
}

func testIME(_ app: NSRunningApplication, _ ax: AXUIElement, _ controlFolder: URL) throws {
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
    guard let original = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue() else {
        throw IMEUnavailable("macOS input sources are unavailable")
    }
    let koreanID = "com.apple.inputmethod.Korean.2SetKorean"
    let available = inputSources(includeInstalled: true)
    guard let korean = available.first(where: { inputSourceID($0) == koreanID }) else {
        throw IMEUnavailable("Korean 2-set input source is unavailable")
    }
    let bundle = inputString(korean, kTISPropertyBundleID)
    let method = available.first { source in
        inputString(source, kTISPropertyInputSourceType) == kTISTypeKeyboardInputMethodModeEnabled as String &&
            (inputSourceID(source) == "com.apple.inputmethod.Korean" || (!bundle.isEmpty && inputString(source, kTISPropertyBundleID) == bundle))
    }
    let modeWasEnabled = inputFlag(korean, kTISPropertyInputSourceIsEnabled)
    let parentWasEnabled = method.map { inputFlag($0, kTISPropertyInputSourceIsEnabled) } ?? false
    defer {
        TISSelectInputSource(original)
        if !modeWasEnabled { TISDisableInputSource(korean) }
        if let method, !parentWasEnabled { TISDisableInputSource(method) }
    }
    key(18)
    key(19)
    guard waitFor(4, { (attribute(field, kAXValueAttribute) as? String) == "12" ? true : nil }) == true else {
        throw JourneyFailure("The search field did not receive the baseline physical number keys")
    }
    key(123)
    let parentResult = method.map { parentWasEnabled ? noErr : TISEnableInputSource($0) }
    let mode = inputSources(includeInstalled: true).first { inputSourceID($0) == koreanID } ?? korean
    let enableResult = inputFlag(mode, kTISPropertyInputSourceIsEnabled) ? noErr : TISEnableInputSource(mode)
    guard let selectable = waitFor(6, {
        inputSources(includeInstalled: false).first {
            inputSourceID($0) == koreanID && inputFlag($0, kTISPropertyInputSourceIsEnabled) && inputFlag($0, kTISPropertyInputSourceIsSelectCapable)
        }
    }) else {
        let parent = method.map(inputSourceID) ?? "unavailable"
        let parentStatus = parentResult.map { String($0) } ?? "unavailable"
        throw IMEUnavailable("Runner did not register a selectable Korean mode (parent \(parent), parent status \(parentStatus), mode status \(enableResult))")
    }
    let originalID = inputSourceID(original)
    var switchPath = "TIS fallback"
    if originalID != koreanID {
        TISSelectInputSource(original)
        let originalFocused = waitFor(4, {
            currentSourceID() == originalID && attribute(field, kAXFocusedAttribute) as? Bool == true ? true : nil
        }) == true
        if originalFocused {
            key(49, flags: [.maskControl])
            if waitFor(5, { currentSourceID() == koreanID && attribute(field, kAXFocusedAttribute) as? Bool == true ? true : nil }) == true {
                switchPath = "Control-Space"
            }
        }
    }
    guard (attribute(field, kAXValueAttribute) as? String) == "12" else {
        throw IMEUnavailable("Control-Space changed the baseline text instead of selecting Korean: \(attribute(field, kAXValueAttribute) as? String ?? "unavailable")")
    }
    if switchPath != "Control-Space" {
        let selected = TISSelectInputSource(selectable)
        guard waitFor(5, { currentSourceID() == koreanID ? true : nil }) == true else {
            throw IMEUnavailable("Runner did not switch to Korean 2-set (select status \(selected), current ID \(currentSourceID()))")
        }
    }
    typeKoreanSample()
    let expected = "1한글32"
    guard waitFor(6, {
        (attribute(field, kAXValueAttribute) as? String)?.precomposedStringWithCanonicalMapping == expected ? true : nil
    }) == true else {
        let sdirValue = attribute(field, kAXValueAttribute) as? String ?? "unavailable"
        let control: String
        do {
            control = try textEditControl(folder: controlFolder, source: selectable, sourceID: koreanID)
        } catch {
            throw IMEUnavailable("SDIR held \(sdirValue); TextEdit comparison incomplete: \(error)")
        }
        if control == expected {
            let webKit: String
            do { webKit = try webKitControl(folder: controlFolder, source: selectable, sourceID: koreanID) }
            catch { webKit = "inconclusive: \(error)" }
            throw JourneyFailure("SDIR after \(switchPath)=\(sdirValue); TextEdit=\(control); plain WKWebView \(webKit)")
        }
        throw IMEUnavailable("The same keys under Korean selection yielded \(sdirValue) in SDIR and \(control) in TextEdit; the runner did not produce the Korean control result")
    }
    emit("korean_ime", "passed", "Korean 2-set physical keys committed 1한글32 in the Spotlight field after \(switchPath), preserving insertion position")
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
if args.count == 3 && args[1] == "--webkit-ime-probe" {
    MainActor.assumeIsolated { runWebKitProbe(output: URL(fileURLWithPath: args[2])) }
    exit(0)
}
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
do { try testIME(app, ax, destination.deletingLastPathComponent()) }
catch let reason as IMEUnavailable { emit("korean_ime", "blocked", reason.description) }
catch { emit("korean_ime", "failed", String(describing: error)) }
do { try testFinderDrop(app, ax, source, destination) } catch { emit("finder_drop", "failed", String(describing: error)) }
if activate(app) { key(12, flags: [.maskCommand]) }
