//! Smoke test of the task endpoints against a real TrusCo server, using the app's client.
//!
//!   TRUSCO_URL=https://trusco.app TRUSCO_CODE=XXXX-XXXX cargo run --example live_tasks_check
//!
//! Pairs a device, creates a project and two tasks, completes one, checks validation and prints
//! what TrusCo returned. The device token is never printed. Remove the test data afterwards.

use chrono::{Duration, Local};
use serde_json::json;
use tracker_core::api::Client;
use tracker_core::tasks;

fn main() {
    let url = std::env::var("TRUSCO_URL").unwrap_or_else(|_| "https://trusco.app".into());
    let code = std::env::var("TRUSCO_CODE").expect("set TRUSCO_CODE");
    let c = Client::new(&url);
    let (token, account) = c.pair(&code, "Tasks live check (delete me)").unwrap_or_else(|e| panic!("pair: {e}"));
    println!("paired in '{}'", account.organization_name);

    println!("projects before: {}", c.projects(&token).unwrap_or_else(|e| panic!("projects: {e}")).len());
    let project = c.create_project(&token, "Tracker live check").unwrap_or_else(|e| panic!("create project: {e}"));
    println!("created project {} '{}'", project.number, project.name);

    let due = tasks::local_to_utc(Some(&Local::now().format("%Y-%m-%d").to_string()), Some("23:30")).unwrap();
    let todo = c
        .save_task(&token, &json!({ "project_id": project.id, "title": "Call client back", "kind": "todo", "priority": "high",
                                     "due_at": due.to_rfc3339(), "due_date": Local::now().format("%Y-%m-%d").to_string() }))
        .unwrap_or_else(|e| panic!("save todo: {e}"));
    let slot = (Local::now() + Duration::days(1)).format("%Y-%m-%d").to_string();
    let time = c
        .save_task(&token, &json!({ "project_id": project.id, "title": "Draft proposal", "kind": "time", "priority": "normal",
                                     "due_at": null, "due_date": slot, "estimated_hours": 2.5,
                                     "scheduled_at": tasks::local_to_utc(Some(&slot), Some("09:00")).unwrap().to_rfc3339() }))
        .unwrap_or_else(|e| panic!("save time task: {e}"));

    let now = Local::now();
    for t in [&todo, &time] {
        println!(
            "task '{}': kind={} priority={} urgency={:?} due='{}' budget={:?} slot={:?}",
            t.title,
            t.kind,
            t.priority,
            tasks::urgency(t, now),
            tasks::due_label(t, now),
            tasks::budget_label(t),
            tasks::slot_label(t, now)
        );
    }

    let done = c.save_task(&token, &json!({ "id": todo.id, "status": "done" })).unwrap_or_else(|e| panic!("done: {e}"));
    println!("completed: status={} completed_at_set={}", done.status, done.completed_at.is_some());

    let list = c.tasks(&token).unwrap_or_else(|e| panic!("tasks: {e}"));
    println!("listed: {:?}", list.iter().map(|t| format!("{} [{}]", t.title, t.status)).collect::<Vec<_>>());

    match c.save_task(&token, &json!({ "project_id": project.id, "title": "x", "kind": "meeting" })) {
        Err(e) => println!("invalid kind rejected: {e}"),
        Ok(_) => println!("invalid kind ACCEPTED (unexpected)"),
    }
    match c.save_task(&token, &json!({ "id": "00000000-0000-0000-0000-000000000000", "status": "done" })) {
        Err(e) => println!("unknown task rejected: {e}"),
        Ok(_) => println!("unknown task ACCEPTED (unexpected)"),
    }
}
