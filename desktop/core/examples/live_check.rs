//! Smoke test against a real TrusCo server using the same client code as the app.
//!
//!   TRUSCO_URL=https://trusco.app TRUSCO_CODE=XXXX-XXXX cargo run --example live_check
//!
//! Pairs a device, checks /me, syncs one synthetic block and prints what TrusCo matched.
//! The device token is never printed; disconnect the test device in TrusCo afterwards.

use chrono::{Duration, Utc};
use tracker_core::api::{BlockPayload, Client};
use tracker_core::{Engine, Sample, Settings, WindowInfo};

fn main() {
    let url = std::env::var("TRUSCO_URL").unwrap_or_else(|_| "https://trusco.app".into());
    let code = std::env::var("TRUSCO_CODE").expect("set TRUSCO_CODE to a pairing code from TrusCo");
    let client = Client::new(&url);

    let (token, account) = client.pair(&code, "Live check (delete me)").unwrap_or_else(|e| panic!("pair: {e}"));
    println!("paired: device {} in '{}' as '{}'", account.device_id, account.organization_name, account.user_name);

    let me = client.me(&token).unwrap_or_else(|e| panic!("me: {e}"));
    println!("me: device '{}' in '{}'", me.device_name, me.organization_name);

    let window = WindowInfo {
        app_name: "Microsoft Word".into(),
        title: "Live check - Settlement agreement.docx".into(),
        document_path: Some("/Clients/Live check/Live check - Settlement agreement.docx".into()),
        document_pages: Some(3),
        ..Default::default()
    };
    let (cfg, mut engine) = (Settings::default(), Engine::default());
    let start = Utc::now() - Duration::minutes(3);
    for i in 0..=30 {
        engine.tick(&Sample { at: start + Duration::seconds(i * 5), idle_secs: 0.0, window: Some(window.clone()) }, &cfg);
    }
    let payload: Vec<BlockPayload> = engine.pending_sync(&cfg, 10).iter().map(BlockPayload::from).collect();
    let results = client.sync(&token, &payload).unwrap_or_else(|e| panic!("sync: {e}"));
    for r in results {
        println!(
            "synced {}: status={:?} matter={:?} reason={:?}",
            r.external_id, r.status, r.matter_label, r.match_reason
        );
    }
}
