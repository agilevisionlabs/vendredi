import CoreGraphics
import Foundation

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
