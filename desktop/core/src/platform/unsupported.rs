use crate::WindowInfo;

#[derive(Default)]
pub struct Backend;

impl Backend {
    pub fn foreground(&mut self) -> Option<WindowInfo> {
        None
    }
}

pub fn idle_seconds() -> f64 {
    0.0
}

pub fn accessibility_granted() -> bool {
    true
}

pub fn request_accessibility() {}
