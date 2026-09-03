//! [`AppState`]: the shared state every route handler receives via `axum::extract::State`.

use bhtune_db::SqlitePool;
use chrono::{DateTime, Duration, Utc};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};

use crate::active_run::ActiveRun;
use bhtune_cli::config::{DemoPolicy, ServerMode};
use tokio::sync::Mutex;
use tokio::sync::{OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DemoQuotaExceeded {
    pub retry_after_secs: u64,
}

#[derive(Debug, Clone)]
struct AcceptedStart {
    id: u64,
    accepted_at: DateTime<Utc>,
}

#[derive(Default)]
struct AcceptedStartWindows {
    next_id: u64,
    by_token: HashMap<String, VecDeque<AcceptedStart>>,
    by_ip: HashMap<String, VecDeque<AcceptedStart>>,
}

#[derive(Debug)]
pub struct DemoAcceptedStart {
    id: u64,
    token_hash: String,
    client_ip: String,
}

#[derive(Clone)]
pub struct DemoRuntime {
    run_permits: Arc<Semaphore>,
    sse_permits: Arc<Semaphore>,
    ordinary_request_permits: Arc<Semaphore>,
    start_admission: Arc<Mutex<()>>,
    accepted_starts: Arc<Mutex<AcceptedStartWindows>>,
    visitor_run_permits: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    visitor_sse_permits: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
}

impl DemoRuntime {
    fn new(policy: DemoPolicy) -> Self {
        Self {
            run_permits: Arc::new(Semaphore::new(policy.max_active_runs_global as usize)),
            sse_permits: Arc::new(Semaphore::new(policy.max_sse_global as usize)),
            ordinary_request_permits: Arc::new(Semaphore::new(
                policy.ordinary_request_concurrency as usize,
            )),
            start_admission: Arc::new(Mutex::new(())),
            accepted_starts: Arc::new(Mutex::new(AcceptedStartWindows::default())),
            visitor_run_permits: Arc::new(Mutex::new(HashMap::new())),
            visitor_sse_permits: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn try_acquire_global_run(
        &self,
    ) -> Result<OwnedSemaphorePermit, tokio::sync::TryAcquireError> {
        self.run_permits.clone().try_acquire_owned()
    }

    pub fn try_acquire_global_sse(
        &self,
    ) -> Result<OwnedSemaphorePermit, tokio::sync::TryAcquireError> {
        self.sse_permits.clone().try_acquire_owned()
    }

    pub fn try_acquire_ordinary_request(
        &self,
    ) -> Result<OwnedSemaphorePermit, tokio::sync::TryAcquireError> {
        self.ordinary_request_permits.clone().try_acquire_owned()
    }

    /// Serializes the short admission phase that checks global row capacity and persists a
    /// prepared run. Without this guard, several simultaneous starts could all observe one
    /// remaining row slot before any of them inserted, exceeding the fixed global Demo cap.
    pub async fn lock_start_admission(&self) -> OwnedMutexGuard<()> {
        self.start_admission.clone().lock_owned().await
    }

    pub async fn try_acquire_visitor_run(
        &self,
        token_hash: &str,
        limit: u32,
    ) -> Result<OwnedSemaphorePermit, tokio::sync::TryAcquireError> {
        visitor_semaphore(&self.visitor_run_permits, token_hash, limit)
            .await
            .try_acquire_owned()
    }

    pub async fn try_acquire_visitor_sse(
        &self,
        token_hash: &str,
        limit: u32,
    ) -> Result<OwnedSemaphorePermit, tokio::sync::TryAcquireError> {
        visitor_semaphore(&self.visitor_sse_permits, token_hash, limit)
            .await
            .try_acquire_owned()
    }

    /// Atomically reserves one accepted-start slot in both the visitor-token and client-IP
    /// windows. A caller that fails before scheduling the run must release the returned handle;
    /// a successfully scheduled run leaves it recorded until the fixed window expires.
    pub async fn reserve_accepted_start(
        &self,
        token_hash: &str,
        client_ip: &str,
        now: DateTime<Utc>,
        policy: DemoPolicy,
    ) -> Result<DemoAcceptedStart, DemoQuotaExceeded> {
        let mut windows = self.accepted_starts.lock().await;
        prune_start_windows(&mut windows, now, policy.accepted_start_window_secs);
        if let Some(retry_after_secs) =
            quota_retry_after(&windows, token_hash, client_ip, now, policy)
        {
            return Err(DemoQuotaExceeded { retry_after_secs });
        }

        windows.next_id = windows.next_id.wrapping_add(1);
        let accepted = AcceptedStart {
            id: windows.next_id,
            accepted_at: now,
        };
        windows
            .by_token
            .entry(token_hash.to_owned())
            .or_default()
            .push_back(accepted.clone());
        windows
            .by_ip
            .entry(client_ip.to_owned())
            .or_default()
            .push_back(accepted.clone());
        Ok(DemoAcceptedStart {
            id: accepted.id,
            token_hash: token_hash.to_owned(),
            client_ip: client_ip.to_owned(),
        })
    }

    pub async fn release_accepted_start(&self, accepted: DemoAcceptedStart) {
        let mut windows = self.accepted_starts.lock().await;
        remove_accepted_start(&mut windows.by_token, &accepted.token_hash, accepted.id);
        remove_accepted_start(&mut windows.by_ip, &accepted.client_ip, accepted.id);
    }

    /// Reclaims expired quota entries and inactive per-visitor semaphores during the periodic
    /// Demo cleanup pass, including tokens that never make another request.
    pub async fn cleanup(&self, now: DateTime<Utc>, policy: DemoPolicy) {
        {
            let mut accepted_starts = self.accepted_starts.lock().await;
            prune_start_windows(&mut accepted_starts, now, policy.accepted_start_window_secs);
        }
        {
            let mut visitor_runs = self.visitor_run_permits.lock().await;
            retain_active_visitor_semaphores(&mut visitor_runs, policy.max_active_runs_per_visitor);
        }
        {
            let mut visitor_streams = self.visitor_sse_permits.lock().await;
            retain_active_visitor_semaphores(&mut visitor_streams, policy.max_sse_per_visitor);
        }
    }
}

async fn visitor_semaphore(
    permits: &Mutex<HashMap<String, Arc<Semaphore>>>,
    token_hash: &str,
    limit: u32,
) -> Arc<Semaphore> {
    let mut permits = permits.lock().await;
    permits.retain(|_, permit| {
        Arc::strong_count(permit) > 1 || permit.available_permits() < limit as usize
    });
    permits
        .entry(token_hash.to_owned())
        .or_insert_with(|| Arc::new(Semaphore::new(limit as usize)))
        .clone()
}

fn retain_active_visitor_semaphores(permits: &mut HashMap<String, Arc<Semaphore>>, limit: u32) {
    permits.retain(|_, permit| {
        Arc::strong_count(permit) > 1 || permit.available_permits() < limit as usize
    });
}

fn prune_start_windows(windows: &mut AcceptedStartWindows, now: DateTime<Utc>, window_secs: u64) {
    let cutoff = now - Duration::seconds(window_secs as i64);
    windows.by_token.retain(|_, starts| {
        while starts
            .front()
            .is_some_and(|start| start.accepted_at <= cutoff)
        {
            starts.pop_front();
        }
        !starts.is_empty()
    });
    windows.by_ip.retain(|_, starts| {
        while starts
            .front()
            .is_some_and(|start| start.accepted_at <= cutoff)
        {
            starts.pop_front();
        }
        !starts.is_empty()
    });
}

fn quota_retry_after(
    windows: &AcceptedStartWindows,
    token_hash: &str,
    client_ip: &str,
    now: DateTime<Utc>,
    policy: DemoPolicy,
) -> Option<u64> {
    let token_retry = retry_after_for(
        windows.by_token.get(token_hash),
        policy.accepted_starts_per_token,
        now,
        policy.accepted_start_window_secs,
    );
    let ip_retry = retry_after_for(
        windows.by_ip.get(client_ip),
        policy.accepted_starts_per_client_ip,
        now,
        policy.accepted_start_window_secs,
    );
    match (token_retry, ip_retry) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(retry), None) | (None, Some(retry)) => Some(retry),
        (None, None) => None,
    }
}

fn retry_after_for(
    starts: Option<&VecDeque<AcceptedStart>>,
    limit: u32,
    now: DateTime<Utc>,
    window_secs: u64,
) -> Option<u64> {
    let starts = starts?;
    if starts.len() < limit as usize {
        return None;
    }
    let remaining_ms = (starts.front()?.accepted_at + Duration::seconds(window_secs as i64) - now)
        .num_milliseconds()
        .max(1);
    Some((remaining_ms as u64).div_ceil(1_000))
}

fn remove_accepted_start(
    windows: &mut HashMap<String, VecDeque<AcceptedStart>>,
    key: &str,
    id: u64,
) {
    let Some(starts) = windows.get_mut(key) else {
        return;
    };
    starts.retain(|start| start.id != id);
    if starts.is_empty() {
        windows.remove(key);
    }
}

/// Cheap to clone (an `Arc`-backed connection pool under the hood, per `sqlx`, and
/// [`ActiveRun`] is itself `Arc<Mutex<..>>`-backed) -- axum's `State` extractor just requires
/// `Clone` to hand a copy to each request.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    /// The in-flight tune registry and exclusive post-hoc write/revert reservation -- see
    /// [`ActiveRun`]'s own doc comment for the concurrency and shutdown behavior.
    pub active_run: ActiveRun,
    /// The live, revisioned TOML configuration. Route handlers take a fresh snapshot for
    /// every operation so a configuration-page save is visible without restarting the server.
    pub config_store: Arc<RwLock<bhtune_cli::config::LoadedConfigStore>>,
    pub allowed_origin: Option<String>,
    pub trusted_proxy: Option<String>,
    pub mode: ServerMode,
    pub demo_policy: DemoPolicy,
    pub demo_runtime: DemoRuntime,
}

impl AppState {
    pub fn config_snapshot(&self) -> anyhow::Result<bhtune_cli::config::BhtuneConfig> {
        self.config_store
            .read()
            .map(|store| store.config.clone())
            .map_err(|_| anyhow::anyhow!("configuration store lock is poisoned"))
    }

    pub fn for_mode(
        pool: SqlitePool,
        config_store: Arc<RwLock<bhtune_cli::config::LoadedConfigStore>>,
        mode: ServerMode,
        demo_policy: DemoPolicy,
    ) -> Self {
        Self::for_mode_with_network_config(pool, config_store, mode, demo_policy, None, None)
    }

    pub fn for_mode_with_network_config(
        pool: SqlitePool,
        config_store: Arc<RwLock<bhtune_cli::config::LoadedConfigStore>>,
        mode: ServerMode,
        demo_policy: DemoPolicy,
        allowed_origin: Option<String>,
        trusted_proxy: Option<String>,
    ) -> Self {
        Self {
            pool,
            active_run: ActiveRun::default(),
            config_store,
            allowed_origin,
            trusted_proxy,
            demo_runtime: DemoRuntime::new(demo_policy),
            mode,
            demo_policy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn config_snapshot_returns_an_independent_config_copy() {
        let state = crate::test_support::in_memory_state().await;
        let mut snapshot = state.config_snapshot().unwrap();
        snapshot.allow_uncertain_quality = false;

        assert!(state.config_snapshot().unwrap().allow_uncertain_quality);
    }

    #[tokio::test]
    async fn config_snapshot_reports_a_poisoned_store_lock() {
        let state = crate::test_support::in_memory_state().await;
        let store = state.config_store.clone();
        let _ = std::thread::spawn(move || {
            let _guard = store.write().unwrap();
            panic!("deliberately poison the test lock");
        })
        .join();

        assert!(
            state
                .config_snapshot()
                .unwrap_err()
                .to_string()
                .contains("poisoned")
        );
    }

    #[tokio::test]
    async fn demo_runtime_uses_the_global_and_ordinary_policy_limits() {
        let policy = DemoPolicy {
            max_active_runs_global: 1,
            max_sse_global: 1,
            ordinary_request_concurrency: 1,
            ..DemoPolicy::default()
        };
        let runtime = DemoRuntime::new(policy);

        let _run = runtime.try_acquire_global_run().unwrap();
        assert!(runtime.try_acquire_global_run().is_err());
        let _sse = runtime.try_acquire_global_sse().unwrap();
        assert!(runtime.try_acquire_global_sse().is_err());
        let _request = runtime.try_acquire_ordinary_request().unwrap();
        assert!(runtime.try_acquire_ordinary_request().is_err());
    }

    #[tokio::test]
    async fn demo_start_admission_is_serialized() {
        let runtime = DemoRuntime::new(DemoPolicy::default());
        let admission = runtime.lock_start_admission().await;
        let waiting_runtime = runtime.clone();
        let waiter = tokio::spawn(async move {
            let _admission = waiting_runtime.lock_start_admission().await;
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        drop(admission);

        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn accepted_start_quotas_are_independent_for_token_and_client_ip() {
        let policy = DemoPolicy::default();
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let token_runtime = DemoRuntime::new(policy);
        for index in 0..policy.accepted_starts_per_token {
            token_runtime
                .reserve_accepted_start("token", &format!("ip-{index}"), now, policy)
                .await
                .unwrap();
        }
        let token_error = token_runtime
            .reserve_accepted_start("token", "unused-ip", now, policy)
            .await
            .unwrap_err();
        assert_eq!(
            token_error.retry_after_secs,
            policy.accepted_start_window_secs
        );
        assert!(
            token_runtime
                .reserve_accepted_start(
                    "token",
                    "unused-ip",
                    now + Duration::seconds(policy.accepted_start_window_secs as i64),
                    policy,
                )
                .await
                .is_ok()
        );

        let ip_runtime = DemoRuntime::new(policy);
        for index in 0..policy.accepted_starts_per_client_ip {
            ip_runtime
                .reserve_accepted_start(&format!("token-{index}"), "shared-ip", now, policy)
                .await
                .unwrap();
        }
        assert!(
            ip_runtime
                .reserve_accepted_start("unused-token", "shared-ip", now, policy)
                .await
                .is_err()
        );
        assert!(
            ip_runtime
                .reserve_accepted_start("unused-token", "unused-ip", now, policy)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn accepted_start_quota_uses_the_larger_token_or_ip_retry_after() {
        let policy = DemoPolicy {
            accepted_starts_per_token: 1,
            accepted_starts_per_client_ip: 1,
            accepted_start_window_secs: 20,
            ..DemoPolicy::default()
        };
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let runtime = DemoRuntime::new(policy);

        runtime
            .reserve_accepted_start("token", "other-ip", now - Duration::seconds(5), policy)
            .await
            .unwrap();
        runtime
            .reserve_accepted_start(
                "other-token",
                "shared-ip",
                now - Duration::seconds(10),
                policy,
            )
            .await
            .unwrap();

        let error = runtime
            .reserve_accepted_start("token", "shared-ip", now, policy)
            .await
            .unwrap_err();

        assert_eq!(error.retry_after_secs, 15);
    }

    #[tokio::test]
    async fn rejected_start_reservations_can_be_released_without_consuming_quota() {
        let policy = DemoPolicy::default();
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let runtime = DemoRuntime::new(policy);
        let mut starts = Vec::new();
        for index in 0..policy.accepted_starts_per_token {
            starts.push(
                runtime
                    .reserve_accepted_start("token", &format!("ip-{index}"), now, policy)
                    .await
                    .unwrap(),
            );
        }
        assert!(
            runtime
                .reserve_accepted_start("token", "another-ip", now, policy)
                .await
                .is_err()
        );

        runtime.release_accepted_start(starts.remove(0)).await;

        assert!(
            runtime
                .reserve_accepted_start("token", "another-ip", now, policy)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn releasing_an_accepted_start_removes_empty_token_and_ip_windows() {
        let policy = DemoPolicy::default();
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let runtime = DemoRuntime::new(policy);
        let accepted = runtime
            .reserve_accepted_start("token", "ip", now, policy)
            .await
            .unwrap();

        runtime.release_accepted_start(accepted).await;

        let windows = runtime.accepted_starts.lock().await;
        assert!(windows.by_token.is_empty());
        assert!(windows.by_ip.is_empty());
    }

    #[test]
    fn removing_an_unknown_accepted_start_is_a_no_op() {
        let mut windows = HashMap::new();

        remove_accepted_start(&mut windows, "missing", 1);

        assert!(windows.is_empty());
    }

    #[tokio::test]
    async fn cleanup_reclaims_expired_windows_and_idle_visitor_semaphores() {
        let policy = DemoPolicy {
            accepted_starts_per_token: 1,
            accepted_starts_per_client_ip: 1,
            accepted_start_window_secs: 10,
            max_active_runs_per_visitor: 1,
            max_sse_per_visitor: 1,
            ..DemoPolicy::default()
        };
        let runtime = DemoRuntime::new(policy);
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        runtime
            .reserve_accepted_start("token", "ip", now, policy)
            .await
            .unwrap();
        drop(runtime.try_acquire_visitor_run("token", 1).await.unwrap());
        drop(runtime.try_acquire_visitor_sse("token", 1).await.unwrap());

        runtime.cleanup(now + Duration::seconds(10), policy).await;

        assert!(
            runtime
                .reserve_accepted_start("token", "ip", now + Duration::seconds(10), policy)
                .await
                .is_ok()
        );
        assert!(runtime.visitor_run_permits.lock().await.is_empty());
        assert!(runtime.visitor_sse_permits.lock().await.is_empty());
    }
}
