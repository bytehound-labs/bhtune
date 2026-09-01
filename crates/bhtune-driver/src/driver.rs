//! The `Driver` trait: bhtune's single seam for all tag I/O.

use async_trait::async_trait;

use crate::{
    error::DriverResult,
    types::{
        BrowsePage, BrowsePageRequest, DriverCapabilities, SearchEvent, SearchIndexControlAction,
        SearchIndexRequest, SearchIndexResponse, SearchIndexStatus, SearchRequest, TagId, TagValue,
        TagWrite, WriteOutcome,
    },
};

/// Abstracts all tag I/O so `bhtune-core`'s tuning engine never knows what it's talking to.
///
/// `OpcDaDriver` (via the `opcda-bridge` crate) is the primary driver for v1.
/// `SimulatorDriver` (an in-process FOPDT process model) and `ReplayDriver` (feeding back a
/// recorded golden-master trace) implement this same trait for CI/demo and validation
/// respectively. `OpcUaDriver`/`ModbusDriver` are roadmap items expected to slot in later
/// without requiring any change to this trait or to `bhtune-core` — that's the entire point
/// of the seam.
///
/// `Send + Sync` so an implementation can be held behind `Arc<dyn Driver>` and shared across
/// async tasks (e.g. a CLI run and a concurrent history-retention sweep in the same process).
/// Connecting/constructing a specific driver (host/port, OPC DA server name, a trace file
/// path, simulator parameters) is deliberately *not* part of this trait — each
/// implementation's own inherent constructor takes whatever it individually needs, since
/// forcing one uniform "connect" signature across such different drivers would either be
/// meaningless for some of them or leak implementation-specific parameters into the trait
/// every other implementation would have to ignore.
#[async_trait]
pub trait Driver: Send + Sync {
    /// Reads the current value of every tag in `tags`, in one batched call where the driver
    /// supports it (a single OPC DA `Read` RPC for all of them, rather than one round trip per
    /// tag). Returns exactly one [`TagValue`] per requested tag, in the same order as `tags`.
    ///
    /// A tag with genuinely bad or uncertain quality is still `Ok` — quality is data about
    /// the reading, not a failure of the read itself — reserving `Err` for the read call not
    /// reaching the driver at all, or the driver rejecting the request outright (e.g. an
    /// unrecognized tag name).
    async fn read(&self, tags: &[TagId]) -> DriverResult<Vec<TagValue>>;

    /// Writes `value` to `tag`.
    ///
    /// Returns `Ok(WriteOutcome)` even when the driver rejects the write itself (read-only
    /// tag, out-of-range value, permissions) — that is a normal, expected result of the
    /// write reaching the driver, not an I/O failure. `Err` is reserved for the write call
    /// not reaching the driver at all.
    async fn write(&self, tag: &TagId, value: TagWrite) -> DriverResult<WriteOutcome>;

    /// Reports the namespace capabilities of this driver/server pair.
    async fn capabilities(&self) -> DriverResult<DriverCapabilities> {
        Err(crate::error::DriverError::Unsupported {
            operation: "capabilities",
        })
    }

    /// Lists one bounded page of immediate children. Navigation uses opaque session, node, and
    /// continuation values returned by the driver; callers must not infer hierarchy by parsing
    /// punctuation in an ItemID.
    async fn browse(&self, request: BrowsePageRequest) -> DriverResult<BrowsePage>;

    /// Releases a server-side browse session.
    async fn close_browse_session(&self, _session_id: &str) -> DriverResult<()> {
        Err(crate::error::DriverError::Unsupported {
            operation: "browse-session close",
        })
    }

    /// Collects a bounded namespace search. Drivers that support progressive search may expose
    /// a richer stream through their concrete type; this method is the portable trait surface.
    async fn search(&self, _request: SearchRequest) -> DriverResult<Vec<SearchEvent>> {
        Err(crate::error::DriverError::Unsupported {
            operation: "search",
        })
    }

    /// Reports the gateway-owned persistent namespace-index status.
    async fn search_index_status(&self) -> DriverResult<SearchIndexStatus> {
        Err(crate::error::DriverError::Unsupported {
            operation: "indexed-search status",
        })
    }

    /// Starts or coalesces a persistent namespace-index refresh.
    async fn refresh_search_index(&self, _force: bool) -> DriverResult<SearchIndexStatus> {
        Err(crate::error::DriverError::Unsupported {
            operation: "indexed-search refresh",
        })
    }

    /// Pauses, resumes, or cancels a persistent namespace-index build.
    async fn control_search_index(
        &self,
        _action: SearchIndexControlAction,
    ) -> DriverResult<SearchIndexStatus> {
        Err(crate::error::DriverError::Unsupported {
            operation: "indexed-search control",
        })
    }

    /// Queries the gateway-owned persistent namespace index.
    async fn search_index(
        &self,
        _request: SearchIndexRequest,
    ) -> DriverResult<SearchIndexResponse> {
        Err(crate::error::DriverError::Unsupported {
            operation: "indexed search",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{error::DriverError, types::Quality};
    use chrono::{TimeZone, Utc};
    use std::sync::Mutex;

    struct BareDriver;

    #[async_trait]
    impl Driver for BareDriver {
        async fn read(&self, _tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
            Ok(Vec::new())
        }

        async fn write(&self, _tag: &TagId, _value: TagWrite) -> DriverResult<WriteOutcome> {
            Ok(WriteOutcome::success())
        }

        async fn browse(&self, _request: BrowsePageRequest) -> DriverResult<BrowsePage> {
            Ok(BrowsePage {
                session_id: "s".into(),
                nodes: Vec::new(),
                next_page_token: None,
                complete: true,
                organization: crate::types::NamespaceOrganization::Unspecified,
                source: crate::types::BrowseSource::Unspecified,
                warning: None,
            })
        }
    }

    /// A minimal in-memory `Driver` used only to prove the trait itself is usable: object-safe
    /// (`Box<dyn Driver>`), async-dispatchable, and that its methods compose the way real
    /// callers (a future `driver-opcda`/`driver-simulator`) will need.
    struct MockDriver {
        values: std::collections::HashMap<TagId, (String, Quality)>,
        writes: Mutex<Vec<(TagId, TagWrite)>>,
    }

    #[async_trait]
    impl Driver for MockDriver {
        async fn read(&self, tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
            let timestamp = Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
            tags.iter()
                .map(|tag| {
                    let (value, quality) = self.values.get(tag).cloned().ok_or_else(|| {
                        DriverError::InvalidTagValue {
                            tag: tag.clone(),
                            message: "unknown tag".to_string(),
                        }
                    })?;
                    Ok(TagValue {
                        tag: tag.clone(),
                        value,
                        quality,
                        timestamp,
                    })
                })
                .collect()
        }

        async fn write(&self, tag: &TagId, value: TagWrite) -> DriverResult<WriteOutcome> {
            self.writes.lock().unwrap().push((tag.clone(), value));
            Ok(WriteOutcome::success())
        }

        async fn capabilities(&self) -> DriverResult<DriverCapabilities> {
            Ok(DriverCapabilities {
                application_version: "test".into(),
                protocol_version: "test".into(),
                max_page_size: 200,
                supports_browse_sessions: true,
                supports_search: true,
                organization: crate::types::NamespaceOrganization::Hierarchical,
                source: crate::types::BrowseSource::Derived,
                supports_indexed_search: false,
                indexed_search_protocol_version: String::new(),
                max_indexed_search_results: 0,
                search_index_state: crate::types::SearchIndexState::Unspecified,
            })
        }

        async fn browse(&self, _request: BrowsePageRequest) -> DriverResult<BrowsePage> {
            Err(DriverError::Unsupported {
                operation: "browse",
            })
        }

        async fn close_browse_session(&self, _session_id: &str) -> DriverResult<()> {
            Err(DriverError::Unsupported {
                operation: "browse-session close",
            })
        }

        async fn search(&self, _request: SearchRequest) -> DriverResult<Vec<SearchEvent>> {
            Err(DriverError::Unsupported {
                operation: "search",
            })
        }
    }

    fn mock() -> MockDriver {
        let mut values = std::collections::HashMap::new();
        values.insert(
            "Area1.LIC101.PV".to_string(),
            ("42.5".to_string(), Quality::Good),
        );
        values.insert(
            "Area1.LIC101.MODE".to_string(),
            ("MAN".to_string(), Quality::Uncertain),
        );
        MockDriver {
            values,
            writes: Mutex::new(Vec::new()),
        }
    }

    #[tokio::test]
    async fn reads_multiple_tags_in_requested_order() {
        let driver = mock();
        let tags = vec![
            "Area1.LIC101.MODE".to_string(),
            "Area1.LIC101.PV".to_string(),
        ];
        let values = driver.read(&tags).await.unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].tag, "Area1.LIC101.MODE");
        assert_eq!(values[0].value, "MAN");
        assert_eq!(values[0].quality, Quality::Uncertain);
        assert_eq!(values[1].tag, "Area1.LIC101.PV");
        assert_eq!(values[1].value, "42.5");
        assert_eq!(values[1].quality, Quality::Good);
    }

    #[tokio::test]
    async fn default_trait_operations_report_unsupported() {
        let driver = BareDriver;
        assert!(driver.read(&[]).await.unwrap().is_empty());
        assert!(
            driver
                .write(&"MV".to_string(), TagWrite::Float(1.0))
                .await
                .unwrap()
                .success
        );
        assert_eq!(
            driver
                .browse(BrowsePageRequest::root(1))
                .await
                .unwrap()
                .session_id,
            "s"
        );
        assert!(matches!(
            driver.capabilities().await,
            Err(DriverError::Unsupported {
                operation: "capabilities"
            })
        ));
        assert!(matches!(
            driver.close_browse_session("session").await,
            Err(DriverError::Unsupported {
                operation: "browse-session close"
            })
        ));
        assert!(matches!(
            driver
                .search(SearchRequest {
                    query: "PV".into(),
                    match_mode: crate::types::SearchMatchMode::Contains,
                    session_id: None,
                    scope_node_key: None,
                    max_results: 10,
                    include_branches: false,
                    refresh: false,
                })
                .await,
            Err(DriverError::Unsupported {
                operation: "search"
            })
        ));
        assert!(matches!(
            driver.search_index_status().await,
            Err(DriverError::Unsupported {
                operation: "indexed-search status"
            })
        ));
        assert!(matches!(
            driver.refresh_search_index(false).await,
            Err(DriverError::Unsupported {
                operation: "indexed-search refresh"
            })
        ));
        assert!(matches!(
            driver
                .control_search_index(crate::types::SearchIndexControlAction::Pause)
                .await,
            Err(DriverError::Unsupported {
                operation: "indexed-search control"
            })
        ));
        assert!(matches!(
            driver
                .search_index(crate::types::SearchIndexRequest::new(
                    "PV",
                    crate::types::SearchMatchMode::Exact,
                    1,
                ))
                .await,
            Err(DriverError::Unsupported {
                operation: "indexed search"
            })
        ));
    }

    #[tokio::test]
    async fn mock_driver_capabilities_and_explicit_unsupported_operations_are_callable() {
        let driver = mock();
        let capabilities = <MockDriver as Driver>::capabilities(&driver).await.unwrap();
        assert_eq!(capabilities.application_version, "test");
        assert!(matches!(
            <MockDriver as Driver>::browse(&driver, BrowsePageRequest::root(1)).await,
            Err(DriverError::Unsupported {
                operation: "browse"
            })
        ));
        assert!(matches!(
            <MockDriver as Driver>::close_browse_session(&driver, "session").await,
            Err(DriverError::Unsupported {
                operation: "browse-session close"
            })
        ));
        assert!(matches!(
            <MockDriver as Driver>::search(
                &driver,
                SearchRequest {
                    query: "PV".into(),
                    match_mode: crate::types::SearchMatchMode::Exact,
                    session_id: None,
                    scope_node_key: None,
                    max_results: 1,
                    include_branches: false,
                    refresh: false,
                },
            )
            .await,
            Err(DriverError::Unsupported {
                operation: "search"
            })
        ));
    }

    #[tokio::test]
    async fn reading_an_unknown_tag_is_invalid_tag_value_not_a_panic() {
        let driver = mock();
        let err = driver
            .read(&["Nonexistent.Tag".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, DriverError::InvalidTagValue { .. }));
    }

    #[tokio::test]
    async fn write_records_the_call_and_reports_success() {
        let driver = mock();
        let outcome = driver
            .write(&"Area1.LIC101.MV".to_string(), TagWrite::Float(55.0))
            .await
            .unwrap();
        assert!(outcome.success);
        assert_eq!(
            driver.writes.lock().unwrap().as_slice(),
            &[("Area1.LIC101.MV".to_string(), TagWrite::Float(55.0))]
        );
    }

    #[tokio::test]
    async fn write_supports_raw_mode_values_for_mode_revert() {
        let driver = mock();
        driver
            .write(
                &"Area1.LIC101.MODE".to_string(),
                TagWrite::Raw("AUT".into()),
            )
            .await
            .unwrap();
        assert_eq!(
            driver.writes.lock().unwrap().as_slice(),
            &[("Area1.LIC101.MODE".to_string(), TagWrite::Raw("AUT".into()))]
        );
    }

    #[tokio::test]
    async fn browse_returns_unsupported_when_driver_has_no_tag_tree() {
        let err = mock()
            .browse(BrowsePageRequest::root(20))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            DriverError::Unsupported {
                operation: "browse"
            }
        ));
    }

    #[tokio::test]
    async fn trait_is_object_safe_and_usable_through_a_trait_object() {
        // The real point of this test: it must compile. `Box<dyn Driver>` is exactly the
        // shape a future config-driven "pick a driver at runtime" call site needs.
        let driver: Box<dyn Driver> = Box::new(mock());
        let values = driver.read(&["Area1.LIC101.PV".to_string()]).await.unwrap();
        assert_eq!(values[0].value, "42.5");
    }
}
