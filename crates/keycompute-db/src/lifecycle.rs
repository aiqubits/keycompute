//! PostgreSQL request lifecycle recorder.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use keycompute_types::{
    AttemptRef, AttemptResponseMeta, AttemptTraceFinish, AttemptTraceStart,
    RequestLifecycleRecorder, RequestStatus, RequestTraceFinish, RequestTraceStart, RouteType,
    TraceWriteError,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::DbRouter;

#[derive(Clone)]
pub struct PostgresRequestLifecycleRecorder {
    pool: Arc<DbRouter>,
    synchronous_write_timeout: Duration,
    intermediate_workers: Arc<Mutex<HashMap<Uuid, IntermediateWorkerHandle>>>,
    intermediate_failed_requests: Arc<Mutex<HashSet<Uuid>>>,
}

#[derive(Clone)]
struct IntermediateWorkerHandle {
    generation: Uuid,
    tx: mpsc::Sender<IntermediateUpdate>,
}

const INTERMEDIATE_QUEUE_CAPACITY_PER_REQUEST: usize = 64;
const INTERMEDIATE_WORKER_IDLE_TIMEOUT: Duration = Duration::from_secs(1);

enum IntermediateUpdate {
    AttemptMeta {
        request_id: Uuid,
        attempt_id: Uuid,
        meta: AttemptResponseMeta,
    },
    AttemptFirstContent {
        request_id: Uuid,
        attempt_id: Uuid,
        at: DateTime<Utc>,
    },
    ClientFirstContent {
        request_id: Uuid,
        at: DateTime<Utc>,
    },
    Barrier {
        request_id: Uuid,
        done: oneshot::Sender<bool>,
    },
}

impl IntermediateUpdate {
    fn request_id(&self) -> Uuid {
        match self {
            Self::AttemptMeta { request_id, .. }
            | Self::AttemptFirstContent { request_id, .. }
            | Self::ClientFirstContent { request_id, .. }
            | Self::Barrier { request_id, .. } => *request_id,
        }
    }
}

fn take_intermediate_failure(failures: &Mutex<HashSet<Uuid>>, request_id: Uuid) -> bool {
    failures
        .lock()
        .expect("intermediate failure state poisoned")
        .remove(&request_id)
}

impl std::fmt::Debug for PostgresRequestLifecycleRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresRequestLifecycleRecorder")
            .field("synchronous_write_timeout", &self.synchronous_write_timeout)
            .finish_non_exhaustive()
    }
}

impl PostgresRequestLifecycleRecorder {
    pub fn new(pool: Arc<DbRouter>) -> Self {
        let intermediate_failed_requests = Arc::new(Mutex::new(HashSet::new()));
        Self {
            pool,
            synchronous_write_timeout: Duration::from_millis(250),
            intermediate_workers: Arc::new(Mutex::new(HashMap::new())),
            intermediate_failed_requests,
        }
    }
    pub fn with_start_timeout(mut self, timeout: Duration) -> Self {
        self.synchronous_write_timeout = timeout;
        self
    }
    pub fn with_synchronous_write_timeout(mut self, timeout: Duration) -> Self {
        self.synchronous_write_timeout = timeout;
        self
    }
    pub fn pool(&self) -> &Arc<DbRouter> {
        &self.pool
    }

    fn spawn_intermediate_worker(&self, request_id: Uuid) -> IntermediateWorkerHandle {
        let generation = Uuid::new_v4();
        let (tx, rx) = mpsc::channel(INTERMEDIATE_QUEUE_CAPACITY_PER_REQUEST);
        tokio::spawn(run_intermediate_worker(
            Arc::clone(&self.pool),
            Arc::clone(&self.intermediate_workers),
            Arc::clone(&self.intermediate_failed_requests),
            request_id,
            generation,
            rx,
        ));
        IntermediateWorkerHandle { generation, tx }
    }

    fn try_send_intermediate(&self, mut update: IntermediateUpdate) -> Result<(), TraceWriteError> {
        let request_id = update.request_id();
        // Holding the map lock through `try_send` closes the idle-worker race:
        // a worker checks the same lock and its receiver before retiring.
        for _ in 0..2 {
            let mut workers = self
                .intermediate_workers
                .lock()
                .expect("intermediate worker state poisoned");
            let handle = workers
                .entry(request_id)
                .or_insert_with(|| self.spawn_intermediate_worker(request_id))
                .clone();
            match handle.tx.try_send(update) {
                Ok(()) => return Ok(()),
                Err(mpsc::error::TrySendError::Closed(returned)) => {
                    if workers
                        .get(&request_id)
                        .is_some_and(|current| current.generation == handle.generation)
                    {
                        workers.remove(&request_id);
                    }
                    update = returned;
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    return Err(TraceWriteError(
                        "intermediate request queue full".to_string(),
                    ));
                }
            }
        }
        Err(TraceWriteError(
            "intermediate request worker closed".to_string(),
        ))
    }

    fn enqueue_intermediate(&self, update: IntermediateUpdate) -> Result<(), TraceWriteError> {
        let request_id = update.request_id();
        self.try_send_intermediate(update).map_err(|error| {
            self.intermediate_failed_requests
                .lock()
                .expect("intermediate failure state poisoned")
                .insert(request_id);
            keycompute_observability::metrics::TRACE_INTERMEDIATE_QUEUE_DROPS_TOTAL.inc();
            keycompute_observability::metrics::TRACE_WRITE_FAILURE_TOTAL
                .with_label_values(&["intermediate"])
                .inc();
            TraceWriteError(format!("intermediate queue full or closed: {error}"))
        })
    }

    fn schedule_intermediate_cleanup(&self, request_id: Uuid) {
        let handle = {
            let mut workers = self
                .intermediate_workers
                .lock()
                .expect("intermediate worker state poisoned");
            workers
                .entry(request_id)
                .or_insert_with(|| self.spawn_intermediate_worker(request_id))
                .clone()
        };
        let failed_requests = Arc::clone(&self.intermediate_failed_requests);
        tokio::spawn(async move {
            let (done, completed) = oneshot::channel();
            if handle
                .tx
                .send(IntermediateUpdate::Barrier { request_id, done })
                .await
                .is_ok()
            {
                // The worker removes the request marker before acknowledging
                // the barrier. Awaiting the acknowledgement keeps cleanup
                // ordered behind every update that was already queued.
                let _ = completed.await;
            }
            // Also cover a worker that closed before accepting or
            // acknowledging the cleanup barrier.
            take_intermediate_failure(&failed_requests, request_id);
        });
    }

    async fn flush_intermediate_queue(&self, request_id: Uuid) -> Result<(), TraceWriteError> {
        let (done_tx, done_rx) = oneshot::channel();
        if let Err(error) = self.try_send_intermediate(IntermediateUpdate::Barrier {
            request_id,
            done: done_tx,
        }) {
            // The client-facing path remains non-blocking. A detached barrier
            // drains any already queued request-local writes and consumes their
            // failure marker once capacity becomes available.
            self.schedule_intermediate_cleanup(request_id);
            return Err(error);
        }
        match tokio::time::timeout(Duration::from_millis(500), done_rx).await {
            Ok(Ok(false)) => Ok(()),
            Ok(Ok(true)) => Err(TraceWriteError(
                "one or more intermediate writes failed".to_string(),
            )),
            Ok(Err(error)) => {
                take_intermediate_failure(&self.intermediate_failed_requests, request_id);
                Err(TraceWriteError(format!(
                    "intermediate worker closed before barrier: {error}"
                )))
            }
            // The barrier is already queued. Even though the caller stops
            // waiting, the worker will process it in order and clear the
            // request-scoped failure marker before its acknowledgement fails.
            Err(_) => Err(TraceWriteError(
                "timed out waiting for intermediate barrier".to_string(),
            )),
        }
    }
}

async fn process_intermediate_update(
    pool: &DbRouter,
    failed_requests: &Mutex<HashSet<Uuid>>,
    update: IntermediateUpdate,
) {
    if let IntermediateUpdate::Barrier { request_id, done } = update {
        let failed = take_intermediate_failure(failed_requests, request_id);
        let _ = done.send(failed);
        return;
    }

    let request_id = update.request_id();
    match tokio::time::timeout(
        Duration::from_millis(250),
        apply_intermediate_update(pool, update),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            failed_requests
                .lock()
                .expect("intermediate failure state poisoned")
                .insert(request_id);
            keycompute_observability::metrics::TRACE_WRITE_FAILURE_TOTAL
                .with_label_values(&["intermediate"])
                .inc();
            tracing::warn!(%error, "monitoring intermediate write failed");
        }
        Err(error) => {
            failed_requests
                .lock()
                .expect("intermediate failure state poisoned")
                .insert(request_id);
            keycompute_observability::metrics::TRACE_WRITE_FAILURE_TOTAL
                .with_label_values(&["intermediate"])
                .inc();
            tracing::warn!(%error, "monitoring intermediate write timed out");
        }
    }
}

async fn run_intermediate_worker(
    pool: Arc<DbRouter>,
    workers: Arc<Mutex<HashMap<Uuid, IntermediateWorkerHandle>>>,
    failed_requests: Arc<Mutex<HashSet<Uuid>>>,
    request_id: Uuid,
    generation: Uuid,
    mut rx: mpsc::Receiver<IntermediateUpdate>,
) {
    loop {
        let update = match tokio::time::timeout(INTERMEDIATE_WORKER_IDLE_TIMEOUT, rx.recv()).await {
            Ok(Some(update)) => Some(update),
            Ok(None) => None,
            Err(_) => {
                // Coordinate retirement with producers. A producer holds this
                // same lock through `try_send`, so checking the receiver while
                // locked prevents a successfully enqueued update from being
                // dropped as the idle worker exits.
                let mut handles = workers.lock().expect("intermediate worker state poisoned");
                if !handles
                    .get(&request_id)
                    .is_some_and(|handle| handle.generation == generation)
                {
                    None
                } else {
                    match rx.try_recv() {
                        Ok(update) => Some(update),
                        Err(mpsc::error::TryRecvError::Empty)
                        | Err(mpsc::error::TryRecvError::Disconnected) => {
                            handles.remove(&request_id);
                            None
                        }
                    }
                }
            }
        };
        let Some(update) = update else {
            break;
        };
        process_intermediate_update(pool.as_ref(), failed_requests.as_ref(), update).await;
    }

    let mut handles = workers.lock().expect("intermediate worker state poisoned");
    if handles
        .get(&request_id)
        .is_some_and(|handle| handle.generation == generation)
    {
        handles.remove(&request_id);
    }
}

fn write_error(error: impl std::fmt::Display) -> TraceWriteError {
    TraceWriteError(error.to_string())
}

async fn bounded_trace_write<T, F>(
    timeout: Duration,
    operation: &'static str,
    future: F,
) -> Result<T, TraceWriteError>
where
    F: Future<Output = Result<T, TraceWriteError>>,
{
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| TraceWriteError(format!("{operation} timed out")))?
}

fn truncate(value: Option<String>, max: usize) -> Option<String> {
    value.map(|value| value.chars().take(max).collect())
}

const ATTEMPT_META_UPDATE_SQL: &str = "UPDATE gateway_request_attempts SET http_status=COALESCE(http_status,$1), headers_received_at=COALESCE(headers_received_at,$2), upstream_request_id=COALESCE(upstream_request_id,$3), updated_at=NOW() WHERE id=$4";
const ATTEMPT_FIRST_CONTENT_UPDATE_SQL: &str = "UPDATE gateway_request_attempts SET first_content_at=COALESCE(first_content_at,$1), updated_at=NOW() WHERE id=$2";

async fn apply_intermediate_update(
    pool: &DbRouter,
    update: IntermediateUpdate,
) -> Result<(), TraceWriteError> {
    let statement = match update {
        IntermediateUpdate::AttemptMeta {
            attempt_id, meta, ..
        } => Statement::from_sql_and_values(
            DbBackend::Postgres,
            ATTEMPT_META_UPDATE_SQL,
            [
                meta.http_status.into(),
                meta.headers_received_at.into(),
                truncate(meta.upstream_request_id, 128).into(),
                attempt_id.into(),
            ],
        ),
        IntermediateUpdate::AttemptFirstContent { attempt_id, at, .. } => {
            Statement::from_sql_and_values(
                DbBackend::Postgres,
                ATTEMPT_FIRST_CONTENT_UPDATE_SQL,
                [at.into(), attempt_id.into()],
            )
        }
        IntermediateUpdate::ClientFirstContent { request_id, at } => {
            Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE gateway_requests SET client_first_content_at=COALESCE(client_first_content_at,$1), updated_at=NOW() WHERE request_id=$2",
                [at.into(), request_id.into()],
            )
        }
        IntermediateUpdate::Barrier { .. } => return Ok(()),
    };
    let result = pool.execute(statement).await.map_err(write_error)?;
    if result.rows_affected() != 1 {
        return Err(TraceWriteError(
            "intermediate update target is missing".to_string(),
        ));
    }
    Ok(())
}

#[async_trait]
impl RequestLifecycleRecorder for PostgresRequestLifecycleRecorder {
    async fn start_request(&self, request: RequestTraceStart) -> Result<(), TraceWriteError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO gateway_requests (
                request_id, client_request_id, tenant_id, user_id, produce_ai_key_id,
                protocol, request_path, requested_model, is_stream, status, received_at,
                billing_status, trace_quality, trace_version
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'received',$10,'pending','actual',1)
            ON CONFLICT (request_id) DO NOTHING"#,
            [
                request.request_id.into(),
                truncate(request.client_request_id, 128).into(),
                request.tenant_id.into(),
                request.user_id.into(),
                request.produce_ai_key_id.into(),
                request.protocol.into(),
                request.request_path.into(),
                request.requested_model.into(),
                request.is_stream.into(),
                request.received_at.into(),
            ],
        );
        bounded_trace_write(self.synchronous_write_timeout, "start_request", async {
            self.pool.execute(statement).await.map_err(write_error)
        })
        .await?;
        Ok(())
    }

    async fn set_route(
        &self,
        request_id: Uuid,
        route: RouteType,
        status: RequestStatus,
    ) -> Result<(), TraceWriteError> {
        bounded_trace_write(self.synchronous_write_timeout, "set_route", async {
            self.pool
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE gateway_requests SET route_type=$1, status=$2, updated_at=NOW() WHERE request_id=$3 AND finished_at IS NULL",
                    [route.as_str().into(), status.as_str().into(), request_id.into()],
                ))
                .await
                .map_err(write_error)
        })
        .await?;
        Ok(())
    }

    async fn start_attempt(
        &self,
        attempt: AttemptTraceStart,
    ) -> Result<AttemptRef, TraceWriteError> {
        bounded_trace_write(self.synchronous_write_timeout, "start_attempt", async {
            let tx = self.pool.begin().await.map_err(write_error)?;
            tx.query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT request_id FROM gateway_requests WHERE request_id=$1 AND finished_at IS NULL FOR UPDATE",
                [attempt.request_id.into()],
            )).await.map_err(write_error)?.ok_or_else(|| TraceWriteError("request is missing or terminal".to_string()))?;
            if attempt.route_type == RouteType::Node
                && let (Some(task_id), Some(lease_id)) = (attempt.node_task_id, attempt.lease_id)
                && let Some(row) = tx.query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT id,attempt_no FROM gateway_request_attempts WHERE node_task_id=$1 AND lease_id=$2",
                    [task_id.into(),lease_id.into()],
                )).await.map_err(write_error)?
            {
                let reference=AttemptRef{id:row.try_get("","id").map_err(write_error)?,attempt_no:row.try_get("","attempt_no").map_err(write_error)?};
                tx.commit().await.map_err(write_error)?;
                return Ok(reference);
            }
            let row = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    r#"INSERT INTO gateway_request_attempts (
                request_id, attempt_no, attempt_kind, route_type, model, status,
                provider_name, account_id, node_task_id, node_id, session_id, lease_id, started_at
            ) SELECT $1, COALESCE(MAX(attempt_no),0)+1, $2,$3,$4,'running',$5,$6,$7,$8,$9,$10,$11
              FROM gateway_request_attempts WHERE request_id=$1
              RETURNING id, attempt_no"#,
                [
                    attempt.request_id.into(),
                    attempt.attempt_kind.as_str().into(),
                    attempt.route_type.as_str().into(),
                    attempt.model.into(),
                    attempt.provider_name.into(),
                    attempt.account_id.into(),
                    attempt.node_task_id.into(),
                    attempt.node_id.into(),
                    attempt.session_id.into(),
                    attempt.lease_id.into(),
                    attempt.started_at.into(),
                ],
                ))
                .await
                .map_err(write_error)?
                .ok_or_else(|| TraceWriteError("attempt insert returned no row".to_string()))?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE gateway_requests SET status='running', updated_at=NOW() WHERE request_id=$1",
                [attempt.request_id.into()],
            ))
            .await
            .map_err(write_error)?;
            tx.commit().await.map_err(write_error)?;
            Ok(AttemptRef {
                id: row.try_get("", "id").map_err(write_error)?,
                attempt_no: row.try_get("", "attempt_no").map_err(write_error)?,
            })
        })
        .await
    }

    async fn mark_trace_partial(&self, request_id: Uuid) -> Result<(), TraceWriteError> {
        let result = bounded_trace_write(self.synchronous_write_timeout, "mark_trace_partial", async {
            self.pool
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE gateway_requests SET trace_quality='partial',updated_at=NOW() WHERE request_id=$1",
                    [request_id.into()],
                ))
                .await
                .map_err(write_error)
        })
        .await?;
        if result.rows_affected() != 1 {
            return Err(TraceWriteError(
                "request is missing while marking trace partial".to_string(),
            ));
        }
        Ok(())
    }

    async fn record_attempt_response_meta(
        &self,
        request_id: Uuid,
        attempt_id: Uuid,
        meta: AttemptResponseMeta,
    ) -> Result<(), TraceWriteError> {
        self.enqueue_intermediate(IntermediateUpdate::AttemptMeta {
            request_id,
            attempt_id,
            meta,
        })
    }

    async fn record_attempt_first_content(
        &self,
        request_id: Uuid,
        attempt_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), TraceWriteError> {
        self.enqueue_intermediate(IntermediateUpdate::AttemptFirstContent {
            request_id,
            attempt_id,
            at,
        })
    }

    async fn record_client_first_content(
        &self,
        request_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), TraceWriteError> {
        self.enqueue_intermediate(IntermediateUpdate::ClientFirstContent { request_id, at })
    }

    async fn flush_intermediate_updates(&self, request_id: Uuid) -> Result<(), TraceWriteError> {
        let Err(flush_error) = self.flush_intermediate_queue(request_id).await else {
            return Ok(());
        };

        // A handler-side flush can happen after the request's terminal update,
        // so no later terminal transaction is guaranteed to observe this
        // failure. Persist the degraded quality here and return the original
        // error for logging. The barrier has removed the request from
        // `intermediate_failed_requests`, or its detached replacement will do
        // so after a bounded enqueue wait expires.
        if let Err(partial_error) = self.mark_trace_partial(request_id).await {
            return Err(TraceWriteError(format!(
                "{flush_error}; failed to mark trace partial: {partial_error}"
            )));
        }
        Err(flush_error)
    }

    async fn finish_attempt_and_request(
        &self,
        finish: AttemptTraceFinish,
    ) -> Result<(), TraceWriteError> {
        if !finish.attempt_status.is_terminal()
            || (!finish.is_final && finish.request_status.is_terminal())
        {
            return Err(TraceWriteError(
                "invalid attempt/request terminal update".to_string(),
            ));
        }
        let intermediate_flush_failed = self
            .flush_intermediate_queue(finish.request_id)
            .await
            .is_err();
        bounded_trace_write(
            self.synchronous_write_timeout,
            "finish_attempt_and_request",
            async {
                let tx = self.pool.begin().await.map_err(write_error)?;
                // Keep the same lock order as the stale reconciler (request, then
                // attempt) so a normal completion cannot deadlock with or be
                // overwritten by reconciliation.
                tx.query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT request_id FROM gateway_requests WHERE request_id=$1 FOR UPDATE",
                    [finish.request_id.into()],
                ))
                .await
                .map_err(write_error)?
                .ok_or_else(|| TraceWriteError("request is missing".to_string()))?;
                let (origin, category, code, summary, retryable) = match finish.error {
                    Some(error) => (
                        Some(error.origin.as_str()),
                        Some(error.category.as_str()),
                        truncate(Some(error.code), 128),
                        truncate(error.summary, 512),
                        error.retryable,
                    ),
                    None => (None, None, None, None, None),
                };
                let attempt_update = tx
                    .execute(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        r#"UPDATE gateway_request_attempts SET status=$1, is_final=$2, retryable=$3,
                error_origin=$4, error_category=$5, error_code=$6, error_summary=$7,
                stream_end_reason=$8, stream_error_count=$9, finished_at=$10, updated_at=NOW()
               WHERE id=$11 AND request_id=$12 AND finished_at IS NULL"#,
                        [
                            finish.attempt_status.as_str().into(),
                            finish.is_final.into(),
                            retryable.into(),
                            origin.into(),
                            category.into(),
                            code.clone().into(),
                            summary.into(),
                            finish.stream_end_reason.map(|v| v.as_str()).into(),
                            finish.stream_error_count.into(),
                            finish.finished_at.into(),
                            finish.attempt_id.into(),
                            finish.request_id.into(),
                        ],
                    ))
                    .await
                    .map_err(write_error)?;
                if attempt_update.rows_affected() != 1 {
                    let existing=tx.query_one(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "SELECT status,is_final FROM gateway_request_attempts WHERE id=$1 AND request_id=$2",
                        [finish.attempt_id.into(),finish.request_id.into()],
                    )).await.map_err(write_error)?;
                    let idempotent = existing
                        .and_then(|row| {
                            Some((
                                row.try_get::<String>("", "status").ok()?,
                                row.try_get::<bool>("", "is_final").ok()?,
                            ))
                        })
                        .is_some_and(|(status, is_final)| {
                            status == finish.attempt_status.as_str() && is_final == finish.is_final
                        });
                    if idempotent {
                        if intermediate_flush_failed {
                            tx.execute(Statement::from_sql_and_values(
                                DbBackend::Postgres,
                                "UPDATE gateway_requests SET trace_quality='partial',updated_at=NOW() WHERE request_id=$1",
                                [finish.request_id.into()],
                            ))
                            .await
                            .map_err(write_error)?;
                        }
                        tx.commit().await.map_err(write_error)?;
                        return Ok(());
                    }
                    let _ = tx.rollback().await;
                    return Err(TraceWriteError(
                        "attempt is missing or already terminal".to_string(),
                    ));
                }
                // `is_final` identifies the last upstream/node attempt. A
                // provider attempt may be final while the request remains
                // running until the protocol handler validates or forwards
                // the client response, so request terminalization follows the
                // request status rather than the attempt flag.
                if finish.request_status.is_terminal() {
                    let request_update = tx
                        .execute(Statement::from_sql_and_values(
                            DbBackend::Postgres,
                            r#"UPDATE gateway_requests SET status=$1, error_origin=$2, error_category=$3,
                   error_code=$4, billing_status=CASE WHEN billing_status IN ('succeeded','failed') THEN billing_status ELSE $5 END, finished_at=$6,
                   trace_quality=CASE WHEN $8 THEN 'partial' ELSE trace_quality END, updated_at=NOW()
                   WHERE request_id=$7 AND finished_at IS NULL"#,
                            [
                                finish.request_status.as_str().into(),
                                origin.into(),
                                category.into(),
                                code.into(),
                                finish.billing_status.as_str().into(),
                                finish.finished_at.into(),
                                finish.request_id.into(),
                                intermediate_flush_failed.into(),
                            ],
                        ))
                        .await
                        .map_err(write_error)?;
                    if request_update.rows_affected() != 1 {
                        let existing = tx
                            .query_one(Statement::from_sql_and_values(
                                DbBackend::Postgres,
                                "SELECT finished_at FROM gateway_requests WHERE request_id=$1",
                                [finish.request_id.into()],
                            ))
                            .await
                            .map_err(write_error)?;
                        let handler_already_finished_request = existing
                            .and_then(|row| {
                                row.try_get::<Option<DateTime<Utc>>>("", "finished_at")
                                    .ok()
                            })
                            .flatten()
                            .is_some();
                        if !handler_already_finished_request {
                            let _ = tx.rollback().await;
                            return Err(TraceWriteError(
                                "request is missing or no longer writable".to_string(),
                            ));
                        }
                        // Client delivery and attempt persistence intentionally
                        // run independently. If the handler won the race, keep
                        // its terminal request outcome and commit only the
                        // attempt update from this transaction.
                        if intermediate_flush_failed {
                            tx.execute(Statement::from_sql_and_values(
                                DbBackend::Postgres,
                                "UPDATE gateway_requests SET trace_quality='partial',updated_at=NOW() WHERE request_id=$1",
                                [finish.request_id.into()],
                            ))
                            .await
                            .map_err(write_error)?;
                        }
                    }
                } else {
                    let request_update = tx
                        .execute(Statement::from_sql_and_values(
                            DbBackend::Postgres,
                            "UPDATE gateway_requests SET status=$1,trace_quality=CASE WHEN $3 THEN 'partial' ELSE trace_quality END,updated_at=NOW() WHERE request_id=$2 AND finished_at IS NULL",
                            [
                                finish.request_status.as_str().into(),
                                finish.request_id.into(),
                                intermediate_flush_failed.into(),
                            ],
                        ))
                        .await
                        .map_err(write_error)?;
                    if request_update.rows_affected() != 1 {
                        let existing = tx
                            .query_one(Statement::from_sql_and_values(
                                DbBackend::Postgres,
                                "SELECT finished_at FROM gateway_requests WHERE request_id=$1",
                                [finish.request_id.into()],
                            ))
                            .await
                            .map_err(write_error)?;
                        let handler_already_finished_request = existing
                            .and_then(|row| {
                                row.try_get::<Option<DateTime<Utc>>>("", "finished_at")
                                    .ok()
                            })
                            .flatten()
                            .is_some();
                        if !handler_already_finished_request {
                            let _ = tx.rollback().await;
                            return Err(TraceWriteError(
                                "request is missing or no longer writable".to_string(),
                            ));
                        }
                        if intermediate_flush_failed {
                            tx.execute(Statement::from_sql_and_values(
                                DbBackend::Postgres,
                                "UPDATE gateway_requests SET trace_quality='partial',updated_at=NOW() WHERE request_id=$1",
                                [finish.request_id.into()],
                            ))
                            .await
                            .map_err(write_error)?;
                        }
                    }
                }
                tx.commit().await.map_err(write_error)?;
                Ok(())
            },
        )
        .await
    }

    async fn finish_request_without_attempt(
        &self,
        finish: RequestTraceFinish,
    ) -> Result<(), TraceWriteError> {
        if !finish.status.is_terminal() {
            return Err(TraceWriteError(
                "terminal update requires a terminal status".to_string(),
            ));
        }
        let intermediate_flush_failed = self
            .flush_intermediate_queue(finish.request_id)
            .await
            .is_err();
        let (origin, category, code) = match finish.error {
            Some(error) => (
                Some(error.origin.as_str()),
                Some(error.category.as_str()),
                Some(error.code),
            ),
            None => (None, None, None),
        };
        bounded_trace_write(
            self.synchronous_write_timeout,
            "finish_request_without_attempt",
            async {
                let result=self.pool.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE gateway_requests SET status=$1,error_origin=$2,error_category=$3,error_code=$4,billing_status=CASE WHEN billing_status IN ('succeeded','failed') THEN billing_status ELSE $5 END,finished_at=$6,trace_quality=CASE WHEN $8 OR (route_type='provider_account' AND NOT EXISTS(SELECT 1 FROM gateway_request_attempts a WHERE a.request_id=gateway_requests.request_id)) THEN 'partial' ELSE trace_quality END,updated_at=NOW() WHERE request_id=$7 AND finished_at IS NULL",
                    [finish.status.as_str().into(), origin.into(), category.into(), truncate(code, 128).into(), finish.billing_status.as_str().into(), finish.finished_at.into(), finish.request_id.into(), intermediate_flush_failed.into()],
                )).await.map_err(write_error)?;
                if result.rows_affected() != 1 {
                    let existing = self
                        .pool
                        .write_conn()
                        .query_one(Statement::from_sql_and_values(
                            DbBackend::Postgres,
                            "SELECT status FROM gateway_requests WHERE request_id=$1",
                            [finish.request_id.into()],
                        ))
                        .await
                        .map_err(write_error)?;
                    let idempotent = existing
                        .and_then(|row| row.try_get::<String>("", "status").ok())
                        .is_some_and(|status| status == finish.status.as_str());
                    if !idempotent {
                        return Err(TraceWriteError(
                            "request is missing or already terminal".to_string(),
                        ));
                    }
                }
                Ok(())
            },
        )
        .await
    }

    async fn mark_billing_succeeded(&self, request_id: Uuid) -> Result<(), TraceWriteError> {
        bounded_trace_write(self.synchronous_write_timeout, "mark_billing_succeeded", async {
            self.pool.execute(Statement::from_sql_and_values(DbBackend::Postgres, "UPDATE gateway_requests SET billing_status='succeeded',updated_at=NOW() WHERE request_id=$1", [request_id.into()])).await.map_err(write_error)
        }).await?;
        Ok(())
    }

    async fn mark_billing_failed(&self, request_id: Uuid) -> Result<(), TraceWriteError> {
        bounded_trace_write(self.synchronous_write_timeout, "mark_billing_failed", async {
            self.pool.execute(Statement::from_sql_and_values(DbBackend::Postgres, "UPDATE gateway_requests SET billing_status='failed',updated_at=NOW() WHERE request_id=$1 AND billing_status='pending'", [request_id.into()])).await.map_err(write_error)
        }).await?;
        Ok(())
    }
}

const RECONCILED_BILLING_STATUS: &str = "CASE WHEN billing_status IN ('succeeded','failed') THEN billing_status ELSE 'not_applicable' END";
const STALE_REQUESTS_CTE: &str = r#"WITH stale AS MATERIALIZED (
        SELECT gr.request_id,
               gr.route_type,
               nt.status AS node_status,
               gr.route_type='node' AND nt.status IN ('succeeded','image_succeeded') AS node_execution_succeeded,
               CASE WHEN gr.route_type='node' AND nt.status IN ('succeeded','image_succeeded') THEN 'failed'
                    WHEN gr.route_type='node' AND nt.status='failed' THEN 'failed'
                    WHEN gr.route_type='node' AND nt.status='expired' THEN 'timed_out'
                    ELSE 'timed_out' END AS terminal_status,
               CASE WHEN gr.route_type='node' AND nt.status IN ('succeeded','image_succeeded') THEN 'succeeded'
                    WHEN gr.route_type='node' AND nt.status='failed' THEN 'failed'
                    WHEN gr.route_type='node' AND nt.status='expired' THEN 'expired'
                    ELSE 'timed_out' END AS attempt_status
        FROM gateway_requests gr LEFT JOIN node_tasks nt ON nt.request_id=gr.request_id
        WHERE gr.finished_at IS NULL AND gr.updated_at < NOW()-make_interval(secs => $1)
          AND (gr.route_type IS DISTINCT FROM 'node' OR nt.status IN ('failed','expired','succeeded','image_succeeded') OR nt.id IS NULL)
        ORDER BY gr.updated_at FOR UPDATE OF gr SKIP LOCKED LIMIT $2
    )"#;
const STALE_BILLING_RECONCILIATION_SQL: &str = r#"WITH stale_billing AS MATERIALIZED (
    SELECT gr.request_id
    FROM gateway_requests gr
    WHERE gr.finished_at IS NOT NULL
      AND gr.billing_status='pending'
      AND gr.finished_at < NOW()-make_interval(secs => $1)
    ORDER BY gr.finished_at
    FOR UPDATE OF gr SKIP LOCKED
    LIMIT $2
)
UPDATE gateway_requests gr
SET billing_status=CASE
        WHEN EXISTS(SELECT 1 FROM usage_logs ul WHERE ul.request_id=gr.request_id)
        THEN 'succeeded' ELSE 'failed' END,
    updated_at=NOW()
FROM stale_billing s
WHERE gr.request_id=s.request_id AND gr.billing_status='pending'"#;

/// Repair abandoned traces without overwriting requests that completed normally.
pub async fn reconcile_stale_requests(
    pool: &DbRouter,
    older_than_seconds: i64,
    batch_size: i64,
) -> Result<u64, crate::DbError> {
    let tx = pool.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("{STALE_REQUESTS_CTE} UPDATE gateway_request_attempts a SET
          status=s.attempt_status,
          is_final=CASE WHEN a.id=(SELECT latest.id FROM gateway_request_attempts latest WHERE latest.request_id=a.request_id AND latest.finished_at IS NULL ORDER BY latest.attempt_no DESC LIMIT 1) AND NOT EXISTS(SELECT 1 FROM gateway_request_attempts final_attempt WHERE final_attempt.request_id=a.request_id AND final_attempt.is_final) THEN TRUE ELSE a.is_final END,
          finished_at=NOW(),
          stream_end_reason=CASE WHEN s.node_execution_succeeded THEN 'completed' WHEN s.route_type='node' AND s.node_status='failed' THEN 'upstream_error' ELSE 'timeout' END,
          error_origin=CASE WHEN s.node_execution_succeeded THEN NULL WHEN s.route_type='node' THEN 'node' ELSE 'gateway' END,
          error_category=CASE WHEN s.node_execution_succeeded THEN NULL WHEN s.route_type='node' AND s.node_status='failed' THEN 'node_failed' WHEN s.route_type='node' THEN 'node_expired' ELSE 'timeout' END,
          error_code=CASE WHEN s.node_execution_succeeded THEN NULL WHEN s.route_type='node' AND s.node_status='failed' THEN 'node_task_failed' WHEN s.route_type='node' THEN 'node_task_expired' ELSE 'trace_stale_reconciled' END,
          updated_at=NOW() FROM stale s WHERE a.request_id=s.request_id AND a.finished_at IS NULL"),
        [older_than_seconds.into(), batch_size.into()],
    )).await?;
    let request_result = tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("{STALE_REQUESTS_CTE} UPDATE gateway_requests gr SET status=s.terminal_status,
          error_origin=CASE WHEN s.node_execution_succeeded THEN 'gateway' WHEN s.route_type='node' THEN 'node' ELSE 'gateway' END,
          error_category=CASE WHEN s.node_execution_succeeded THEN 'internal' WHEN s.route_type='node' AND s.node_status='failed' THEN 'node_failed' WHEN s.route_type='node' THEN 'node_expired' ELSE 'timeout' END,
          error_code=CASE WHEN s.node_execution_succeeded THEN 'node_client_response_missing' WHEN s.route_type='node' AND s.node_status='failed' THEN 'node_task_failed' WHEN s.route_type='node' THEN 'node_task_expired' ELSE 'trace_stale_reconciled' END,
          trace_quality='partial',billing_status={RECONCILED_BILLING_STATUS},finished_at=NOW(),updated_at=NOW() FROM stale s WHERE gr.request_id=s.request_id AND gr.finished_at IS NULL"),
        [older_than_seconds.into(), batch_size.into()],
    )).await?;
    let billing_result = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            STALE_BILLING_RECONCILIATION_SQL,
            [older_than_seconds.into(), batch_size.into()],
        ))
        .await?;
    tx.commit().await?;
    Ok(request_result
        .rows_affected()
        .saturating_add(billing_result.rows_affected()))
}

#[cfg(test)]
mod tests {
    use super::{
        ATTEMPT_FIRST_CONTENT_UPDATE_SQL, ATTEMPT_META_UPDATE_SQL, RECONCILED_BILLING_STATUS,
        STALE_BILLING_RECONCILIATION_SQL, STALE_REQUESTS_CTE, bounded_trace_write,
        take_intermediate_failure,
    };
    use std::{collections::HashSet, future::pending, sync::Mutex, time::Duration};
    use uuid::Uuid;

    #[test]
    fn stale_reconciliation_preserves_committed_billing_facts() {
        assert!(RECONCILED_BILLING_STATUS.contains("IN ('succeeded','failed')"));
        assert!(RECONCILED_BILLING_STATUS.contains("THEN billing_status"));
        assert!(!RECONCILED_BILLING_STATUS.contains("terminal_status='succeeded'"));
    }

    #[test]
    fn stale_terminal_billing_is_resolved_from_the_usage_ledger() {
        assert!(STALE_BILLING_RECONCILIATION_SQL.contains("finished_at IS NOT NULL"));
        assert!(STALE_BILLING_RECONCILIATION_SQL.contains("billing_status='pending'"));
        assert!(STALE_BILLING_RECONCILIATION_SQL.contains("FROM usage_logs"));
        assert!(STALE_BILLING_RECONCILIATION_SQL.contains("THEN 'succeeded' ELSE 'failed'"));
    }

    #[test]
    fn stale_node_success_requires_client_response_evidence() {
        assert!(
            STALE_REQUESTS_CTE
                .contains("nt.status IN ('succeeded','image_succeeded') THEN 'failed'")
        );
        assert!(
            STALE_REQUESTS_CTE
                .contains("nt.status IN ('failed','expired','succeeded','image_succeeded')")
        );
        assert!(STALE_REQUESTS_CTE.contains("END AS attempt_status"));
    }

    #[test]
    fn stale_request_age_uses_last_lifecycle_activity() {
        assert!(STALE_REQUESTS_CTE.contains("gr.updated_at < NOW()-make_interval(secs => $1)"));
        assert!(STALE_REQUESTS_CTE.contains("ORDER BY gr.updated_at"));
        assert!(!STALE_REQUESTS_CTE.contains("gr.received_at <"));
    }

    #[test]
    fn late_intermediate_attempt_updates_can_complete_terminal_rows() {
        assert!(!ATTEMPT_META_UPDATE_SQL.contains("finished_at IS NULL"));
        assert!(!ATTEMPT_FIRST_CONTENT_UPDATE_SQL.contains("finished_at IS NULL"));
        assert!(ATTEMPT_META_UPDATE_SQL.contains("COALESCE"));
        assert!(ATTEMPT_FIRST_CONTENT_UPDATE_SQL.contains("COALESCE"));
    }

    #[test]
    fn intermediate_failure_is_scoped_to_its_request() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let failures = Mutex::new(HashSet::from([first]));

        assert!(!take_intermediate_failure(&failures, second));
        assert!(take_intermediate_failure(&failures, first));
    }

    #[tokio::test]
    async fn synchronous_trace_writes_are_time_bounded() {
        for operation in [
            "finish_attempt_and_request",
            "finish_request_without_attempt",
            "mark_billing_succeeded",
            "mark_billing_failed",
        ] {
            let timeout = bounded_trace_write(
                Duration::from_millis(1),
                operation,
                pending::<Result<(), keycompute_types::TraceWriteError>>(),
            )
            .await
            .unwrap_err();
            assert_eq!(timeout.0, format!("{operation} timed out"));
        }

        let completed = bounded_trace_write(Duration::from_secs(1), "test_write", async {
            Ok::<_, keycompute_types::TraceWriteError>(42)
        })
        .await
        .unwrap();
        assert_eq!(completed, 42);
    }
}
