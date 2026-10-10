//! Native delivery is a presentation adapter. The Server owns durable attention
//! history; receiving/showing a banner never approves or marks a record read.
use crate::SERVER;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::Deserialize;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

#[derive(Deserialize)]
struct Feed {
    items: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    sequence: i64,
    project_name: String,
    kind: String,
    read: bool,
    resolved: bool,
}
fn title(kind: &str) -> Option<&'static str> {
    match kind {
        "run_completed" => Some("Agent 工作已完成"),
        "run_failed" => Some("Agent 运行失败"),
        "run_interrupted" => Some("Agent 运行已中断"),
        "approval_requested" => Some("需要你批准操作"),
        _ => None,
    }
}
pub async fn listen(app: tauri::AppHandle, enabled: Arc<AtomicBool>) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .read_timeout(Duration::from_secs(45))
        .build()
        .unwrap();
    let mut cursor: Option<i64> = None;
    loop {
        let url = format!(
            "{SERVER}/v1/notifications/stream{}",
            cursor.map(|i| format!("?after={i}")).unwrap_or_default()
        );
        match client.get(&url).send().await {
            Ok(response)
                if response.status().is_success()
                    && response.headers().get("content-type").is_some_and(|h| {
                        h.to_str().unwrap_or("").starts_with("text/event-stream")
                    }) =>
            {
                if let Some(window) = app.get_webview_window("main") {
                    if window.url().is_ok_and(|url| url.scheme() == "tauri") {
                        let _ = window.navigate(format!("{SERVER}/console/").parse().unwrap());
                    }
                }
                let mut events = response.bytes_stream().eventsource();
                while let Some(Ok(event)) = events.next().await {
                    if event.event == "ready" {
                        cursor = event.id.parse().ok();
                        continue;
                    }
                    if event.event != "notifications" {
                        continue;
                    }
                    let Ok(feed) = serde_json::from_str::<Feed>(&event.data) else {
                        break;
                    };
                    let fresh: Vec<_> = feed
                        .items
                        .iter()
                        .filter(|n| {
                            n.sequence > cursor.unwrap_or(0)
                                && !n.read
                                && !n.resolved
                                && title(&n.kind).is_some()
                        })
                        .collect();
                    if enabled.load(Ordering::Relaxed) {
                        if fresh.len() > 3 {
                            let _ = app
                                .notification()
                                .builder()
                                .title("Feynman · 项目有新动态")
                                .body(format!("{} 条新提醒；请打开消息中心查看。", fresh.len()))
                                .show();
                        } else {
                            for n in fresh {
                                // No raw prompt, CLI output, credential or external message text.
                                if let Err(e) = app
                                    .notification()
                                    .builder()
                                    .title(format!("Feynman · {}", title(&n.kind).unwrap()))
                                    .body(n.project_name.chars().take(80).collect::<String>())
                                    .show()
                                {
                                    eprintln!("native notification dispatch: {e}");
                                }
                            }
                        }
                    }
                    cursor = event.id.parse().ok().or(cursor);
                }
            }
            Ok(response) if response.status() == reqwest::StatusCode::BAD_REQUEST => cursor = None, // reset DB / invalid old cursor
            _ => {}
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
