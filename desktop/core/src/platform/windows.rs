//! Windows: foreground window via Win32, idle time via GetLastInputInfo,
//! Office document paths via the running Office COM object (through PowerShell).

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
};
use windows::core::PWSTR;

use super::SELF_APP_NAME;
use crate::WindowInfo;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn idle_seconds() -> f64 {
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe {
        if GetLastInputInfo(&mut info).as_bool() {
            return GetTickCount().wrapping_sub(info.dwTime) as f64 / 1000.0;
        }
    }
    0.0
}

pub fn accessibility_granted() -> bool {
    true
}

pub fn request_accessibility() {}

#[derive(Default)]
pub struct Backend {
    /// (exe, window title) -> document path, so Office is only asked once per window.
    office_paths: HashMap<(String, String), Option<String>>,
}

impl Backend {
    pub fn foreground(&mut self) -> Option<WindowInfo> {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            return None;
        }
        let title = unsafe {
            let len = GetWindowTextLengthW(hwnd).max(0) as usize;
            let mut buf = vec![0u16; len + 1];
            let n = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
            String::from_utf16_lossy(&buf[..n])
        };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == std::process::id() {
            return Some(WindowInfo { app_name: SELF_APP_NAME.into(), ..Default::default() });
        }

        let exe = process_image(pid);
        let stem = exe
            .as_deref()
            .and_then(|p| Path::new(p).file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let document_path = self.office_document_path(&stem, &title);

        Some(WindowInfo {
            app_name: friendly_name(&stem),
            app_bundle: exe,
            title,
            document_path,
            document_pages: None,
        })
    }

    fn office_document_path(&mut self, stem: &str, title: &str) -> Option<String> {
        let (prog_id, member) = match stem.to_ascii_uppercase().as_str() {
            "WINWORD" => ("Word.Application", "ActiveDocument"),
            "EXCEL" => ("Excel.Application", "ActiveWorkbook"),
            "POWERPNT" => ("PowerPoint.Application", "ActivePresentation"),
            _ => return None,
        };
        let cache_key = (stem.to_string(), title.to_string());
        if let Some(hit) = self.office_paths.get(&cache_key) {
            return hit.clone();
        }
        let script = format!(
            "[Console]::OutputEncoding=[Text.Encoding]::UTF8; \
             try {{ ([Runtime.InteropServices.Marshal]::GetActiveObject('{prog_id}')).{member}.FullName }} catch {{ }}"
        );
        // Windows PowerShell 5.1 (not pwsh 7) still has Marshal.GetActiveObject.
        let path = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        if self.office_paths.len() > 500 {
            self.office_paths.clear();
        }
        self.office_paths.insert(cache_key, path.clone());
        path
    }
}

fn process_image(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size);
        let _ = CloseHandle(handle);
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    }
}

/// Same names as macOS reports, so TrusCo's matching treats both platforms alike.
fn friendly_name(stem: &str) -> String {
    match stem.to_ascii_lowercase().as_str() {
        "winword" => "Microsoft Word",
        "excel" => "Microsoft Excel",
        "powerpnt" => "Microsoft PowerPoint",
        "outlook" | "olk" => "Microsoft Outlook",
        "ms-teams" | "teams" => "Microsoft Teams",
        "zoom" => "zoom.us",
        "chrome" => "Google Chrome",
        "msedge" => "Microsoft Edge",
        "firefox" => "Firefox",
        "acrord32" | "acrobat" => "Adobe Acrobat",
        "foxitpdfreader" | "foxitreader" | "foxitphantompdf" => "Foxit PDF",
        "explorer" => "File Explorer",
        "whatsapp" => "WhatsApp",
        "1password" => "1Password",
        "keepassxc" => "KeePassXC",
        "bitwarden" => "Bitwarden",
        "lockapp" => "loginwindow",
        _ => return stem.to_string(),
    }
    .to_string()
}
