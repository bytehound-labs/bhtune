//! `bhtune opc servers/read/write/browse/search`: thin passthrough diagnostics directly over
//! [`bhtune_driver::OpcDaDriver`], independent of running a full tune. Useful for checking
//! gateway connectivity and confirming tag names before starting a real test.

use bhtune_db::SqlitePool;
use bhtune_db::models::LiveOperationKind;
use bhtune_driver::{
    BrowseNode, BrowseNodeKind, BrowsePage, BrowsePageRequest, Driver, OpcDaDriver,
    OpcDaGatewayInfo, Quality, SearchEvent, SearchIndexControlAction, SearchIndexRequest,
    SearchIndexResponse, SearchIndexStatus, SearchMatch, SearchRequest, TagWrite, WriteOutcome,
    check_gateway_compatibility, close_opcda_browse_session, get_opcda_gateway_info,
    list_opcda_servers,
};
use bhtune_runtime::live_ownership::{AuditedDriver, LiveOperationGuard};

use crate::args::{OpcCommand, OpcSearchMatchModeArg, SearchIndexCommand};
use crate::output::OutputFormat;

pub async fn run(
    pool: &SqlitePool,
    command: OpcCommand,
    config: &crate::config::BhtuneConfig,
) -> anyhow::Result<()> {
    run_with_output(pool, command, config, OutputFormat::Table).await
}

pub async fn run_with_output(
    pool: &SqlitePool,
    command: OpcCommand,
    config: &crate::config::BhtuneConfig,
    output: OutputFormat,
) -> anyhow::Result<()> {
    match command {
        OpcCommand::GatewayInfo { bridge_host } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            gateway_info_with_output(&bridge_host, output).await
        }
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
            write_with_output(pool, &bridge_host, &server, &tag, &value, output).await
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
async fn gateway_info(bridge_host: &str) -> anyhow::Result<()> {
    gateway_info_with_output(bridge_host, OutputFormat::Table).await
}

async fn gateway_info_with_output(bridge_host: &str, output: OutputFormat) -> anyhow::Result<()> {
    let info = get_opcda_gateway_info(bridge_host).await?;
    let compatibility = check_gateway_compatibility(bridge_host, None).await?;
    if output == OutputFormat::Json {
        let mut value = gateway_info_json(&info);
        value["compatibility"] = serde_json::to_value(&compatibility)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!("Application version: {}", info.application_version);
    println!(
        "Compatibility schema: {}",
        info.compatibility_schema_version
    );
    for feature in info.features {
        println!(
            "{}: {}-{}",
            feature.feature.as_str(),
            feature.min_version,
            feature.max_version
        );
    }
    println!("Compatibility: {}", compatibility.status.as_str());
    if let Some(warning) = compatibility.warning_message() {
        println!("Compatibility warning: {warning}");
    }
    Ok(())
}

fn gateway_info_json(info: &OpcDaGatewayInfo) -> serde_json::Value {
    serde_json::json!({
        "application_version": info.application_version,
        "compatibility_schema_version": info.compatibility_schema_version,
        "features": info.features.iter().map(|feature| {
            serde_json::json!({
                "feature": feature.feature.as_str(),
                "min_version": feature.min_version,
                "max_version": feature.max_version,
            })
        }).collect::<Vec<_>>(),
    })
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
    let directory = tempfile::tempdir()?;
    let pool = bhtune_db::connect(&directory.path().join("opc-write-test.db")).await?;
    write_with_output(&pool, bridge_host, server, tag, value, OutputFormat::Table).await
}

async fn write_with_output(
    pool: &SqlitePool,
    bridge_host: &str,
    server: &str,
    tag: &str,
    value: &str,
    output: OutputFormat,
) -> anyhow::Result<()> {
    // Numeric-looking values are written as floats (matching a live process value or PID
    // constant write); anything else is written raw (e.g. a mode code like "MAN").
    let write_value = match value.parse::<f32>() {
        Ok(f) => TagWrite::Float(f),
        Err(_) => TagWrite::Raw(value.to_string()),
    };
    let target = match &write_value {
        TagWrite::Float(value) => serde_json::json!(value),
        TagWrite::Raw(value) => serde_json::json!(value),
    };
    let restore_intent = serde_json::to_string(&serde_json::json!({
        "version": 1,
        "kind": "opc_write",
        "tag": tag,
        "target": target,
        "state": "pending",
    }))?;
    let ownership = LiveOperationGuard::acquire(
        pool,
        None,
        LiveOperationKind::OpcWrite,
        bridge_host,
        server,
        tag,
        Some(restore_intent),
    )
    .await?;
    let operation_result = async {
        let driver = OpcDaDriver::connect(bridge_host, server).await?;
        let audited = AuditedDriver::new(&driver, pool, &ownership, None);
        let outcome = audited
            .write(&tag.to_string(), write_value)
            .await
            .map_err(anyhow::Error::from)?;
        anyhow::Ok(outcome)
    }
    .await;
    let release_result = ownership.release().await;
    let outcome = combine_write_and_release(operation_result, release_result)?;
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

fn combine_write_and_release(
    operation_result: anyhow::Result<WriteOutcome>,
    release_result: anyhow::Result<()>,
) -> anyhow::Result<WriteOutcome> {
    match (operation_result, release_result) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(anyhow::anyhow!(
            "OPC write completed but live ownership release was not persisted: {error}"
        )),
        (Err(operation_error), Err(release_error)) => Err(anyhow::anyhow!(
            "OPC write failed ({operation_error}); live ownership release also failed: {release_error}"
        )),
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
    validate_browse_options(&options)?;

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
    render_browse_pages(&pages, &session, output)
}

fn validate_browse_options(options: &BrowseOptions) -> anyhow::Result<()> {
    if (options.parent_node_key.is_some() || options.page_token.is_some())
        && options.session_id.is_none()
    {
        anyhow::bail!("--session-id is required with --parent-node-key or --page-token");
    }
    Ok(())
}

fn render_browse_pages(
    pages: &[BrowsePage],
    session_id: &str,
    output: OutputFormat,
) -> anyhow::Result<()> {
    if output == OutputFormat::Json {
        return render_browse_json(pages, session_id);
    }
    render_browse_table(pages);
    Ok(())
}

fn render_browse_json(pages: &[BrowsePage], session_id: &str) -> anyhow::Result<()> {
    if pages.len() == 1 {
        println!(
            "{}",
            serde_json::to_string_pretty(&json_browse_page(&pages[0]))?
        );
    } else {
        let response = BrowsePagesOutput {
            session_id: session_id.to_owned(),
            pages: pages.iter().map(json_browse_page).collect(),
            complete: pages.last().is_some_and(|page| page.complete),
        };
        println!("{}", serde_json::to_string_pretty(&response)?);
    }
    Ok(())
}

fn render_browse_table(pages: &[BrowsePage]) {
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
        println!("Browse session closed successfully.");
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
        #[allow(
            clippy::expect_used,
            reason = "pages always contains the first page; the loop only runs after that page is pushed"
        )]
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
    println!("Auto-refresh enabled: {}", status.auto_refresh_enabled);
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
        "auto_refresh_enabled": status.auto_refresh_enabled,
        "active_generation": status.active_generation,
        "entry_count": status.entry_count,
        "unique_item_count": status.unique_item_count,
        "started_at": status.started_at,
        "completed_at": status.completed_at,
        "last_error": status.last_error,
        "database_bytes": status.database_bytes,
        "organization": format!("{:?}", status.organization).to_lowercase(),
        "source": format!("{:?}", status.source).to_lowercase(),
        "scheduler": {
            "next_refresh_at": status.scheduler.next_refresh_at,
            "last_attempt_at": status.scheduler.last_attempt_at,
            "last_success_at": status.scheduler.last_success_at,
            "last_success_duration_ms": status.scheduler.last_success_duration_ms,
            "retry_after": status.scheduler.retry_after,
            "consecutive_failures": status.scheduler.consecutive_failures,
            "circuit_open": status.scheduler.circuit_open,
        },
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
mod tests;
