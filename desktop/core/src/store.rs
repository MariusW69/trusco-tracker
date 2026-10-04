use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

use crate::{Engine, Settings};

/// Non-secret account details shown in the UI (the device token lives in the OS keychain).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Account {
    pub device_id: String,
    pub device_name: String,
    pub organization_id: String,
    pub organization_name: String,
    pub user_name: String,
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn load_engine(&self) -> Engine {
        self.load("blocks.json").unwrap_or_default()
    }

    pub fn save_engine(&self, e: &Engine) -> std::io::Result<()> {
        self.save("blocks.json", e)
    }

    pub fn load_settings(&self) -> Settings {
        self.load("settings.json").unwrap_or_default()
    }

    pub fn save_settings(&self, s: &Settings) -> std::io::Result<()> {
        self.save("settings.json", s)
    }

    pub fn load_account(&self) -> Option<Account> {
        self.load("account.json")
    }

    pub fn save_account(&self, a: Option<&Account>) -> std::io::Result<()> {
        match a {
            Some(a) => self.save("account.json", a),
            None => match std::fs::remove_file(self.dir.join("account.json")) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        }
    }

    fn load<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        let bytes = std::fs::read(self.dir.join(name)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Write to a temp file then rename, so a crash never leaves a half-written file.
    fn save<T: Serialize>(&self, name: &str, value: &T) -> std::io::Result<()> {
        let tmp = self.dir.join(format!("{name}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
        restrict_permissions(&tmp);
        std::fs::rename(&tmp, self.dir.join(name))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Window titles and paths can contain privileged client information: owner-only access.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}
