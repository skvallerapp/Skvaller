// The macOS side of bench.py: finds an app's window and scrolls it with real wheel events.
//
//   mac-input trusted                 exit 0 when this process may post events (Accessibility)
//   mac-input window <pid>            print "x y width height" of pid's largest on-screen window
//   mac-input focus <pid> <x> <y>     activate pid's app and move the pointer to x, y
//   mac-input scroll <n> <delta> <ms> post n wheel events of delta lines, ms apart
//
// Coordinates are global points, top-left origin, as CGWindowList reports them.

import AppKit
import ApplicationServices

let args = CommandLine.arguments

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(2)
}

switch args.count > 1 ? args[1] : "" {
case "trusted":
    exit(AXIsProcessTrusted() ? 0 : 1)

case "window":
    guard args.count == 3, let pid = Int(args[2]) else { fail("usage: window <pid>") }
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]] ?? []
    var best: CGRect?
    for info in list {
        guard (info[kCGWindowOwnerPID as String] as? Int) == pid,
              (info[kCGWindowLayer as String] as? Int) == 0,
              let dict = info[kCGWindowBounds as String] as? NSDictionary,
              let rect = CGRect(dictionaryRepresentation: dict),
              rect.width >= 300, rect.height >= 300 else { continue }
        if best == nil || rect.width * rect.height > best!.width * best!.height { best = rect }
    }
    guard let rect = best else { exit(1) }
    print("\(Int(rect.minX)) \(Int(rect.minY)) \(Int(rect.width)) \(Int(rect.height))")

case "focus":
    guard args.count == 5, let pid = Int32(args[2]), let x = Double(args[3]), let y = Double(args[4]) else {
        fail("usage: focus <pid> <x> <y>")
    }
    NSRunningApplication(processIdentifier: pid)?.activate(options: [.activateIgnoringOtherApps])
    let point = CGPoint(x: x, y: y)
    CGWarpMouseCursorPosition(point)
    CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left)?
        .post(tap: .cghidEventTap)

case "scroll":
    guard args.count == 5, let n = Int(args[2]), let delta = Int32(args[3]), let ms = UInt32(args[4]) else {
        fail("usage: scroll <n> <delta> <ms>")
    }
    for _ in 0..<n {
        CGEvent(scrollWheelEvent2Source: nil, units: .line, wheelCount: 1, wheel1: delta, wheel2: 0, wheel3: 0)?
            .post(tap: .cghidEventTap)
        usleep(ms * 1000)
    }

default:
    fail("usage: mac-input trusted | window <pid> | focus <pid> <x> <y> | scroll <n> <delta> <ms>")
}
