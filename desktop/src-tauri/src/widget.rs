//! Asks macOS to redraw the TrusCo Tasks widget after the task snapshot changes.

#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// Swift (WidgetCenter.shared.reloadAllTimelines), compiled by build.rs from swift/WidgetBridge.swift.
    fn trusco_reload_widgets();
}

#[cfg(target_os = "macos")]
pub fn reload() {
    unsafe { trusco_reload_widgets() }
}

#[cfg(not(target_os = "macos"))]
pub fn reload() {}
