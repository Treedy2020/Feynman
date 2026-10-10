//! Independent terminal views over durable source histories, never a Provider listener.
use super::*;
use feynman_core::{ObservationChannel, ObservationFeedPage};

#[derive(Subcommand)]
pub enum FeedCommand {
    Read {
        project_id: String,
        #[arg(long)]
        source: Option<String>,
        #[arg(long, default_value_t = 0)]
        after: i64,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(i64).range(1..=200))]
        limit: i64,
    },
    Watch {
        project_id: String,
        #[arg(long)]
        source: Option<String>,
        /// Omit to start at the current head; use 0 to replay retained history first.
        #[arg(long)]
        after: Option<i64>,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=60))]
        poll_seconds: u64,
    },
}
struct FetchFailure {
    retryable: bool,
}

async fn page(
    client: &Client,
    base: &reqwest::Url,
    project: &str,
    source: Option<&str>,
    after: i64,
    limit: i64,
    head: bool,
) -> std::result::Result<ObservationFeedPage, FetchFailure> {
    let mut url = base.clone();
    url.set_path(&format!("/v1/projects/{project}/feed"));
    url.query_pairs_mut()
        .append_pair("after", &after.to_string())
        .append_pair("limit", &limit.to_string())
        .append_pair("head", if head { "true" } else { "false" });
    if let Some(source) = source {
        url.query_pairs_mut().append_pair("source_id", source);
    }
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| FetchFailure { retryable: true })?;
    let status = response.status();
    if !status.is_success() {
        return Err(FetchFailure {
            retryable: status.is_server_error() || status.as_u16() == 429,
        });
    }
    response
        .json()
        .await
        .map_err(|_| FetchFailure { retryable: false })
}
fn output(value: &Value) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, value)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}
fn terminal_text(text: &str) -> String {
    text.chars()
        .take(4096)
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
fn show_channels(channels: &[ObservationChannel], after: i64, machine: bool) -> Result<()> {
    if machine {
        return output(&json!({"event":"channels","channels":channels,"after":after}));
    }
    eprintln!(
        "Watching {} channel(s), after {}. Ctrl-C closes the view, not collection.",
        channels.len(),
        after
    );
    for c in channels {
        eprintln!(
            "{} · {} · {} · coverage={}",
            c.source_id,
            terminal_text(&c.name),
            if !c.enabled {
                "paused"
            } else if c.kind == "dws_messages" {
                "polling"
            } else {
                &c.listener_state
            },
            c.coverage
        );
    }
    Ok(())
}

pub async fn handle(base: &str, command: &Command, machine: bool) -> Result<bool> {
    let (project, source, after, watch, limit, interval) = match command {
        Command::Feed {
            command:
                FeedCommand::Read {
                    project_id,
                    source,
                    after,
                    limit,
                },
        } => (
            project_id,
            source.as_deref(),
            Some(*after),
            false,
            *limit,
            1,
        ),
        Command::Feed {
            command:
                FeedCommand::Watch {
                    project_id,
                    source,
                    after,
                    poll_seconds,
                },
        } => (
            project_id,
            source.as_deref(),
            *after,
            true,
            100,
            *poll_seconds,
        ),
        Command::Sources {
            command:
                automation::SourceCommand::Watch {
                    project_id,
                    source_id,
                    after,
                    poll_seconds,
                },
        } => (
            project_id,
            Some(source_id.as_str()),
            *after,
            true,
            100,
            *poll_seconds,
        ),
        _ => return Ok(false),
    };
    if after.is_some_and(|a| a < 0) {
        bail!("after must be >= 0");
    }
    let url = reqwest::Url::parse(base)?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        bail!("observation feeds require the trusted loopback HTTP server");
    }
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()?;
    if !watch {
        let response = page(
            &client,
            &url,
            project,
            source,
            after.unwrap_or(0),
            limit,
            false,
        )
        .await
        .map_err(|_| anyhow::anyhow!("feed unavailable or access denied"))?;
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(true);
    }
    let mut cursor = after;
    let mut channels = None;
    let mut delay = 0;
    let mut backoff = 1;
    let interrupted = tokio::signal::ctrl_c();
    tokio::pin!(interrupted);
    loop {
        // Reads are safe to retry. Never resume/start a Source or acknowledge an Agent input.
        let result = tokio::select! {
            _ = &mut interrupted => {
                if machine {output(&json!({"event":"stopped","after":cursor}))?;}
                else {eprintln!("View closed. Resume with --after {}.",cursor.unwrap_or(0));}
                return Ok(true);
            },
            result = async {
                if delay>0 { sleep(Duration::from_secs(delay)).await; }
                page(&client,&url,project,source,cursor.unwrap_or(0),100,cursor.is_none()).await
            } => result,
        };
        match result {
            Ok(page) => {
                if cursor.is_none() {
                    cursor = Some(page.next_after);
                }
                let serialized = serde_json::to_string(&page.channels)?;
                if channels.as_ref() != Some(&serialized) {
                    show_channels(&page.channels, cursor.unwrap_or(0), machine)?;
                    channels = Some(serialized);
                }
                for record in &page.records {
                    if machine {
                        output(
                            &json!({"event":"observation","record":record,"after":record.sequence}),
                        )?;
                    } else {
                        let name = page
                            .channels
                            .iter()
                            .find(|c| c.source_id == record.source_id)
                            .map_or(record.source_id.as_str(), |c| c.name.as_str());
                        println!(
                            "#{} {} [{}] {}",
                            record.sequence,
                            record.occurred_at,
                            terminal_text(name),
                            terminal_text(&record.body.to_string())
                        );
                        std::io::stdout().flush()?;
                    }
                }
                cursor = Some(page.next_after);
                backoff = 1;
                delay = if page.has_more { 0 } else { interval };
            }
            Err(FetchFailure { retryable: true }) => {
                if machine {
                    output(
                        &json!({"event":"reconnecting","after":cursor,"retry_seconds":backoff}),
                    )?;
                } else {
                    eprintln!(
                        "Feed temporarily unavailable; retrying in {backoff}s, after {:?}.",
                        cursor
                    );
                }
                delay = backoff;
                backoff = (backoff * 2).min(30);
            }
            Err(_) => bail!(
                "feed access denied, registration unavailable, or invalid response; stopped after {:?}",
                cursor
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::terminal_text;
    #[test]
    fn terminal_view_preserves_unicode_but_never_emits_controls() {
        let text = terminal_text("杨睿仪\n\x1b[2J\t反馈");
        assert!(text.contains("杨睿仪"));
        assert!(text.contains("反馈"));
        assert!(!text.chars().any(char::is_control));
    }
    #[test]
    fn human_view_has_a_bounded_preview() {
        assert_eq!(terminal_text(&"文".repeat(5000)).chars().count(), 4096);
    }
}
