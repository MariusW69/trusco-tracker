//! macOS: frontmost app from NSWorkspace, window title + document from the
//! Accessibility API (needs the Accessibility permission, no screen recording),
//! Office document paths via AppleScript as a fallback.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::process::Command;

use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::CFURL;
use objc2_app_kit::NSWorkspace;

use super::{SELF_APP_NAME, file_url_to_path};
use crate::WindowInfo;

type AXUIElementRef = *const c_void;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: *mut CFTypeRef) -> i32;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout_secs: f32) -> i32;
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state_id: i32, event_type: u32) -> f64;
}

const COMBINED_SESSION_STATE: i32 = 0;
const ANY_INPUT_EVENT: u32 = u32::MAX;

pub fn idle_seconds() -> f64 {
    unsafe { CGEventSourceSecondsSinceLastEventType(COMBINED_SESSION_STATE, ANY_INPUT_EVENT) }
}

pub fn accessibility_granted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

pub fn request_accessibility() {
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let opts = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
    unsafe {
        AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef());
    }
    let _ = Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

#[derive(Default)]
pub struct Backend {
    /// (bundle id, window title) -> document path from AppleScript, so we ask Word once per window.
    office_paths: HashMap<(String, String), Option<String>>,
    /// Apps where the user declined the Automation prompt; don't ask again this session.
    office_denied: HashSet<String>,
}

impl Backend {
    pub fn foreground(&mut self) -> Option<WindowInfo> {
        let (name, bundle, pid) = objc2::rc::autoreleasepool(|_| {
            let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
            let name = app.localizedName().map(|s| s.to_string()).unwrap_or_default();
            let bundle = app.bundleIdentifier().map(|s| s.to_string());
            Some((name, bundle, app.processIdentifier()))
        })?;

        if pid as u32 == std::process::id() {
            return Some(WindowInfo { app_name: SELF_APP_NAME.into(), ..Default::default() });
        }

        let (title, mut document_path) = if accessibility_granted() { focused_window(pid) } else { (String::new(), None) };

        if document_path.is_none() {
            if let Some(b) = bundle.as_deref() {
                document_path = self.office_document_path(b, &title);
            }
        }

        Some(WindowInfo { app_name: name, app_bundle: bundle, title, document_path, document_pages: None })
    }

    fn office_document_path(&mut self, bundle: &str, title: &str) -> Option<String> {
        let (app, expr) = match bundle {
            "com.microsoft.Word" => ("Microsoft Word", "full name of active document"),
            "com.microsoft.Excel" => ("Microsoft Excel", "full name of active workbook"),
            "com.microsoft.Powerpoint" => ("Microsoft PowerPoint", "full name of active presentation"),
            _ => return None,
        };
        if self.office_denied.contains(bundle) {
            return None;
        }
        let cache_key = (bundle.to_string(), title.to_string());
        if let Some(hit) = self.office_paths.get(&cache_key) {
            return hit.clone();
        }

        let out = Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(format!("tell application \"{app}\" to return {expr}"))
            .output()
            .ok()?;
        let path = if out.status.success() {
            hfs_to_posix(String::from_utf8_lossy(&out.stdout).trim())
        } else {
            // -1743: the user said no to "TrusCo Tracker wants to control Microsoft Word".
            if String::from_utf8_lossy(&out.stderr).contains("-1743") {
                self.office_denied.insert(bundle.to_string());
            }
            None
        };
        if self.office_paths.len() > 500 {
            self.office_paths.clear();
        }
        self.office_paths.insert(cache_key, path.clone());
        path
    }
}

/// Title and document of the app's focused window, via the Accessibility API.
fn focused_window(pid: i32) -> (String, Option<String>) {
    unsafe {
        let raw = AXUIElementCreateApplication(pid);
        if raw.is_null() {
            return (String::new(), None);
        }
        let app = CFType::wrap_under_create_rule(raw);
        // A hung app must not stall the tracker.
        AXUIElementSetMessagingTimeout(app.as_CFTypeRef(), 0.5);
        let Some(window) = copy_attr(app.as_CFTypeRef(), "AXFocusedWindow") else {
            return (String::new(), None);
        };
        let title = copy_attr(window.as_CFTypeRef(), "AXTitle").and_then(|v| cf_string(&v)).unwrap_or_default();
        let document = copy_attr(window.as_CFTypeRef(), "AXDocument")
            .and_then(|v| cf_string(&v))
            .filter(|s| !s.is_empty())
            .map(|s| file_url_to_path(&s));
        (title, document)
    }
}

unsafe fn copy_attr(element: AXUIElementRef, name: &str) -> Option<CFType> {
    let attr = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value) };
    if err != 0 || value.is_null() {
        return None;
    }
    Some(unsafe { CFType::wrap_under_create_rule(value) })
}

fn cf_string(v: &CFType) -> Option<String> {
    if let Some(s) = v.downcast::<CFString>() {
        return Some(s.to_string());
    }
    v.downcast::<CFURL>().map(|u| u.get_string().to_string())
}

/// Office sometimes answers with classic "Macintosh HD:Users:a:Doc.docx" paths.
fn hfs_to_posix(s: &str) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    if s.starts_with('/') || s.contains("://") {
        return Some(s.to_string());
    }
    let (volume, rest) = s.split_once(':')?;
    let rest = rest.replace(':', "/");
    let on_boot_volume = format!("/{rest}");
    if std::path::Path::new(&on_boot_volume).exists() {
        Some(on_boot_volume)
    } else {
        Some(format!("/Volumes/{volume}/{rest}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_hfs_paths() {
        assert_eq!(hfs_to_posix("/Users/a/x.docx").as_deref(), Some("/Users/a/x.docx"));
        assert_eq!(hfs_to_posix("Macintosh HD:Users").as_deref(), Some("/Users"));
        assert_eq!(hfs_to_posix("USB:Clients:x.docx").as_deref(), Some("/Volumes/USB/Clients/x.docx"));
        assert_eq!(hfs_to_posix(""), None);
    }
}
