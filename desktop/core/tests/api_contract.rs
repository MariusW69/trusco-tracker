//! Exercises the HTTP client against a local stand-in for TrusCo's
//! /api/public/tracker/* routes, using the response shapes those routes return.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use chrono::{Duration, Utc};
use tracker_core::api::{ApiError, BlockPayload, Client};
use tracker_core::{Engine, Sample, Settings, WindowInfo};

struct Request {
    method: String,
    path: String,
    authorization: Option<String>,
    body: serde_json::Value,
}

/// Serves `responses` in order (status, JSON body) and reports each request it saw.
fn serve(responses: Vec<(u16, &'static str)>) -> (String, mpsc::Receiver<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or_default().to_string();
            let (mut len, mut authorization) = (0usize, None);
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                let h = h.trim_end();
                if h.is_empty() {
                    break;
                }
                let (k, v) = h.split_once(':').unwrap();
                match k.to_ascii_lowercase().as_str() {
                    "content-length" => len = v.trim().parse().unwrap(),
                    "authorization" => authorization = Some(v.trim().to_string()),
                    _ => {}
                }
            }
            let mut buf = vec![0; len];
            reader.read_exact(&mut buf).unwrap();
            let request_body = serde_json::from_slice(&buf).unwrap_or(serde_json::Value::Null);
            tx.send(Request { method, path, authorization, body: request_body }).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (base, rx)
}

const TOKEN: &str = "ttk_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn pair_me_and_sync_round_trip() {
    let pair_ok = r#"{"device_id":"d-1","device_token":"ttk_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","organization_id":"o-1","organization_name":"Example Attorneys Inc","user_name":null}"#;
    let me_ok = r#"{"device_id":"d-1","device_name":"MacBook","organization_id":"o-1","organization_name":null,"user_name":"A. Attorney"}"#;
    let sync_ok = r#"{"results":[{"external_id":"__ID__","status":"pending","matter_id":"m-4","matter_label":"M-1004 · Divorce Settlement","match_reason":"Client name \"jane smit\""}],"server_time":"2026-10-04T09:00:00Z"}"#;
    // The sync reply must echo the block id, which we only know after building the block.
    let mut engine = Engine::default();
    let cfg = Settings::default();
    let w = WindowInfo {
        app_name: "Microsoft Word".into(),
        title: "Jane Smit - Divorce Settlement.docx".into(),
        document_path: Some("/Clients/Smit/Jane Smit - Divorce Settlement.docx".into()),
        document_pages: Some(12),
        ..Default::default()
    };
    let start = Utc::now() - Duration::minutes(5);
    for i in 0..=24 {
        engine.tick(&Sample { at: start + Duration::seconds(i * 5), idle_secs: 0.0, window: Some(w.clone()) }, &cfg);
    }
    let pending = engine.pending_sync(&cfg, 200);
    assert_eq!(pending.len(), 1);
    let sync_body: &'static str = Box::leak(sync_ok.replace("__ID__", &pending[0].id).into_boxed_str());

    let (base, seen) = serve(vec![(200, pair_ok), (200, me_ok), (200, sync_body)]);
    let client = Client::new(&base);

    let (token, account) = client.pair("k7qf-3mxa", "MacBook").map_err(|e| e.to_string()).unwrap();
    assert_eq!(token, TOKEN);
    assert_eq!(account.organization_name, "Example Attorneys Inc");
    let r = seen.recv().unwrap();
    assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/api/public/tracker/pair"));
    assert_eq!(r.body["code"], "k7qf-3mxa");
    assert_eq!(r.body["device_name"], "MacBook");
    assert!(r.body["platform"].is_string() && r.body["app_version"].is_string());

    let me = client.me(&token).map_err(|e| e.to_string()).unwrap();
    assert_eq!(me.user_name, "A. Attorney");
    assert_eq!(me.organization_name, "");
    let r = seen.recv().unwrap();
    assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/api/public/tracker/me"));
    assert_eq!(r.authorization.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));

    let payload: Vec<BlockPayload> = pending.iter().map(BlockPayload::from).collect();
    let results = client.sync(&token, &payload).map_err(|e| e.to_string()).unwrap();
    assert_eq!(results[0].matter_label.as_deref(), Some("M-1004 · Divorce Settlement"));
    let r = seen.recv().unwrap();
    assert_eq!(r.path, "/api/public/tracker/sync");
    let b = &r.body["blocks"][0];
    assert_eq!(b["external_id"], pending[0].id.as_str());
    assert_eq!(b["active_seconds"], 120);
    assert_eq!(b["document_pages"], 12);
    assert_eq!(b["app_name"], "Microsoft Word");
    assert!(b["started_at"].as_str().unwrap().contains('T'));
}

#[test]
fn maps_error_statuses() {
    let (base, _seen) = serve(vec![
        (401, r#"{"error":"invalid_or_expired_code"}"#),
        (400, r#"{"error":"invalid_code"}"#),
        (401, r#"{"error":"invalid_device"}"#),
        (500, r#"{"error":"ingest_failed"}"#),
    ]);
    let client = Client::new(&base);
    assert!(matches!(client.pair("AAAA-BBBB", "x"), Err(ApiError::InvalidCode)));
    assert!(matches!(client.pair("A", "x"), Err(ApiError::InvalidCode)));
    assert!(matches!(client.sync(TOKEN, &[]), Err(ApiError::Unauthorized)));
    assert!(matches!(client.sync(TOKEN, &[]), Err(ApiError::Server(500, _))));
}
