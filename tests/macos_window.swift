import AppKit
import Foundation

// LaunchServices is the entry point Finder uses to open a bundled application.
if CommandLine.arguments[1] == "launch" {
    let url = URL(fileURLWithPath: CommandLine.arguments[2])
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.createsNewApplicationInstance = true
    configuration.activates = false
    configuration.environment = ["HOME": CommandLine.arguments[3]]
    NSWorkspace.shared.openApplication(at: url, configuration: configuration) { app, error in
        guard let app = app else {
            fputs("LaunchServices failed: \(String(describing: error))\n", stderr)
            exit(1)
        }
        guard app.bundleIdentifier == "com.agilevision.vendredi",
              app.bundleURL?.resolvingSymlinksInPath().path == url.resolvingSymlinksInPath().path
        else {
            app.terminate()
            fputs("LaunchServices opened a different bundle\n", stderr)
            exit(1)
        }
        print(app.processIdentifier)
        exit(0)
    }
    RunLoop.main.run()
    exit(1)
}

// Wait for the measured process to own a visible application window.
let pid = Int(CommandLine.arguments[1])!
let deadline = Date().addingTimeInterval(10)
while Date() < deadline {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []
    if windows.contains(where: {
        $0[kCGWindowOwnerPID as String] as? Int == pid && $0[kCGWindowLayer as String] as? Int == 0
    }) { exit(0) }
    Thread.sleep(forTimeInterval: 0.01)
}
exit(1)
