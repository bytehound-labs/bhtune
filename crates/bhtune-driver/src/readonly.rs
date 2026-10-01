//! A driver adapter that forwards reads but never forwards writes.

use async_trait::async_trait;

use crate::{
    Driver, DriverCapabilities, DriverError, DriverResult, TagId, TagValue, TagWrite, WriteOutcome,
};

/// Restricts a driver to read-only use.
///
/// Reads and capability discovery are forwarded. Writes and every other operation retain the
/// trait's unsupported behavior, so a caller cannot accidentally use this wrapper to mutate a
/// controller or a gateway-owned namespace index.
pub struct ReadOnlyDriver {
    inner: Box<dyn Driver>,
}

impl ReadOnlyDriver {
    /// Wraps a driver for a read-only operation.
    pub fn new(inner: Box<dyn Driver>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl Driver for ReadOnlyDriver {
    async fn read(&self, tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
        self.inner.read(tags).await
    }

    async fn write(&self, _tag: &TagId, _value: TagWrite) -> DriverResult<WriteOutcome> {
        Err(DriverError::Unsupported { operation: "write" })
    }

    async fn capabilities(&self) -> DriverResult<DriverCapabilities> {
        self.inner.capabilities().await
    }

    async fn browse(&self, _request: crate::BrowsePageRequest) -> DriverResult<crate::BrowsePage> {
        Err(DriverError::Unsupported {
            operation: "browse",
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::Utc;

    use super::*;
    use crate::{
        BrowseNodeKind, BrowsePage, BrowsePageRequest, BrowseSource, DriverCapabilities,
        NamespaceOrganization, Quality,
    };

    struct RecordingDriver {
        writes: Arc<Mutex<Vec<(TagId, TagWrite)>>>,
    }

    #[async_trait]
    impl Driver for RecordingDriver {
        async fn read(&self, tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
            Ok(tags
                .iter()
                .map(|tag| TagValue {
                    tag: tag.clone(),
                    value: "1".to_string(),
                    quality: Quality::Good,
                    timestamp: Some(Utc::now()),
                })
                .collect())
        }

        async fn write(&self, tag: &TagId, value: TagWrite) -> DriverResult<WriteOutcome> {
            self.writes.lock().unwrap().push((tag.clone(), value));
            Ok(WriteOutcome::success())
        }

        async fn capabilities(&self) -> DriverResult<DriverCapabilities> {
            Ok(DriverCapabilities {
                application_version: "test".to_string(),
                protocol_version: "1".to_string(),
                max_page_size: 1,
                supports_browse_sessions: false,
                supports_search: false,
                organization: NamespaceOrganization::Unspecified,
                source: BrowseSource::Unspecified,
                supports_indexed_search: false,
                indexed_search_protocol_version: String::new(),
                max_indexed_search_results: 0,
                search_index_state: crate::SearchIndexState::Unspecified,
            })
        }

        async fn browse(&self, _request: BrowsePageRequest) -> DriverResult<BrowsePage> {
            Ok(BrowsePage {
                session_id: "test".to_string(),
                nodes: vec![crate::BrowseNode {
                    node_key: "node".to_string(),
                    display_name: "Node".to_string(),
                    kind: BrowseNodeKind::Item,
                    item_id: Some("tag".to_string()),
                }],
                next_page_token: None,
                complete: true,
                organization: NamespaceOrganization::Unspecified,
                source: BrowseSource::Unspecified,
                warning: None,
            })
        }
    }

    #[tokio::test]
    async fn attempted_write_does_not_reach_the_underlying_driver() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let driver = ReadOnlyDriver::new(Box::new(RecordingDriver {
            writes: Arc::clone(&writes),
        }));

        assert!(matches!(
            driver
                .write(&"Unit1.LIC101.MV".to_string(), TagWrite::Float(50.0))
                .await,
            Err(DriverError::Unsupported { operation: "write" })
        ));
        assert!(writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn recording_driver_write_records_an_underlying_write() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let driver = RecordingDriver {
            writes: Arc::clone(&writes),
        };

        driver
            .write(&"Unit1.LIC101.MV".to_string(), TagWrite::Float(50.0))
            .await
            .unwrap();
        assert_eq!(writes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn recording_driver_browse_returns_its_fixture_page() {
        let driver = RecordingDriver {
            writes: Arc::new(Mutex::new(Vec::new())),
        };

        let page = driver.browse(BrowsePageRequest::root(1)).await.unwrap();
        assert_eq!(page.session_id, "test");
        assert_eq!(page.nodes[0].item_id.as_deref(), Some("tag"));
    }

    #[tokio::test]
    async fn read_only_driver_forwards_reads_and_capabilities_but_not_browse() {
        let driver = ReadOnlyDriver::new(Box::new(RecordingDriver {
            writes: Arc::new(Mutex::new(Vec::new())),
        }));

        let values = driver.read(&["Unit1.LIC101.PV".to_string()]).await.unwrap();
        assert_eq!(values[0].value, "1");
        assert_eq!(
            driver.capabilities().await.unwrap().application_version,
            "test"
        );
        assert!(matches!(
            driver.browse(BrowsePageRequest::root(1)).await,
            Err(DriverError::Unsupported {
                operation: "browse"
            })
        ));
    }
}
