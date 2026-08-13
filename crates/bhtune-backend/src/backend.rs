//! The `Backend` trait: bhtune's single seam for all tag I/O.

use async_trait::async_trait;

use crate::{
    error::BackendResult,
    types::{TagId, TagNode, TagValue, TagWrite, WriteOutcome},
};

/// Abstracts all tag I/O so `bhtune-core`'s tuning engine never knows what it's talking to.
///
/// `OpcDaBackend` (via the `opcda-bridge` crate) is the primary driver for v1.
/// `SimulatorBackend` (an in-process FOPDT process model) and `ReplayBackend` (feeding back a
/// recorded golden-master trace) implement this same trait for CI/demo and validation
/// respectively. `OpcUaBackend`/`ModbusBackend` are roadmap items expected to slot in later
/// without requiring any change to this trait or to `bhtune-core` — that's the entire point
/// of the seam.
///
/// `Send + Sync` so an implementation can be held behind `Arc<dyn Backend>` and shared across
/// async tasks (e.g. a CLI run and a concurrent history-retention sweep in the same process).
/// Connecting/constructing a specific backend (host/port, OPC DA server name, a trace file
/// path, simulator parameters) is deliberately *not* part of this trait — each
/// implementation's own inherent constructor takes whatever it individually needs, since
/// forcing one uniform "connect" signature across such different backends would either be
/// meaningless for some of them or leak implementation-specific parameters into the trait
/// every other implementation would have to ignore.
#[async_trait]
pub trait Backend: Send + Sync {
    /// Reads the current value of every tag in `tags`, in one batched call where the backend
    /// supports it (a single OPC DA `Read` RPC for all of them, rather than one round trip per
    /// tag). Returns exactly one [`TagValue`] per requested tag, in the same order as `tags`.
    ///
    /// A tag with genuinely bad or uncertain quality is still `Ok` — quality is data about
    /// the reading, not a failure of the read itself — reserving `Err` for the read call not
    /// reaching the backend at all, or the backend rejecting the request outright (e.g. an
    /// unrecognized tag name).
    async fn read(&self, tags: &[TagId]) -> BackendResult<Vec<TagValue>>;

    /// Writes `value` to `tag`.
    ///
    /// Returns `Ok(WriteOutcome)` even when the backend rejects the write itself (read-only
    /// tag, out-of-range value, permissions) — that is a normal, expected result of the
    /// write reaching the backend, not an I/O failure. `Err` is reserved for the write call
    /// not reaching the backend at all.
    async fn write(&self, tag: &TagId, value: TagWrite) -> BackendResult<WriteOutcome>;

    /// Lists the tags/branches available directly under `path` (empty string for the top
    /// level) — one level, not a recursive dump of the whole tree.
    ///
    /// Backends with no real browsable tag tree (`SimulatorBackend`, `ReplayBackend`) return
    /// `Err(BackendError::Unsupported { .. })` rather than a misleadingly empty `Ok(vec![])`,
    /// so a caller (e.g. a GUI tag picker) can distinguish "this backend has no tags here"
    /// from "this backend has no concept of browsing at all".
    async fn browse(&self, path: &str) -> BackendResult<Vec<TagNode>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{error::BackendError, types::Quality};
    use chrono::{TimeZone, Utc};
    use std::sync::Mutex;

    /// A minimal in-memory `Backend` used only to prove the trait itself is usable: object-safe
    /// (`Box<dyn Backend>`), async-dispatchable, and that its methods compose the way real
    /// callers (a future `backend-opcda`/`backend-simulator`) will need.
    struct MockBackend {
        values: std::collections::HashMap<TagId, (String, Quality)>,
        writes: Mutex<Vec<(TagId, TagWrite)>>,
        browsable: bool,
    }

    #[async_trait]
    impl Backend for MockBackend {
        async fn read(&self, tags: &[TagId]) -> BackendResult<Vec<TagValue>> {
            let timestamp = Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
            tags.iter()
                .map(|tag| {
                    let (value, quality) = self.values.get(tag).cloned().ok_or_else(|| {
                        BackendError::InvalidTagValue {
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

        async fn write(&self, tag: &TagId, value: TagWrite) -> BackendResult<WriteOutcome> {
            self.writes.lock().unwrap().push((tag.clone(), value));
            Ok(WriteOutcome::success())
        }

        async fn browse(&self, _path: &str) -> BackendResult<Vec<TagNode>> {
            if self.browsable {
                Ok(vec![TagNode {
                    tag: "Area1.LIC101".to_string(),
                    is_branch: true,
                }])
            } else {
                Err(BackendError::Unsupported {
                    operation: "browse",
                })
            }
        }
    }

    fn mock() -> MockBackend {
        let mut values = std::collections::HashMap::new();
        values.insert(
            "Area1.LIC101.PV".to_string(),
            ("42.5".to_string(), Quality::Good),
        );
        values.insert(
            "Area1.LIC101.MODE".to_string(),
            ("MAN".to_string(), Quality::Uncertain),
        );
        MockBackend {
            values,
            writes: Mutex::new(Vec::new()),
            browsable: true,
        }
    }

    #[tokio::test]
    async fn reads_multiple_tags_in_requested_order() {
        let backend = mock();
        let tags = vec![
            "Area1.LIC101.MODE".to_string(),
            "Area1.LIC101.PV".to_string(),
        ];
        let values = backend.read(&tags).await.unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].tag, "Area1.LIC101.MODE");
        assert_eq!(values[0].value, "MAN");
        assert_eq!(values[0].quality, Quality::Uncertain);
        assert_eq!(values[1].tag, "Area1.LIC101.PV");
        assert_eq!(values[1].value, "42.5");
        assert_eq!(values[1].quality, Quality::Good);
    }

    #[tokio::test]
    async fn reading_an_unknown_tag_is_invalid_tag_value_not_a_panic() {
        let backend = mock();
        let err = backend
            .read(&["Nonexistent.Tag".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, BackendError::InvalidTagValue { .. }));
    }

    #[tokio::test]
    async fn write_records_the_call_and_reports_success() {
        let backend = mock();
        let outcome = backend
            .write(&"Area1.LIC101.MV".to_string(), TagWrite::Float(55.0))
            .await
            .unwrap();
        assert!(outcome.success);
        assert_eq!(
            backend.writes.lock().unwrap().as_slice(),
            &[("Area1.LIC101.MV".to_string(), TagWrite::Float(55.0))]
        );
    }

    #[tokio::test]
    async fn write_supports_raw_mode_values_for_mode_revert() {
        let backend = mock();
        backend
            .write(
                &"Area1.LIC101.MODE".to_string(),
                TagWrite::Raw("AUT".into()),
            )
            .await
            .unwrap();
        assert_eq!(
            backend.writes.lock().unwrap().as_slice(),
            &[("Area1.LIC101.MODE".to_string(), TagWrite::Raw("AUT".into()))]
        );
    }

    #[tokio::test]
    async fn browse_returns_unsupported_when_backend_has_no_tag_tree() {
        let mut backend = mock();
        backend.browsable = false;
        let err = backend.browse("").await.unwrap_err();
        assert!(matches!(
            err,
            BackendError::Unsupported {
                operation: "browse"
            }
        ));
    }

    #[tokio::test]
    async fn trait_is_object_safe_and_usable_through_a_trait_object() {
        // The real point of this test: it must compile. `Box<dyn Backend>` is exactly the
        // shape a future config-driven "pick a backend at runtime" call site needs.
        let backend: Box<dyn Backend> = Box::new(mock());
        let values = backend
            .read(&["Area1.LIC101.PV".to_string()])
            .await
            .unwrap();
        assert_eq!(values[0].value, "42.5");
    }
}
