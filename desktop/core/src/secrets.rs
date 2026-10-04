//! The device token is stored in the macOS Keychain / Windows Credential Manager.

const SERVICE: &str = "app.trusco.tracker";
const ACCOUNT: &str = "device-token";

fn entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT)
}

pub fn load_token() -> Option<String> {
    entry().ok()?.get_password().ok()
}

pub fn save_token(token: &str) -> Result<(), String> {
    entry().and_then(|e| e.set_password(token)).map_err(|e| e.to_string())
}

pub fn clear_token() {
    if let Ok(e) = entry() {
        let _ = e.delete_credential();
    }
}
