// Lets the Rust app ask macOS to redraw the TrusCo Tasks widget (WidgetKit is Swift-only).
import WidgetKit

@_cdecl("trusco_reload_widgets")
public func truscoReloadWidgets() {
    WidgetCenter.shared.reloadAllTimelines()
}
