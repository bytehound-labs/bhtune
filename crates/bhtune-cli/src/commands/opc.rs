//! `bhtune opc servers/read/write/browse/search`: thin passthrough diagnostics directly over
//! [`bhtune_driver::OpcDaDriver`], independent of running a full tune. Useful for checking
//! gateway connectivity and confirming tag names before starting a real test.

use bhtune_driver::{
    BrowseNode, BrowseNodeKind, BrowsePage, BrowsePageRequest, Driver, OpcDaDriver, Quality,
    SearchEvent, SearchIndexControlAction, SearchIndexRequest, SearchIndexResponse,
    SearchIndexStatus, SearchMatch, SearchRequest, TagWrite, close_opcda_browse_session,
    list_opcda_servers,
};

use crate::args::{OpcCommand, OpcSearchMatchModeArg, SearchIndexCommand};
use crate::output::OutputFormat;

pub async fn run(command: OpcCommand, config: &crate::config::BhtuneConfig) -> anyhow::Result<()> {
    run_with_output(command, config, OutputFormat::Table).await
}

pub async fn run_with_output(
    command: OpcCommand,
    config: &crate::config::BhtuneConfig,
    output: OutputFormat,
) -> anyhow::Result<()> {
    match command {
        OpcCommand::Servers { bridge_host } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            servers_with_output(&bridge_host, output).await
        }
        OpcCommand::Read {
            bridge_host,
            server,
            tags,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            let server = crate::config::resolve_server(server, config)?;
            read_with_output(&bridge_host, &server, &tags, output).await
        }
        OpcCommand::Write {
            bridge_host,
            server,
            tag,
            value,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            let server = crate::config::resolve_server(server, config)?;
            write_with_output(&bridge_host, &server, &tag, &value, output).await
        }
        OpcCommand::Browse {
            bridge_host,
            server,
            session_id,
            parent_node_key,
            page_token,
            page_size,
            all,
            refresh,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            let server = crate::config::resolve_server(server, config)?;
            browse_with_output(
                &bridge_host,
                &server,
                BrowseOptions {
                    session_id,
                    parent_node_key,
                    page_token,
                    page_size,
                    all,
                    refresh,
                },
                output,
            )
            .await
        }
        OpcCommand::Close {
            bridge_host,
            session_id,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            close_with_output(&bridge_host, &session_id, output).await
        }
        OpcCommand::Search {
            bridge_host,
            server,
            query,
            match_mode,
            max_results,
            session_id,
            scope_node_key,
            include_branches,
            refresh,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            let server = crate::config::resolve_server(server, config)?;
            search_with_output(
                &bridge_host,
                &server,
                SearchOptions {
                    query,
                    match_mode,
                    max_results,
                    session_id,
                    scope_node_key,
                    include_branches,
                    refresh,
                },
                output,
            )
            .await
        }
        OpcCommand::SearchIndex { command } => {
            run_search_index_command(command, config, output).await
        }
    }
}

async fn run_search_index_command(
    command: SearchIndexCommand,
    config: &crate::config::BhtuneConfig,
    output: OutputFormat,
) -> anyhow::Result<()> {
    match command {
        SearchIndexCommand::Status {
            bridge_host,
            server,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            let server = crate::config::resolve_server(server, config)?;
            let driver = OpcDaDriver::connect(&bridge_host, &server).await?;
            let status = driver.search_index_status().await?;
            print_search_index_status(&status, output)
        }
        SearchIndexCommand::Search {
            bridge_host,
            server,
            query,
            match_mode,
            max_results,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            let server = crate::config::resolve_server(server, config)?;
            let driver = OpcDaDriver::connect(&bridge_host, &server).await?;
            let response = driver
                .search_index(SearchIndexRequest::new(
                    query,
                    match_mode.into(),
                    max_results,
                ))
                .await?;
            print_search_index_results(&response, output)
        }
        SearchIndexCommand::Refresh {
            bridge_host,
            server,
            force,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            let server = crate::config::resolve_server(server, config)?;
            let driver = OpcDaDriver::connect(&bridge_host, &server).await?;
            let status = driver.refresh_search_index(force).await?;
            print_search_index_status(&status, output)
        }
        SearchIndexCommand::Control {
            bridge_host,
            server,
            action,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            let server = crate::config::resolve_server(server, config)?;
            let driver = OpcDaDriver::connect(&bridge_host, &server).await?;
            let status = driver
                .control_search_index(SearchIndexControlAction::from(action))
                .await?;
            print_search_index_status(&status, output)
        }
    }
}

#[cfg(test)]
async fn servers(bridge_host: &str) -> anyhow::Result<()> {
    servers_with_output(bridge_host, OutputFormat::Table).await
}

async fn servers_with_output(bridge_host: &str, output: OutputFormat) -> anyhow::Result<()> {
    let servers = list_opcda_servers(bridge_host).await?;
    if output == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "servers": servers }))?
        );
        return Ok(());
    }
    if servers.is_empty() {
        println!("No OPC DA servers registered on the gateway's host.");
        return Ok(());
    }
    for server in servers {
        println!("{server}");
    }
    Ok(())
}

#[cfg(test)]
async fn read(bridge_host: &str, server: &str, tags: &[String]) -> anyhow::Result<()> {
    read_with_output(bridge_host, server, tags, OutputFormat::Table).await
}

async fn read_with_output(
    bridge_host: &str,
    server: &str,
    tags: &[String],
    output: OutputFormat,
) -> anyhow::Result<()> {
    if tags.is_empty() {
        anyhow::bail!("at least one tag is required");
    }
    let driver = OpcDaDriver::connect(bridge_host, server).await?;
    let values = driver.read(tags).await?;
    if output == OutputFormat::Json {
        let values = values
            .into_iter()
            .map(|value| {
                serde_json::json!({
                    "tag": value.tag,
                    "value": value.value,
                    "quality": quality_name(value.quality),
                    "timestamp": value.timestamp.map(|time| time.to_rfc3339()),
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "values": values }))?
        );
        return Ok(());
    }
    println!(
        "{:<40} {:<15} {:<10} {:<20}",
        "TAG", "VALUE", "QUALITY", "TIMESTAMP"
    );
    for v in values {
        println!(
            "{:<40} {:<15} {:<10} {:<20}",
            v.tag,
            v.value,
            format!("{:?}", v.quality),
            format_timestamp(v.timestamp),
        );
    }
    Ok(())
}

#[cfg(test)]
async fn write(bridge_host: &str, server: &str, tag: &str, value: &str) -> anyhow::Result<()> {
    write_with_output(bridge_host, server, tag, value, OutputFormat::Table).await
}

async fn write_with_output(
    bridge_host: &str,
    server: &str,
    tag: &str,
    value: &str,
    output: OutputFormat,
) -> anyhow::Result<()> {
    let driver = OpcDaDriver::connect(bridge_host, server).await?;
    // Numeric-looking values are written as floats (matching a live process value or PID
    // constant write); anything else is written raw (e.g. a mode code like "MAN").
    let write_value = match value.parse::<f32>() {
        Ok(f) => TagWrite::Float(f),
        Err(_) => TagWrite::Raw(value.to_string()),
    };
    let outcome = driver.write(&tag.to_string(), write_value).await?;
    if output == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "tag": tag,
                "value": value,
                "success": outcome.success,
                "error": outcome.error_message,
            }))?
        );
        if !outcome.success {
            anyhow::bail!(
                "driver rejected the write: {}",
                outcome
                    .error_message
                    .unwrap_or_else(|| "unknown reason".to_string())
            );
        }
        return Ok(());
    }
    if outcome.success {
        println!("Wrote '{value}' to '{tag}'.");
        Ok(())
    } else {
        anyhow::bail!(
            "driver rejected the write: {}",
            write_rejection_reason(outcome.error_message)
        )
    }
}

fn format_timestamp(timestamp: Option<chrono::DateTime<chrono::Utc>>) -> String {
    timestamp
        .map(|timestamp| timestamp.to_rfc3339())
        .unwrap_or_else(|| "-".to_string())
}

fn write_rejection_reason(error_message: Option<String>) -> String {
    error_message.unwrap_or_else(|| "unknown reason".to_string())
}

#[cfg(test)]
async fn browse(bridge_host: &str, server: &str, _path: &str) -> anyhow::Result<()> {
    browse_with_output(
        bridge_host,
        server,
        BrowseOptions {
            page_size: bhtune_driver::DEFAULT_PAGE_SIZE,
            ..BrowseOptions::default()
        },
        OutputFormat::Table,
    )
    .await
}

#[derive(Debug, Default)]
struct BrowseOptions {
    session_id: Option<String>,
    parent_node_key: Option<String>,
    page_token: Option<String>,
    page_size: u32,
    all: bool,
    refresh: bool,
}

#[derive(serde::Serialize)]
struct BrowsePagesOutput {
    session_id: String,
    pages: Vec<serde_json::Value>,
    complete: bool,
}

async fn browse_with_output(
    bridge_host: &str,
    server: &str,
    options: BrowseOptions,
    output: OutputFormat,
) -> anyhow::Result<()> {
    if (options.parent_node_key.is_some() || options.page_token.is_some())
        && options.session_id.is_none()
    {
        anyhow::bail!("--session-id is required with --parent-node-key or --page-token");
    }

    let driver = OpcDaDriver::connect(bridge_host, server).await?;
    let first_request = BrowsePageRequest {
        session_id: options.session_id,
        parent_node_key: options.parent_node_key.clone(),
        page_token: options.page_token,
        page_size: options.page_size,
        refresh: options.refresh,
    };
    let first = driver.browse(first_request).await?;
    let session = first.session_id.clone();
    let pages = collect_browse_pages(
        &driver,
        first,
        options.parent_node_key,
        options.page_size,
        options.all,
    )
    .await?;
    if output == OutputFormat::Json {
        if pages.len() == 1 {
            println!(
                "{}",
                serde_json::to_string_pretty(&json_browse_page(&pages[0]))?
            );
        } else {
            let response = BrowsePagesOutput {
                session_id: session,
                pages: pages.iter().map(json_browse_page).collect(),
                complete: pages.last().is_some_and(|page| page.complete),
            };
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        return Ok(());
    }
    for (index, page) in pages.iter().enumerate() {
        if pages.len() > 1 {
            println!("Page {}:", index + 1);
        }
        if page.nodes.is_empty() {
            println!("No tags found at this level.");
        }
        for node in &page.nodes {
            let marker = match node.kind {
                BrowseNodeKind::Branch => "[+]",
                BrowseNodeKind::Item => "   ",
                BrowseNodeKind::BranchAndItem => "[*]",
                BrowseNodeKind::Unspecified => "[?]",
            };
            let item = node
                .item_id
                .as_deref()
                .map(|id| format!(" -> {id}"))
                .unwrap_or_default();
            println!("{marker} {}{item}", node.display_name);
        }
        if let Some(token) = &page.next_page_token {
            println!("More pages available (next token: {token}).");
        }
    }
    Ok(())
}

async fn close_with_output(
    bridge_host: &str,
    session_id: &str,
    output: OutputFormat,
) -> anyhow::Result<()> {
    if session_id.trim().is_empty() {
        anyhow::bail!("a browse session ID is required");
    }
    close_opcda_browse_session(bridge_host, session_id).await?;
    if output == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "session_id": session_id,
                "closed": true,
            }))?
        );
    } else {
        println!("Closed browse session '{session_id}'.");
    }
    Ok(())
}

async fn collect_browse_pages(
    driver: &OpcDaDriver,
    first: BrowsePage,
    parent_node_key: Option<String>,
    page_size: u32,
    all: bool,
) -> anyhow::Result<Vec<BrowsePage>> {
    const MAX_PAGES: usize = 10_000;
    let mut pages = vec![first];
    if !all {
        return Ok(pages);
    }
    while let Some(token) = pages.last().and_then(|page| page.next_page_token.clone()) {
        if pages.len() >= MAX_PAGES {
            anyhow::bail!("browse continuation exceeded the safety limit of {MAX_PAGES} pages");
        }
        let last = pages.last().expect("pages always contains the first page");
        let page = driver
            .browse(BrowsePageRequest::next(
                last.session_id.clone(),
                parent_node_key.clone(),
                token,
                page_size,
            ))
            .await?;
        pages.push(page);
    }
    Ok(pages)
}

fn json_browse_page(page: &BrowsePage) -> serde_json::Value {
    serde_json::json!({
        "session_id": page.session_id,
        "nodes": page.nodes.iter().map(json_browse_node).collect::<Vec<_>>(),
        "next_page_token": page.next_page_token,
        "complete": page.complete,
        "organization": format!("{:?}", page.organization).to_lowercase(),
        "source": format!("{:?}", page.source).to_lowercase(),
        "warning": page.warning,
    })
}

fn json_browse_node(node: &BrowseNode) -> serde_json::Value {
    serde_json::json!({
        "node_key": node.node_key,
        "display_name": node.display_name,
        "kind": browse_node_kind_name(node.kind),
        "item_id": node.item_id,
    })
}

fn browse_node_kind_name(kind: BrowseNodeKind) -> &'static str {
    match kind {
        BrowseNodeKind::Unspecified => "unspecified",
        BrowseNodeKind::Branch => "branch",
        BrowseNodeKind::Item => "item",
        BrowseNodeKind::BranchAndItem => "branch_and_item",
    }
}

#[derive(Debug)]
struct SearchOptions {
    query: String,
    match_mode: OpcSearchMatchModeArg,
    max_results: u32,
    session_id: Option<String>,
    scope_node_key: Option<String>,
    include_branches: bool,
    refresh: bool,
}

async fn search_with_output(
    bridge_host: &str,
    server: &str,
    options: SearchOptions,
    output: OutputFormat,
) -> anyhow::Result<()> {
    let driver = OpcDaDriver::connect(bridge_host, server).await?;
    let request = SearchRequest {
        query: options.query,
        match_mode: options.match_mode.into(),
        session_id: options.session_id,
        scope_node_key: options.scope_node_key,
        max_results: options.max_results,
        include_branches: options.include_branches,
        refresh: options.refresh,
    };
    let mut stream = driver.search_stream(request).await?;
    let mut matches = Vec::new();
    let mut progress = Vec::new();
    let mut completed = None;
    while let Some(event) = stream.next().await? {
        match event {
            SearchEvent::Match(found) => matches.push(found),
            SearchEvent::Progress(current) => {
                eprintln!(
                    "search progress: visited={} matches={}{}",
                    current.visited_nodes,
                    current.matches,
                    if current.partial { " (partial)" } else { "" }
                );
                progress.push(current);
            }
            SearchEvent::Completed(current) => completed = Some(current),
        }
    }
    if output == OutputFormat::Json {
        let matches = matches.iter().map(json_search_match).collect::<Vec<_>>();
        let progress = progress
            .iter()
            .map(|current| {
                serde_json::json!({
                    "visited_nodes": current.visited_nodes,
                    "matches": current.matches,
                    "partial": current.partial,
                })
            })
            .collect::<Vec<_>>();
        let completed = completed.map(|current| {
            serde_json::json!({
                "complete": current.complete,
                "cancelled": current.cancelled,
                "truncated": current.truncated,
                "warning": current.warning,
            })
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "matches": matches,
                "progress": progress,
                "completed": completed,
            }))?
        );
        return Ok(());
    }
    if matches.is_empty() {
        println!("No matching tags found.");
    } else {
        for found in matches {
            let item = found.node.item_id.as_deref().unwrap_or("-");
            let breadcrumb = found
                .breadcrumbs
                .iter()
                .map(|part| part.display_name.as_str())
                .collect::<Vec<_>>()
                .join("/");
            println!("{item}\t{breadcrumb}");
        }
    }
    let _ = completed.map(|done| {
        if let Some(warning) = done.warning {
            eprintln!("search warning: {warning}");
        }
        if done.truncated {
            eprintln!("search results were truncated at the requested maximum");
        }
    });
    Ok(())
}

fn json_search_match(found: &SearchMatch) -> serde_json::Value {
    serde_json::json!({
        "node": json_browse_node(&found.node),
        "breadcrumbs": found.breadcrumbs.iter().map(|part| {
            serde_json::json!({
                "node_key": part.node_key,
                "display_name": part.display_name,
            })
        }).collect::<Vec<_>>(),
    })
}

fn print_search_index_status(
    status: &SearchIndexStatus,
    output: OutputFormat,
) -> anyhow::Result<()> {
    if output == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json_search_index_status(status))?
        );
        return Ok(());
    }
    println!("Server: {}", status.server);
    println!("State: {}", status.state);
    println!("Configured: {}", status.configured);
    println!("Active generation: {}", status.active_generation);
    println!("Entries: {}", status.entry_count);
    println!("Unique items: {}", status.unique_item_count);
    if let Some(progress) = &status.progress {
        println!(
            "Progress: {} entries, {:.1} items/s",
            progress.entries_seen, progress.items_per_second
        );
    }
    status.last_error.iter().for_each(|diagnostic| {
        let label = if status.state != bhtune_driver::SearchIndexState::Failed {
            "Last warning"
        } else {
            "Last error"
        };
        println!("{label}: {diagnostic}");
    });
    Ok(())
}

fn print_search_index_results(
    response: &SearchIndexResponse,
    output: OutputFormat,
) -> anyhow::Result<()> {
    if output == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "matches": response.matches.iter().map(json_indexed_search_match).collect::<Vec<_>>(),
                "has_more": response.has_more,
                "status": json_search_index_status(&response.status),
            }))?
        );
        return Ok(());
    }
    if response.matches.is_empty() {
        println!(
            "No matching tags found (index state: {}).",
            response.status.state
        );
    } else {
        for found in &response.matches {
            println!("{}\t{}", found.item_id, found.breadcrumbs.join("/"));
        }
    }
    if response.has_more {
        eprintln!("More than the requested number of matches exist; refine the query.");
    }
    if response.status.state != bhtune_driver::SearchIndexState::Ready {
        eprintln!(
            "Search results come from an {} namespace index.",
            response.status.state
        );
    }
    Ok(())
}

fn json_search_index_status(status: &SearchIndexStatus) -> serde_json::Value {
    serde_json::json!({
        "server": status.server,
        "state": status.state.to_string(),
        "configured": status.configured,
        "active_generation": status.active_generation,
        "entry_count": status.entry_count,
        "unique_item_count": status.unique_item_count,
        "started_at": status.started_at,
        "completed_at": status.completed_at,
        "last_error": status.last_error,
        "database_bytes": status.database_bytes,
        "organization": format!("{:?}", status.organization).to_lowercase(),
        "source": format!("{:?}", status.source).to_lowercase(),
        "progress": status.progress.as_ref().map(|progress| serde_json::json!({
            "branches_visited": progress.branches_visited,
            "entries_seen": progress.entries_seen,
            "unique_items": progress.unique_items,
            "active_time_ms": progress.active_time_ms,
            "paused_time_ms": progress.paused_time_ms,
            "items_per_second": progress.items_per_second,
            "estimated_remaining_ms": progress.estimated_remaining_ms,
        })),
    })
}

fn json_indexed_search_match(found: &bhtune_driver::IndexedSearchMatch) -> serde_json::Value {
    serde_json::json!({
        "item_id": found.item_id,
        "display_name": found.display_name,
        "kind": browse_node_kind_name(found.kind),
        "breadcrumbs": found.breadcrumbs,
    })
}

fn quality_name(quality: Quality) -> &'static str {
    match quality {
        Quality::Good => "good",
        Quality::Uncertain => "uncertain",
        Quality::Bad => "bad",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::SearchIndexControlActionArg;
    use crate::test_support::{MockBridgeService, start_mock_server};
    use opcda_bridge_proto::bridge::{
        BrowseNode, BrowseNodeKind, BrowsePage, ListServersResponse, ReadResponse, SearchCompleted,
        SearchEvent as ProtoSearchEvent, SearchIndexResponse as ProtoSearchIndexResponse,
        SearchIndexState as ProtoSearchIndexState, SearchIndexStatus as ProtoSearchIndexStatus,
        SearchProgress, TagValue as ProtoTagValue, WriteResponse, search_event,
    };

    #[tokio::test]
    async fn servers_prints_every_registered_server_from_a_mock_gateway() {
        let (host, server) = start_mock_server(MockBridgeService {
            list_servers_response: ListServersResponse {
                servers: vec![
                    "Matrikon.OPC.Simulation.1".to_string(),
                    "Kepware.KEPServerEX.V6".to_string(),
                ],
            },
            ..Default::default()
        })
        .await;

        servers(&host).await.unwrap();

        server.shutdown().await;
    }

    #[tokio::test]
    async fn servers_handles_an_empty_result() {
        let (host, server) = start_mock_server(MockBridgeService::default()).await;
        servers(&host).await.unwrap();
        server.shutdown().await;
    }

    #[tokio::test]
    async fn servers_connect_failure_surfaces_as_an_error() {
        let err = servers("127.0.0.1:1").await.unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn read_prints_values_from_a_mock_gateway() {
        let (host, server) = start_mock_server(MockBridgeService {
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "Unit1.LIC101.PV".to_string(),
                    value: "42.5".to_string(),
                    quality: "Good".to_string(),
                    timestamp: "2024-01-15 10:23:45".to_string(),
                }],
            },
            ..Default::default()
        })
        .await;

        read(&host, "Sim.Server", &["Unit1.LIC101.PV".to_string()])
            .await
            .unwrap();

        server.shutdown().await;
    }

    #[tokio::test]
    async fn read_requires_at_least_one_tag() {
        let err = read("127.0.0.1:1", "Sim.Server", &[]).await.unwrap_err();
        assert!(err.to_string().contains("at least one tag"));
    }

    #[tokio::test]
    async fn write_reports_success_from_a_mock_gateway() {
        let (host, server) = start_mock_server(MockBridgeService {
            write_response: WriteResponse {
                tag_id: "Unit1.LIC101.OP".to_string(),
                success: true,
                error: None,
            },
            ..Default::default()
        })
        .await;

        write(&host, "Sim.Server", "Unit1.LIC101.OP", "55.0")
            .await
            .unwrap();

        server.shutdown().await;
    }

    #[tokio::test]
    async fn write_surfaces_a_rejected_write_as_an_error() {
        let (host, server) = start_mock_server(MockBridgeService {
            write_response: WriteResponse {
                tag_id: "Unit1.LIC101.OP".to_string(),
                success: false,
                error: Some("tag is read-only".to_string()),
            },
            ..Default::default()
        })
        .await;

        let err = write(&host, "Sim.Server", "Unit1.LIC101.OP", "55.0")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("read-only"));

        server.shutdown().await;
    }

    #[tokio::test]
    async fn write_reports_a_gateway_rejection_when_it_omits_a_reason() {
        let (host, server) = start_mock_server(MockBridgeService {
            write_response: WriteResponse {
                tag_id: "Unit1.LIC101.OP".to_string(),
                success: false,
                error: None,
            },
            ..Default::default()
        })
        .await;

        let err = write(&host, "Sim.Server", "Unit1.LIC101.OP", "55.0")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("driver rejected the write"));

        server.shutdown().await;
    }

    #[test]
    fn diagnostic_formatters_cover_timestamp_and_reason_fallbacks() {
        assert_eq!(format_timestamp(None), "-");
        assert_eq!(
            format_timestamp(Some(
                chrono::DateTime::parse_from_rfc3339("2024-01-15T10:23:45Z")
                    .unwrap()
                    .into()
            )),
            "2024-01-15T10:23:45+00:00"
        );
        assert_eq!(write_rejection_reason(None), "unknown reason");
        assert_eq!(
            write_rejection_reason(Some("read-only".to_string())),
            "read-only"
        );
    }

    #[tokio::test]
    async fn write_accepts_a_raw_non_numeric_value() {
        let (host, server) = start_mock_server(MockBridgeService {
            write_response: WriteResponse {
                tag_id: "Unit1.LIC101.OP".to_string(),
                success: true,
                error: None,
            },
            ..Default::default()
        })
        .await;

        write(&host, "Sim.Server", "Unit1.LIC101.MODE", "MAN")
            .await
            .unwrap();

        server.shutdown().await;
    }

    #[tokio::test]
    async fn browse_prints_nodes_from_a_mock_gateway() {
        let (host, server) = start_mock_server(MockBridgeService {
            browse_response: BrowsePage {
                session_id: "session".to_string(),
                nodes: vec![BrowseNode {
                    node_key: "unit1".to_string(),
                    display_name: "Unit1".to_string(),
                    kind: BrowseNodeKind::Branch as i32,
                    item_id: None,
                }],
                complete: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .await;

        browse(&host, "Sim.Server", "").await.unwrap();

        server.shutdown().await;
    }

    #[tokio::test]
    async fn browse_handles_an_empty_result() {
        let (host, server) = start_mock_server(MockBridgeService::default()).await;
        browse(&host, "Sim.Server", "Unit1").await.unwrap();
        server.shutdown().await;
    }

    #[tokio::test]
    async fn browse_connect_failure_surfaces_as_an_error() {
        let err = browse("127.0.0.1:1", "Sim.Server", "").await.unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn connect_failure_surfaces_as_an_error() {
        // Port 1 is a privileged/unlikely-bound port; connecting should fail promptly.
        let err = read(
            "127.0.0.1:1",
            "Sim.Server",
            &["Unit1.LIC101.PV".to_string()],
        )
        .await
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn run_dispatches_servers_read_write_and_browse() {
        let close_browse_session_calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (host, server) = start_mock_server(MockBridgeService {
            list_servers_response: ListServersResponse {
                servers: vec!["Matrikon.OPC.Simulation.1".to_string()],
            },
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "Unit1.LIC101.PV".to_string(),
                    value: "42.5".to_string(),
                    quality: "Good".to_string(),
                    timestamp: "2024-01-15 10:23:45".to_string(),
                }],
            },
            write_response: WriteResponse {
                tag_id: "Unit1.LIC101.OP".to_string(),
                success: true,
                error: None,
            },
            browse_response: BrowsePage {
                session_id: "session".to_string(),
                complete: true,
                ..Default::default()
            },
            close_browse_session_calls: close_browse_session_calls.clone(),
            ..Default::default()
        })
        .await;
        let config = crate::config::BhtuneConfig::default();

        run(
            OpcCommand::Servers {
                bridge_host: Some(host.clone()),
            },
            &config,
        )
        .await
        .unwrap();

        run(
            OpcCommand::Read {
                bridge_host: Some(host.clone()),
                server: Some("Sim.Server".to_string()),
                tags: vec!["Unit1.LIC101.PV".to_string()],
            },
            &config,
        )
        .await
        .unwrap();

        run(
            OpcCommand::Write {
                bridge_host: Some(host.clone()),
                server: Some("Sim.Server".to_string()),
                tag: "Unit1.LIC101.OP".to_string(),
                value: "55.0".to_string(),
            },
            &config,
        )
        .await
        .unwrap();

        run(
            OpcCommand::Browse {
                bridge_host: Some(host.clone()),
                server: Some("Sim.Server".to_string()),
                session_id: None,
                parent_node_key: None,
                page_token: None,
                page_size: bhtune_driver::DEFAULT_PAGE_SIZE,
                all: false,
                refresh: false,
            },
            &config,
        )
        .await
        .unwrap();
        assert_eq!(
            close_browse_session_calls.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        run(
            OpcCommand::Close {
                bridge_host: Some(host),
                session_id: "session".to_string(),
            },
            &config,
        )
        .await
        .unwrap();
        assert_eq!(
            close_browse_session_calls.load(std::sync::atomic::Ordering::SeqCst),
            1
        );

        server.shutdown().await;
    }

    #[tokio::test]
    async fn run_resolves_bridge_host_and_server_from_config_when_cli_flags_are_unset() {
        let (host, server) = start_mock_server(MockBridgeService {
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "Unit1.LIC101.PV".to_string(),
                    value: "42.5".to_string(),
                    quality: "Good".to_string(),
                    timestamp: "2024-01-15 10:23:45".to_string(),
                }],
            },
            ..Default::default()
        })
        .await;
        let config = crate::config::BhtuneConfig {
            bridge_host: Some(host),
            server: Some("Sim.Server".to_string()),
            ..Default::default()
        };

        run(
            OpcCommand::Read {
                bridge_host: None,
                server: None,
                tags: vec!["Unit1.LIC101.PV".to_string()],
            },
            &config,
        )
        .await
        .unwrap();

        server.shutdown().await;
    }

    #[tokio::test]
    async fn run_resolves_bridge_host_from_config_for_servers_when_cli_flag_is_unset() {
        let (host, server) = start_mock_server(MockBridgeService::default()).await;
        let config = crate::config::BhtuneConfig {
            bridge_host: Some(host),
            ..Default::default()
        };

        run(OpcCommand::Servers { bridge_host: None }, &config)
            .await
            .unwrap();

        server.shutdown().await;
    }

    #[tokio::test]
    async fn run_errors_when_server_is_unset_in_both_cli_and_config() {
        let err = run(
            OpcCommand::Read {
                bridge_host: None,
                server: None,
                tags: vec!["Unit1.LIC101.PV".to_string()],
            },
            &crate::config::BhtuneConfig::default(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("no OPC server specified"));
    }

    #[tokio::test]
    async fn search_and_index_commands_cover_table_json_and_control_paths() {
        let status = ProtoSearchIndexStatus {
            server: "Sim.Server".into(),
            state: ProtoSearchIndexState::Partial as i32,
            configured: true,
            active_generation: 2,
            entry_count: 3,
            unique_item_count: 2,
            started_at: Some("2026-01-01T00:00:00Z".into()),
            completed_at: None,
            last_error: Some("inventory incomplete".into()),
            database_bytes: 42,
            progress: Some(opcda_bridge_proto::bridge::IndexedSearchProgress {
                branches_visited: 1,
                entries_seen: 3,
                unique_items: 2,
                active_time_ms: 10,
                paused_time_ms: 2,
                items_per_second: 1.5,
                estimated_remaining_ms: Some(5),
            }),
            ..Default::default()
        };
        let (host, server) = start_mock_server(MockBridgeService {
            search_events: vec![
                ProtoSearchEvent {
                    event: Some(search_event::Event::Progress(SearchProgress {
                        visited_nodes: 4,
                        matches: 1,
                        partial: true,
                    })),
                },
                ProtoSearchEvent {
                    event: Some(search_event::Event::Match(
                        opcda_bridge_proto::bridge::SearchMatch {
                            node: Some(BrowseNode {
                                node_key: "pv".into(),
                                display_name: "PV".into(),
                                kind: BrowseNodeKind::Item as i32,
                                item_id: Some("Area.PV".into()),
                            }),
                            breadcrumbs: vec![opcda_bridge_proto::bridge::BrowseBreadcrumb {
                                node_key: "area".into(),
                                display_name: "Area".into(),
                            }],
                        },
                    )),
                },
                ProtoSearchEvent {
                    event: Some(search_event::Event::Completed(SearchCompleted {
                        complete: false,
                        cancelled: false,
                        truncated: true,
                        warning: Some("partial result".into()),
                    })),
                },
            ],
            search_index_status_response: status.clone(),
            search_index_response: ProtoSearchIndexResponse {
                matches: vec![],
                has_more: true,
                status: Some(status),
            },
            ..Default::default()
        })
        .await;
        let config = crate::config::BhtuneConfig::default();

        run_with_output(
            OpcCommand::Search {
                bridge_host: Some(host.clone()),
                server: Some("Sim.Server".into()),
                query: "PV".into(),
                match_mode: OpcSearchMatchModeArg::Contains,
                max_results: 10,
                session_id: Some("session".into()),
                scope_node_key: Some("area".into()),
                include_branches: true,
                refresh: true,
            },
            &config,
            OutputFormat::Table,
        )
        .await
        .unwrap();
        run_with_output(
            OpcCommand::Search {
                bridge_host: Some(host.clone()),
                server: Some("Sim.Server".into()),
                query: "PV".into(),
                match_mode: OpcSearchMatchModeArg::Exact,
                max_results: 10,
                session_id: None,
                scope_node_key: None,
                include_branches: false,
                refresh: false,
            },
            &config,
            OutputFormat::Json,
        )
        .await
        .unwrap();

        for command in [
            SearchIndexCommand::Status {
                bridge_host: Some(host.clone()),
                server: Some("Sim.Server".into()),
            },
            SearchIndexCommand::Refresh {
                bridge_host: Some(host.clone()),
                server: Some("Sim.Server".into()),
                force: true,
            },
            SearchIndexCommand::Control {
                bridge_host: Some(host.clone()),
                server: Some("Sim.Server".into()),
                action: SearchIndexControlActionArg::Pause,
            },
        ] {
            run_with_output(
                OpcCommand::SearchIndex { command },
                &config,
                OutputFormat::Table,
            )
            .await
            .unwrap();
        }
        run_with_output(
            OpcCommand::SearchIndex {
                command: SearchIndexCommand::Search {
                    bridge_host: Some(host.clone()),
                    server: Some("Sim.Server".into()),
                    query: "PV".into(),
                    match_mode: OpcSearchMatchModeArg::Prefix,
                    max_results: 5,
                },
            },
            &config,
            OutputFormat::Json,
        )
        .await
        .unwrap();

        server.shutdown().await;
    }

    #[tokio::test]
    async fn diagnostic_commands_cover_json_and_validation_branches() {
        let status = bhtune_driver::SearchIndexStatus {
            server: "Sim.Server".into(),
            state: bhtune_driver::SearchIndexState::Failed,
            configured: true,
            active_generation: 1,
            entry_count: 2,
            unique_item_count: 1,
            started_at: None,
            completed_at: None,
            last_error: Some("failed".into()),
            database_bytes: 10,
            organization: bhtune_driver::NamespaceOrganization::Flat,
            source: bhtune_driver::BrowseSource::Flat,
            progress: Some(bhtune_driver::IndexedSearchProgress {
                branches_visited: 1,
                entries_seen: 2,
                unique_items: 1,
                active_time_ms: 1,
                paused_time_ms: 0,
                items_per_second: 2.0,
                estimated_remaining_ms: None,
            }),
        };
        let indexed = bhtune_driver::SearchIndexResponse {
            matches: vec![bhtune_driver::IndexedSearchMatch {
                item_id: "Area.PV".into(),
                display_name: "PV".into(),
                kind: bhtune_driver::BrowseNodeKind::BranchAndItem,
                breadcrumbs: vec!["Area".into()],
            }],
            has_more: true,
            status: status.clone(),
        };
        assert_eq!(quality_name(Quality::Good), "good");
        assert_eq!(quality_name(Quality::Uncertain), "uncertain");
        assert_eq!(quality_name(Quality::Bad), "bad");
        print_search_index_status(&status, OutputFormat::Table).unwrap();
        print_search_index_status(&status, OutputFormat::Json).unwrap();
        print_search_index_results(&indexed, OutputFormat::Table).unwrap();
        print_search_index_results(&indexed, OutputFormat::Json).unwrap();
        print_search_index_results(
            &bhtune_driver::SearchIndexResponse {
                matches: vec![],
                has_more: false,
                status,
            },
            OutputFormat::Table,
        )
        .unwrap();

        let host_service = MockBridgeService {
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "Area.PV".into(),
                    value: "1.5".into(),
                    quality: "Uncertain".into(),
                    timestamp: "N/A".into(),
                }],
            },
            write_response: WriteResponse {
                tag_id: "Area.MV".into(),
                success: false,
                error: None,
            },
            browse_response: BrowsePage {
                session_id: "s".into(),
                nodes: vec![
                    BrowseNode {
                        node_key: "u".into(),
                        display_name: "Unspecified".into(),
                        kind: BrowseNodeKind::Unspecified as i32,
                        item_id: None,
                    },
                    BrowseNode {
                        node_key: "i".into(),
                        display_name: "Item".into(),
                        kind: BrowseNodeKind::Item as i32,
                        item_id: Some("Area.PV".into()),
                    },
                    BrowseNode {
                        node_key: "b".into(),
                        display_name: "Branch".into(),
                        kind: BrowseNodeKind::Branch as i32,
                        item_id: None,
                    },
                    BrowseNode {
                        node_key: "both".into(),
                        display_name: "Both".into(),
                        kind: BrowseNodeKind::BranchAndItem as i32,
                        item_id: Some("Area.Both".into()),
                    },
                ],
                next_page_token: Some("next".into()),
                complete: false,
                ..Default::default()
            },
            browse_continuation_response: Some(BrowsePage {
                session_id: "s".into(),
                complete: true,
                ..Default::default()
            }),
            list_servers_response: ListServersResponse {
                servers: vec!["Sim.Server".into()],
            },
            ..Default::default()
        };
        let (host, server) = start_mock_server(host_service).await;
        servers_with_output(&host, OutputFormat::Json)
            .await
            .unwrap();
        read_with_output(&host, "Sim.Server", &["Area.PV".into()], OutputFormat::Json)
            .await
            .unwrap();
        write_with_output(&host, "Sim.Server", "Area.MV", "1.0", OutputFormat::Json)
            .await
            .unwrap_err();
        let err = write_with_output(
            &host,
            "Sim.Server",
            "Area.MV",
            "not-numeric",
            OutputFormat::Json,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("driver rejected"));
        browse_with_output(
            &host,
            "Sim.Server",
            BrowseOptions {
                page_size: 2,
                all: true,
                ..Default::default()
            },
            OutputFormat::Json,
        )
        .await
        .unwrap();
        browse_with_output(
            &host,
            "Sim.Server",
            BrowseOptions {
                page_size: 2,
                all: false,
                ..Default::default()
            },
            OutputFormat::Json,
        )
        .await
        .unwrap();
        browse_with_output(
            &host,
            "Sim.Server",
            BrowseOptions {
                page_size: 2,
                all: true,
                ..Default::default()
            },
            OutputFormat::Table,
        )
        .await
        .unwrap();
        browse_with_output(
            &host,
            "Sim.Server",
            BrowseOptions {
                page_size: 2,
                all: false,
                ..Default::default()
            },
            OutputFormat::Table,
        )
        .await
        .unwrap();
        let err = browse_with_output(
            &host,
            "Sim.Server",
            BrowseOptions {
                parent_node_key: Some("parent".into()),
                ..Default::default()
            },
            OutputFormat::Table,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("--session-id"));
        let err = close_with_output(&host, " ", OutputFormat::Table)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("session ID"));
        close_with_output(&host, "s", OutputFormat::Json)
            .await
            .unwrap();
        server.shutdown().await;

        let (host, server) = start_mock_server(MockBridgeService {
            write_response: WriteResponse {
                tag_id: "Area.MV".into(),
                success: true,
                error: None,
            },
            ..Default::default()
        })
        .await;
        write_with_output(&host, "Sim.Server", "Area.MV", "1.0", OutputFormat::Json)
            .await
            .unwrap();
        server.shutdown().await;
    }

    #[tokio::test]
    async fn table_search_reports_empty_results_and_completion_diagnostics() {
        let (host, server) = start_mock_server(MockBridgeService {
            search_events: vec![ProtoSearchEvent {
                event: Some(search_event::Event::Completed(SearchCompleted {
                    complete: false,
                    cancelled: false,
                    truncated: true,
                    warning: Some("partial".into()),
                })),
            }],
            ..Default::default()
        })
        .await;
        search_with_output(
            &host,
            "Sim.Server",
            SearchOptions {
                query: "PV".into(),
                match_mode: OpcSearchMatchModeArg::Contains,
                max_results: 1,
                session_id: None,
                scope_node_key: None,
                include_branches: false,
                refresh: false,
            },
            OutputFormat::Table,
        )
        .await
        .unwrap();
        server.shutdown().await;
    }

    #[tokio::test]
    async fn browse_all_stops_at_the_safety_page_limit() {
        let (host, server) = start_mock_server(MockBridgeService {
            browse_response: BrowsePage {
                session_id: "s".into(),
                next_page_token: Some("next".into()),
                complete: false,
                ..Default::default()
            },
            browse_continuation_response: Some(BrowsePage {
                session_id: "s".into(),
                next_page_token: Some("next".into()),
                complete: false,
                ..Default::default()
            }),
            ..Default::default()
        })
        .await;
        let err = browse_with_output(
            &host,
            "Sim.Server",
            BrowseOptions {
                all: true,
                ..Default::default()
            },
            OutputFormat::Table,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("safety limit"));
        server.shutdown().await;
    }
}
