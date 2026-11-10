//! The macOS TrusCo Tasks widget: where its task snapshot lives, and asking macOS to redraw it.

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{CStr, CString, OsStr, c_char};
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    // Swift, compiled by build.rs from swift/WidgetBridge.swift.
    unsafe extern "C" {
        fn trusco_reload_widgets();
        fn trusco_group_container(group: *const c_char, buf: *mut c_char, len: usize) -> bool;
    }

    pub fn reload() {
        unsafe { trusco_reload_widgets() }
    }

    /// Asked of macOS once per run; that request is also what grants access (see WidgetBridge.swift).
    pub fn group_dir() -> Option<PathBuf> {
        static DIR: OnceLock<PathBuf> = OnceLock::new();
        if let Some(dir) = DIR.get() {
            return Some(dir.clone());
        }
        let group = CString::new(tracker_core::tasks::APP_GROUP).ok()?;
        let mut buf = vec![0 as c_char; 4096];
        if !unsafe { trusco_group_container(group.as_ptr(), buf.as_mut_ptr(), buf.len()) } {
            return None;
        }
        let path = PathBuf::from(OsStr::from_bytes(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_bytes()));
        Some(DIR.get_or_init(|| path).clone())
    }
}

/// The App Group folder shared with the widget, and asking macOS to redraw the widget.
#[cfg(target_os = "macos")]
pub use mac::{group_dir, reload};

/// Widgets only exist on macOS.
#[cfg(not(target_os = "macos"))]
pub fn group_dir() -> Option<std::path::PathBuf> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn reload() {}
