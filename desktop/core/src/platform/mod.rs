//! Foreground window + idle time, per OS. Each backend fills in a `WindowInfo`;
//! `Sampler` adds things that are OS-independent (page counts).

use crate::WindowInfo;
use crate::docx::PageCache;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as os;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows as os;

#[cfg(not(any(target_os = "macos", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "macos", windows)))]
use unsupported as os;

pub const SELF_APP_NAME: &str = "TrusCo Tracker";

pub struct Sampler {
    os: os::Backend,
    pages: PageCache,
}

impl Default for Sampler {
    fn default() -> Self {
        Self { os: os::Backend::default(), pages: PageCache::default() }
    }
}

impl Sampler {
    /// Seconds since the last keyboard/mouse input, and the window in front.
    pub fn sample(&mut self) -> (f64, Option<WindowInfo>) {
        let idle = os::idle_seconds();
        let mut window = self.os.foreground();
        if let Some(w) = window.as_mut() {
            if let Some(path) = &w.document_path {
                w.document_pages = self.pages.get(path);
            }
        }
        (idle, window)
    }
}

/// macOS needs the Accessibility permission to read window titles; other platforms don't.
pub fn accessibility_granted() -> bool {
    os::accessibility_granted()
}

/// Shows the OS permission prompt / settings page where relevant.
pub fn request_accessibility() {
    os::request_accessibility()
}

/// "file:///Users/a/My%20Doc.docx" -> "/Users/a/My Doc.docx"; other strings unchanged.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn file_url_to_path(s: &str) -> String {
    let Some(rest) = s.strip_prefix("file://") else {
        return s.to_string();
    };
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_file_urls() {
        assert_eq!(file_url_to_path("file:///Users/a/My%20Doc.docx"), "/Users/a/My Doc.docx");
        assert_eq!(file_url_to_path("file:///Users/a/Caf%C3%A9.docx"), "/Users/a/Café.docx");
        assert_eq!(file_url_to_path("file:///x/100%"), "/x/100%");
        assert_eq!(file_url_to_path("https://x.sharepoint.com/a.docx"), "https://x.sharepoint.com/a.docx");
    }
}
