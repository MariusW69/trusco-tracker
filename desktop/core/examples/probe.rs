//! Network probe: posts an invalid pairing code; a healthy connection answers "invalid code".
fn main() {
    let url = std::env::var("TRUSCO_URL").unwrap_or_else(|_| "https://trusco.app".into());
    match tracker_core::api::Client::new(&url).pair("AAAA-BBBB", "probe") {
        Err(tracker_core::api::ApiError::InvalidCode) => println!("OK: reached {url} (code rejected as expected)"),
        Err(e) => println!("FAILED: {e}"),
        Ok(_) => println!("unexpected success"),
    }
}
