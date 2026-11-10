// Bridges from the Rust app to Swift-only APIs used by the TrusCo Tasks widget.
import Foundation
import WidgetKit

/// Asks macOS to redraw the widget after the task snapshot changes.
@_cdecl("trusco_reload_widgets")
public func truscoReloadWidgets() {
    WidgetCenter.shared.reloadAllTimelines()
}

/// Copies the App Group folder's path (NUL-terminated UTF-8) into `buf`; false if unavailable.
///
/// The folder must be obtained this way, not built from ~/Library/Group Containers/<group>:
/// macOS 15+ protects group containers, and this call is what makes it check our team-ID-prefixed
/// App Group entitlement and grant this process access. Writing to the hand-built path is
/// treated as "access data from other apps" and is silently denied (or prompts the user).
@_cdecl("trusco_group_container")
public func truscoGroupContainer(_ group: UnsafePointer<CChar>, _ buf: UnsafeMutablePointer<CChar>, _ len: Int) -> Bool {
    guard let url = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: String(cString: group)) else {
        return false
    }
    return url.path.withCString { strlcpy(buf, $0, len) < len }
}
