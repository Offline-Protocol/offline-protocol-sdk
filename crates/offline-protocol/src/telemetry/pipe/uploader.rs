//! The HTTPS uploader: one batch at a time, oldest first, with idempotent
//! retry.
//!
//! Every batch carries a `batch_id` that is reused on every resend, and the
//! ingest deduplicates on it, so a batch may be sent more than once and can
//! never be counted more than once. That is what lets the queue be
//! at-least-once: a crash between a send and its acknowledgement replays the
//! batch, and the replay is answered `200 {deduplicated: true}`.
//!
//! # What each answer means
//!
//! | Answer | Action |
//! |---|---|
//! | 2xx | The ingest owns the batch; delete it, reset the backoff |
//! | 3xx | Never followed, so it arrives as an answer; drop it, keep going |
//! | 413 | The batch can never fit; drop it, keep going |
//! | 401, 403 | The key or the app id is wrong; keep the batch, record the error, stop until the next `enable_telemetry` |
//! | 429 | Wait `Retry-After` when given, else back off |
//! | 408, 5xx, no answer | Back off: 1 s doubling to 15 min, plus up to a second of jitter |
//! | any other 4xx | The bytes will never be accepted; drop, keep going |
//!
//! A queued batch older than six days is dropped unsent: the ingest
//! deduplicates for seven, and a batch replayed past that window would be
//! ingested twice.
//!
//! The uploader never takes the emit path's lock and holds the store only
//! while it is the only thread that could touch it.

use std::time::{Duration, Instant};

use crate::telemetry::pipe::store::BatchStore;

/// Where every batch goes. Fixed at build time; see `build.rs`.
pub(crate) const TELEMETRY_INGEST_URL: &str = env!("OFFLINE_TELEMETRY_ENDPOINT");
/// The SDK version stamped on every batch.
pub(crate) const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Backoff floor on a retryable failure.
pub(crate) const BACKOFF_BASE_MS: i64 = 1_000;
/// Backoff ceiling.
pub(crate) const BACKOFF_MAX_MS: i64 = 15 * 60_000;
/// Additive jitter is drawn from `[0, BACKOFF_JITTER_MS)`.
pub(crate) const BACKOFF_JITTER_MS: u64 = 1_000;
/// A batch older than this is dropped rather than sent.
pub(crate) const MAX_BATCH_AGE_MS: i64 = 6 * 24 * 60 * 60_000;
/// Non-final flushes are deferred below this battery level unless charging.
pub(crate) const BATTERY_DEFER_BELOW: u8 = 15;

pub(crate) const HEADER_APP_ID: &str = "X-Mesh-Analytics-App-Id";
pub(crate) const HEADER_IDEMPOTENCY: &str = "Idempotency-Key";

/// One HTTP request, as the client sees it.
pub(crate) struct Request<'a> {
    pub(crate) url: &'static str,
    pub(crate) headers: Vec<(&'static str, &'a str)>,
    pub(crate) body: &'a str,
}

/// One HTTP answer, reduced to what the uploader classifies.
#[derive(Debug, Clone)]
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) retry_after: Option<String>,
    pub(crate) body: String,
}

/// The transport the uploader posts through. Implemented by `UreqClient` and
/// by the test doubles.
pub(crate) trait HttpClient: Send {
    /// Posts one request. `Err` is a transport failure: no answer arrived.
    fn post(&mut self, request: &Request<'_>) -> Result<Response, String>;
}

/// Outcome of one send attempt, after classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Accepted (or a deduplicated replay): `accepted` events counted, or
    /// the whole batch when the answer carried no count.
    Ok { accepted: Option<u64> },
    /// Retry later; `after_ms` is an explicit server-given delay.
    Retry { after_ms: Option<i64> },
    /// Never resend these bytes.
    Drop,
    /// Never resend anything until re-enabled.
    AuthHalt,
}

/// What one drain did, for the caller's stats.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DrainReport {
    pub(crate) sent_events: u64,
    pub(crate) accepted_events: u64,
    pub(crate) dropped_events: u64,
    pub(crate) batches_sent: u32,
    pub(crate) halted: bool,
}

pub(crate) struct Uploader {
    client: Box<dyn HttpClient>,
    authorization: String,
    app_id: String,
    user_agent: String,
    retry_attempt: u32,
    /// Epoch ms before which sends are suppressed.
    next_retry_at_ms: Option<i64>,
    auth_halted: bool,
    last_error: Option<String>,
    jitter: Box<dyn FnMut() -> u64 + Send>,
}

impl Uploader {
    pub(crate) fn new(
        client: Box<dyn HttpClient>,
        api_key: &str,
        app_id: &str,
        user_agent: String,
        jitter: Box<dyn FnMut() -> u64 + Send>,
    ) -> Self {
        Self {
            client,
            authorization: format!("Bearer {api_key}"),
            app_id: app_id.to_string(),
            user_agent,
            retry_attempt: 0,
            next_retry_at_ms: None,
            auth_halted: false,
            last_error: None,
            jitter,
        }
    }

    /// Whether a 401 or 403 stopped the uploader.
    #[cfg(test)]
    pub(crate) fn is_halted(&self) -> bool {
        self.auth_halted
    }

    /// The moment the backoff lifts, if one is in force.
    pub(crate) fn next_retry_at_ms(&self) -> Option<i64> {
        self.next_retry_at_ms
    }

    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Sends queued batches oldest first until one must be retried, the
    /// queue empties, `max_batches` were sent, or `deadline` passes.
    pub(crate) fn drain(
        &mut self,
        store: &mut BatchStore,
        now_ms: i64,
        max_batches: u32,
        deadline: Option<Instant>,
    ) -> DrainReport {
        let mut report = DrainReport::default();
        loop {
            if self.auth_halted {
                report.halted = true;
                break;
            }
            if self.next_retry_at_ms.is_some_and(|at| now_ms < at) {
                break;
            }
            if report.batches_sent >= max_batches {
                break;
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
            let Some(head) = store.front() else {
                break;
            };
            if now_ms - head.created_ms > MAX_BATCH_AGE_MS {
                let count = u64::from(head.event_count);
                store.pop_front();
                report.dropped_events += count;
                continue;
            }
            let count = u64::from(head.event_count);
            let batch_id = head.batch_id.to_string();
            let outcome = self.send(&batch_id, &head.body, now_ms);
            tracing::debug!(
                target: "offline_protocol::telemetry::pipe",
                batch_id = %batch_id,
                events = count,
                outcome = ?outcome,
                "telemetry batch sent"
            );
            match outcome {
                Outcome::Ok { accepted } => {
                    store.pop_front();
                    report.sent_events += count;
                    report.accepted_events += accepted.unwrap_or(count);
                    report.batches_sent += 1;
                    // Cleared only here. `last_error` is what the stats
                    // surface as "the most recent send failure", and an
                    // accepted batch is the one event that makes an earlier
                    // one no longer current. A drop also resets the backoff
                    // but keeps its error, because the drop is the error.
                    self.last_error = None;
                    self.reset_backoff();
                }
                Outcome::Drop => {
                    store.pop_front();
                    report.dropped_events += count;
                    self.reset_backoff();
                }
                Outcome::AuthHalt => {
                    // The batch stays queued. A 401 or 403 is a configuration
                    // fault the developer fixes and re-enables through, not
                    // bytes the ingest will never take, and the halt already
                    // stops every later send: dropping the head would lose one
                    // batch and buy nothing. The caps still bound the queue if
                    // the key is never fixed.
                    self.auth_halted = true;
                    report.halted = true;
                    break;
                }
                Outcome::Retry { after_ms } => {
                    self.schedule_backoff(now_ms, after_ms);
                    break;
                }
            }
        }
        store.flush_index();
        report
    }

    fn send(&mut self, batch_id: &str, body: &str, now_ms: i64) -> Outcome {
        let request = Request {
            url: TELEMETRY_INGEST_URL,
            headers: vec![
                ("Authorization", self.authorization.as_str()),
                (HEADER_APP_ID, self.app_id.as_str()),
                ("Content-Type", "application/json"),
                (HEADER_IDEMPOTENCY, batch_id),
                ("User-Agent", self.user_agent.as_str()),
            ],
            body,
        };
        let response = match self.client.post(&request) {
            Ok(response) => response,
            Err(err) => {
                self.last_error = Some(err);
                return Outcome::Retry { after_ms: None };
            }
        };
        classify_response(&response, now_ms, &mut self.last_error)
    }

    fn reset_backoff(&mut self) {
        self.retry_attempt = 0;
        self.next_retry_at_ms = None;
    }

    fn schedule_backoff(&mut self, now_ms: i64, after_ms: Option<i64>) {
        let delay = match after_ms {
            Some(after) => after.clamp(0, BACKOFF_MAX_MS),
            None => {
                let exponent = self.retry_attempt.min(20);
                let capped = (BACKOFF_BASE_MS << exponent).min(BACKOFF_MAX_MS);
                let jitter = ((self.jitter)() % BACKOFF_JITTER_MS) as i64;
                capped + jitter
            }
        };
        self.retry_attempt = self.retry_attempt.saturating_add(1);
        self.next_retry_at_ms = Some(now_ms + delay);
    }
}

/// Classifies one answer. Public to the module so the status matrix can be
/// tested without a client.
///
/// `now_ms` is the pipe's clock rather than the wall clock, because a
/// `Retry-After` given as an HTTP date is a delay only relative to the same
/// clock the backoff is scheduled against.
pub(crate) fn classify_response(
    response: &Response,
    now_ms: i64,
    last_error: &mut Option<String>,
) -> Outcome {
    let status = response.status;
    if (200..300).contains(&status) {
        return Outcome::Ok {
            accepted: accepted_count(&response.body),
        };
    }
    *last_error = Some(format!("ingest responded {status}"));
    match status {
        401 | 403 => Outcome::AuthHalt,
        408 | 500..=599 => Outcome::Retry { after_ms: None },
        429 => Outcome::Retry {
            after_ms: response
                .retry_after
                .as_deref()
                .and_then(|value| parse_retry_after(value, now_ms)),
        },
        // 413 included: the ingest's body limit is far above the largest
        // batch the pipe cuts, so an oversized batch is a pathological
        // event, and splitting it would only send the pathology twice.
        //
        // A 3xx lands here too. The agent follows no redirect (see
        // `UreqClient::with_timeouts`), so a redirect arrives as an answer,
        // and resending to the endpoint fixed at build time would only draw
        // the same answer again.
        _ => Outcome::Drop,
    }
}

/// The number of events the ingest counted for a 2xx answer.
///
/// A 202 body carries `accepted`; a 200 replay carries `deduplicated: true`
/// and counted nothing new. A body that is neither is `None`, which the
/// caller reads as the batch having been accepted whole, since that is what
/// a bare 2xx means.
fn accepted_count(body: &str) -> Option<u64> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    if let Some(accepted) = value.get("accepted").and_then(|v| v.as_u64()) {
        return Some(accepted);
    }
    if value.get("deduplicated").and_then(|v| v.as_bool()) == Some(true) {
        return Some(0);
    }
    None
}

/// Parses a `Retry-After` value, seconds or an HTTP date, into a delay.
pub(crate) fn parse_retry_after(value: &str, now_ms: i64) -> Option<i64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some((seconds as i64).saturating_mul(1000));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some((date.timestamp_millis() - now_ms).max(0))
}

/// Whether a non-final flush should wait for a charger.
pub(crate) fn battery_defers(battery_level: Option<u8>, is_charging: bool) -> bool {
    matches!(battery_level, Some(level) if level < BATTERY_DEFER_BELOW) && !is_charging
}

/// The `User-Agent` every request carries.
pub(crate) fn user_agent(os_token: &str, os_major: u16) -> String {
    format!("offline-protocol-sdk/{SDK_VERSION} ({os_token}/{os_major})")
}

/// The production client: TLS through rustls with the bundled Mozilla roots,
/// no redirect followed, an HTTP CONNECT proxy when the environment names
/// one, a connection pool that outlives the flush cadence, and bounded
/// timeouts so a stalled socket cannot hold the uploader thread past its
/// deadline.
pub(crate) struct UreqClient {
    agent: ureq::Agent,
}

impl UreqClient {
    /// Global request timeout: a stalled socket cannot hold the uploader
    /// thread past this.
    pub(crate) const GLOBAL_TIMEOUT: Duration = Duration::from_secs(15);
    /// Connect timeout.
    pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
    /// How long an idle connection is kept. Above the flush cadence, so a
    /// flush every 30 s reuses the TLS session rather than handshaking.
    pub(crate) const IDLE_AGE: Duration = Duration::from_secs(60);

    pub(crate) fn new() -> Self {
        Self::with_timeouts(Self::GLOBAL_TIMEOUT, Self::CONNECT_TIMEOUT)
    }

    /// Every setting the egress properties in `docs/telemetry.md` rest on is
    /// named here rather than inherited, because a ureq upgrade that moved a
    /// default would otherwise change how the SDK sends with no diff in this
    /// crate. Each one is pinned by a test below.
    pub(crate) fn with_timeouts(global: Duration, connect: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            // An ordinary HTTP CONNECT proxy when the process environment
            // names one (`ALL_PROXY`, `HTTPS_PROXY` or `HTTP_PROXY`, with
            // `NO_PROXY` exempting hosts), read here, which is when telemetry
            // is enabled. The tunnel carries a TLS session that terminates at
            // the ingest, so the proxy forwards the payload without reading
            // it.
            .proxy(ureq::Proxy::try_from_env())
            // No redirect is followed. Followed, a 301, 302 or 303 turned the
            // POST into a GET at the new location, a 2xx there was classified
            // as success, and the batch was deleted unsent and counted as
            // accepted. At zero every 3xx comes back as an answer, and
            // `classify_response` drops it.
            .max_redirects(0)
            // The Mozilla roots compiled into the SDK, never the device's
            // trust store, so an interception certificate installed only on
            // the device fails validation.
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::WebPki)
                    .disable_verification(false)
                    .build(),
            )
            .timeout_global(Some(global))
            .timeout_connect(Some(connect))
            .max_idle_age(Self::IDLE_AGE)
            .max_idle_connections_per_host(1)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl HttpClient for UreqClient {
    fn post(&mut self, request: &Request<'_>) -> Result<Response, String> {
        let mut builder = self.agent.post(request.url);
        for (name, value) in &request.headers {
            builder = builder.header(*name, *value);
        }
        let mut response = builder
            .send(request.body.as_bytes())
            .map_err(|err| err.to_string())?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = response
            .body_mut()
            .with_config()
            .limit(64 * 1024)
            .read_to_string()
            .unwrap_or_default();
        Ok(Response {
            status,
            retry_after,
            body,
        })
    }
}

/// Uploader cadence knobs the worker reads.
pub(crate) const MAX_BATCHES_PER_WAKE: u32 = 8;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::telemetry::pipe::store::tests::{batch, test_store};
    use crate::telemetry::pipe::store::Backend;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A client that answers from a script and records every request.
    #[derive(Default)]
    pub(crate) struct ScriptedClient {
        pub(crate) answers: VecDeque<Result<Response, String>>,
        pub(crate) requests: Arc<Mutex<Vec<(Vec<(String, String)>, String)>>>,
    }

    impl HttpClient for ScriptedClient {
        fn post(&mut self, request: &Request<'_>) -> Result<Response, String> {
            self.requests.lock().unwrap().push((
                request
                    .headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                request.body.to_string(),
            ));
            self.answers
                .pop_front()
                .unwrap_or_else(|| Err("no scripted answer".into()))
        }
    }

    pub(crate) fn answer(status: u16, body: &str) -> Result<Response, String> {
        Ok(Response {
            status,
            retry_after: None,
            body: body.into(),
        })
    }

    fn uploader(
        answers: Vec<Result<Response, String>>,
    ) -> (Uploader, Arc<Mutex<Vec<(Vec<(String, String)>, String)>>>) {
        let client = ScriptedClient {
            answers: answers.into(),
            requests: Arc::default(),
        };
        let requests = client.requests.clone();
        (
            Uploader::new(
                Box::new(client),
                "key",
                "app_1",
                user_agent("ios", 18),
                Box::new(|| 0),
            ),
            requests,
        )
    }

    #[test]
    fn every_request_carries_the_five_headers() {
        let (mut up, requests) = uploader(vec![answer(202, r#"{"accepted":1,"rejected":0}"#)]);
        let mut store = test_store(Backend::Memory);
        store.push(batch(1, 0));
        up.drain(&mut store, 0, 8, None);
        let (headers, _) = &requests.lock().unwrap()[0];
        let names: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            [
                "Authorization",
                HEADER_APP_ID,
                "Content-Type",
                HEADER_IDEMPOTENCY,
                "User-Agent"
            ]
        );
        assert_eq!(headers[0].1, "Bearer key");
        assert_eq!(headers[1].1, "app_1");
        assert!(headers[4].1.starts_with("offline-protocol-sdk/"));
        assert!(headers[4].1.ends_with("(ios/18)"));
    }

    #[test]
    fn a_2xx_deletes_and_counts_sent_and_accepted_separately() {
        let (mut up, _) = uploader(vec![
            answer(202, r#"{"accepted":2,"rejected":1,"rejected_reasons":[]}"#),
            answer(200, r#"{"deduplicated":true}"#),
            answer(204, ""),
        ]);
        let mut store = test_store(Backend::Memory);
        store.push(batch(3, 0));
        store.push(batch(4, 0));
        store.push(batch(5, 0));
        let report = up.drain(&mut store, 0, 8, None);
        assert!(store.is_empty());
        assert_eq!(report.sent_events, 12);
        // 2 from the 202 body, 0 for the replay, 5 for a bare 2xx.
        assert_eq!(report.accepted_events, 7);
    }

    #[test]
    fn a_4xx_drops_and_continues_and_401_403_halt() {
        for status in [400, 413, 422] {
            let (mut up, _) = uploader(vec![answer(status, ""), answer(202, r#"{"accepted":1}"#)]);
            let mut store = test_store(Backend::Memory);
            store.push(batch(1, 0));
            store.push(batch(1, 0));
            let report = up.drain(&mut store, 0, 8, None);
            assert!(store.is_empty(), "{status}");
            assert_eq!(report.dropped_events, 1, "{status}");
            assert_eq!(report.sent_events, 1, "{status}");
            assert!(up.next_retry_at_ms().is_none(), "{status} resets backoff");
        }
        for status in [401, 403] {
            let (mut up, requests) = uploader(vec![answer(status, ""), answer(202, "")]);
            let mut store = test_store(Backend::Memory);
            store.push(batch(1, 0));
            store.push(batch(1, 0));
            let report = up.drain(&mut store, 0, 8, None);
            assert!(report.halted);
            assert!(up.is_halted());
            assert_eq!(
                report.dropped_events, 0,
                "{status}: a rejected key loses nothing; the queue waits for a good one"
            );
            assert_eq!(store.len(), 2, "{status}: the refused batch is kept too");
            assert_eq!(
                requests.lock().unwrap().len(),
                1,
                "{status}: nothing more is sent"
            );
            assert_eq!(
                up.last_error(),
                Some(format!("ingest responded {status}").as_str())
            );
            let again = up.drain(&mut store, 1_000_000, 8, None);
            assert!(again.halted);
            assert_eq!(requests.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn retryable_answers_back_off_exponentially_with_jitter_to_a_15_minute_cap() {
        let client = ScriptedClient {
            answers: (0..12).map(|_| answer(500, "")).collect(),
            requests: Arc::default(),
        };
        let mut up = Uploader::new(
            Box::new(client),
            "key",
            "app",
            String::new(),
            Box::new(|| 7),
        );
        let mut store = test_store(Backend::Memory);
        store.push(batch(1, 0));
        let mut now = 0;
        let mut delays = Vec::new();
        for _ in 0..12 {
            up.drain(&mut store, now, 8, None);
            let at = up.next_retry_at_ms().expect("backoff armed");
            delays.push(at - now);
            now = at;
        }
        assert_eq!(delays[0], 1_007);
        assert_eq!(delays[1], 2_007);
        assert_eq!(delays[2], 4_007);
        assert_eq!(delays[9], 512_007);
        assert_eq!(delays[10], BACKOFF_MAX_MS + 7);
        assert_eq!(delays[11], BACKOFF_MAX_MS + 7);
        assert_eq!(store.len(), 1, "the batch is kept for the next try");
    }

    #[test]
    fn a_408_and_a_network_error_are_retryable_and_the_backoff_window_is_honoured() {
        let (mut up, requests) = uploader(vec![answer(408, ""), Err("connection refused".into())]);
        let mut store = test_store(Backend::Memory);
        store.push(batch(1, 0));
        up.drain(&mut store, 0, 8, None);
        assert_eq!(requests.lock().unwrap().len(), 1);
        up.drain(&mut store, 10, 8, None);
        assert_eq!(
            requests.lock().unwrap().len(),
            1,
            "inside the backoff window"
        );
        let at = up.next_retry_at_ms().expect("armed");
        up.drain(&mut store, at, 8, None);
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert_eq!(up.last_error(), Some("connection refused"));
    }

    #[test]
    fn a_429_honours_retry_after_in_seconds_and_as_a_date_and_backs_off_without_it() {
        let mut last = None;
        let seconds = Response {
            status: 429,
            retry_after: Some("120".into()),
            body: String::new(),
        };
        assert_eq!(
            classify_response(&seconds, 0, &mut last),
            Outcome::Retry {
                after_ms: Some(120_000)
            }
        );
        let bare = Response {
            status: 429,
            retry_after: None,
            body: String::new(),
        };
        assert_eq!(
            classify_response(&bare, 0, &mut last),
            Outcome::Retry { after_ms: None }
        );
        let now = chrono::Utc::now().timestamp_millis();
        let date = chrono::DateTime::from_timestamp_millis(now + 90_000)
            .expect("valid")
            .to_rfc2822();
        assert!(matches!(
            parse_retry_after(&date, now),
            Some(ms) if (89_000..=90_000).contains(&ms)
        ));
        assert_eq!(parse_retry_after("soon", now), None);
    }

    #[test]
    fn a_batch_past_the_ttl_is_dropped_unsent() {
        let (mut up, requests) = uploader(vec![answer(202, "")]);
        let mut store = test_store(Backend::Memory);
        store.push(batch(3, 0));
        store.push(batch(1, MAX_BATCH_AGE_MS));
        let report = up.drain(&mut store, MAX_BATCH_AGE_MS + 1, 8, None);
        assert_eq!(report.dropped_events, 3);
        assert_eq!(report.sent_events, 1);
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_drain_sends_at_most_max_batches_per_wake() {
        let (mut up, requests) = uploader((0..10).map(|_| answer(202, "")).collect());
        let mut store = test_store(Backend::Memory);
        for _ in 0..10 {
            store.push(batch(1, 0));
        }
        up.drain(&mut store, 0, 8, None);
        assert_eq!(requests.lock().unwrap().len(), 8);
        assert_eq!(store.len(), 2);
    }

    /// The real client against a local server: the headers and body arrive
    /// as sent, and the status and `Retry-After` come back as answered.
    #[test]
    fn the_ureq_client_posts_headers_and_body_and_reads_status_and_retry_after() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("local server");
        let addr = server.server_addr().to_ip().expect("tcp listener");
        let url: &'static str = Box::leak(format!("http://{addr}/v1/events").into_boxed_str());
        let served = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for n in 0..3 {
                let mut request = server.recv().expect("request");
                let mut body = String::new();
                request
                    .as_reader()
                    .read_to_string(&mut body)
                    .expect("body reads");
                let headers: Vec<(String, String)> = request
                    .headers()
                    .iter()
                    .map(|h| (h.field.as_str().to_string(), h.value.as_str().to_string()))
                    .collect();
                seen.push((headers, body));
                let response = match n {
                    0 => tiny_http::Response::from_string(r#"{"accepted":1,"rejected":0}"#)
                        .with_status_code(202),
                    1 => tiny_http::Response::from_string("")
                        .with_status_code(429)
                        .with_header(
                            tiny_http::Header::from_bytes(&b"Retry-After"[..], &b"7"[..])
                                .expect("header"),
                        ),
                    _ => tiny_http::Response::from_string("").with_status_code(500),
                };
                request.respond(response).expect("respond");
            }
            seen
        });

        let mut client = UreqClient::new();
        let post = |client: &mut UreqClient| {
            client.post(&Request {
                url,
                headers: vec![
                    ("Authorization", "Bearer mp_key"),
                    (HEADER_APP_ID, "app_1"),
                    ("Content-Type", "application/json"),
                    (HEADER_IDEMPOTENCY, "b-1"),
                    ("User-Agent", "offline-protocol-sdk/test (linux/0)"),
                ],
                body: r#"{"events":[]}"#,
            })
        };
        let first = post(&mut client).expect("answered");
        assert_eq!(first.status, 202);
        assert!(first.body.contains("accepted"));
        let second = post(&mut client).expect("answered");
        assert_eq!(second.status, 429);
        assert_eq!(second.retry_after.as_deref(), Some("7"));
        let third = post(&mut client).expect("answered");
        assert_eq!(third.status, 500);

        let seen = served.join().expect("server thread");
        let (headers, body) = &seen[0];
        assert_eq!(body, r#"{"events":[]}"#);
        let get = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("authorization"), Some("Bearer mp_key"));
        assert_eq!(get("x-mesh-analytics-app-id"), Some("app_1"));
        assert_eq!(get("content-type"), Some("application/json"));
        assert_eq!(get("idempotency-key"), Some("b-1"));
        assert_eq!(
            get("user-agent"),
            Some("offline-protocol-sdk/test (linux/0)")
        );
    }

    /// A server that accepts the connection and never answers is a transport
    /// error at the global timeout, not a hang.
    #[test]
    fn the_ureq_client_gives_up_on_a_server_that_never_answers() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("local server");
        let addr = server.server_addr().to_ip().expect("tcp listener");
        let url: &'static str = Box::leak(format!("http://{addr}/v1/events").into_boxed_str());
        let hold = std::thread::spawn(move || {
            let request = server.recv().expect("request");
            std::thread::sleep(Duration::from_millis(1_500));
            drop(request);
        });
        let mut client =
            UreqClient::with_timeouts(Duration::from_millis(400), Duration::from_millis(400));
        let started = Instant::now();
        let result = client.post(&Request {
            url,
            headers: Vec::new(),
            body: "{}",
        });
        assert!(result.is_err(), "{result:?}");
        assert!(
            started.elapsed() < Duration::from_millis(1_400),
            "{:?}",
            started.elapsed()
        );
        hold.join().expect("server thread");
    }

    /// Refused outright: a connection error, not a status.
    #[test]
    fn the_ureq_client_reports_a_refused_connection_as_a_transport_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        let url: &'static str = Box::leak(format!("http://{addr}/v1/events").into_boxed_str());
        let mut client = UreqClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(2));
        let result = client.post(&Request {
            url,
            headers: Vec::new(),
            body: "{}",
        });
        assert!(result.is_err());
    }

    #[test]
    fn a_3xx_drops_the_batch_and_continues() {
        for status in [300, 301, 302, 303, 307, 308] {
            let (mut up, _) = uploader(vec![answer(status, ""), answer(202, r#"{"accepted":1}"#)]);
            let mut store = test_store(Backend::Memory);
            store.push(batch(1, 0));
            store.push(batch(1, 0));
            let report = up.drain(&mut store, 0, 8, None);
            assert!(store.is_empty(), "{status}");
            assert_eq!(report.dropped_events, 1, "{status}");
            assert_eq!(report.sent_events, 1, "{status}");
            assert_eq!(
                report.accepted_events, 1,
                "{status}: a redirect is never counted as accepted"
            );
            assert!(!report.halted, "{status}");
        }
    }

    /// A redirect comes back as an answer rather than being followed, so it
    /// reaches the status matrix instead of a location the build never named.
    #[test]
    fn the_ureq_client_returns_a_redirect_rather_than_following_it() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("local server");
        let addr = server.server_addr().to_ip().expect("tcp listener");
        let url: &'static str = Box::leak(format!("http://{addr}/v1/events").into_boxed_str());
        let location = format!("http://{addr}/moved");
        let served = std::thread::spawn(move || {
            let mut paths = Vec::new();
            for status in [301, 308] {
                let request = server.recv().expect("request");
                paths.push(request.url().to_string());
                let response = tiny_http::Response::from_string("")
                    .with_status_code(status)
                    .with_header(
                        tiny_http::Header::from_bytes(&b"Location"[..], location.as_bytes())
                            .expect("header"),
                    );
                request.respond(response).expect("respond");
            }
            // A followed redirect would arrive here as one more request.
            if let Some(request) = server
                .recv_timeout(Duration::from_millis(500))
                .expect("server")
            {
                paths.push(request.url().to_string());
            }
            paths
        });

        let mut client = UreqClient::new();
        for expected in [301, 308] {
            let response = client
                .post(&Request {
                    url,
                    headers: vec![("Content-Type", "application/json")],
                    body: r#"{"events":[]}"#,
                })
                .expect("answered");
            assert_eq!(response.status, expected);
        }
        assert_eq!(
            served.join().expect("server thread"),
            ["/v1/events", "/v1/events"],
            "nothing followed the Location header"
        );
    }

    /// Set only on the child process the proxy test spawns, so the child
    /// half is a no-op in an ordinary run and never posts anywhere.
    const PROXY_PROBE_ENV: &str = "OFFLINE_TELEMETRY_PROXY_PROBE";

    /// The production constructor tunnels through the CONNECT proxy the
    /// environment names, and TLS runs through that tunnel to the
    /// destination rather than terminating at the proxy.
    ///
    /// The proxy variable is set on a child process that runs only
    /// `proxy_probe_child`, never on this one: the environment is
    /// process-wide, and every other `UreqClient` test in the binary would
    /// route through the stand-in while it was set.
    #[test]
    fn the_ureq_client_tunnels_through_the_connect_proxy_the_environment_names() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let proxy = format!("http://{}", listener.local_addr().expect("addr"));
        let (_, module) = module_path!()
            .split_once("::")
            .expect("crate-qualified path");
        let mut command = std::process::Command::new(std::env::current_exe().expect("test binary"));
        command.args(["--exact", &format!("{module}::proxy_probe_child")]);
        // Cleared before the one that is set: ureq takes the first of these
        // it can parse, a developer's shell may export any of them, and on
        // Windows the names are case-insensitive, so clearing `https_proxy`
        // after setting `HTTPS_PROXY` would clear the stand-in.
        for name in [
            "ALL_PROXY",
            "all_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "NO_PROXY",
            "no_proxy",
        ] {
            command.env_remove(name);
        }
        let mut child = command
            .env("HTTPS_PROXY", &proxy)
            .env(PROXY_PROBE_ENV, "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("child test process");

        let deadline = Instant::now() + Duration::from_secs(30);
        let mut tunnel = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Some(status) = child.try_wait().expect("child status") {
                        panic!("the client never reached the proxy (child exited: {status})");
                    }
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        panic!("the client never reached the proxy");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("accept: {err}"),
            }
        };
        tunnel.set_nonblocking(false).expect("blocking tunnel");
        tunnel
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");

        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            tunnel.read_exact(&mut byte).expect("CONNECT request");
            head.push(byte[0]);
        }
        let head = String::from_utf8(head).expect("ASCII request head");
        assert!(
            head.starts_with("CONNECT ingest.invalid:443 HTTP/1.1\r\n"),
            "{head:?}"
        );
        tunnel
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .expect("open the tunnel");

        // What follows is a TLS handshake naming the destination, not an
        // HTTP request: the proxy forwards the session and reads none of it.
        let mut record = [0u8; 5];
        tunnel.read_exact(&mut record).expect("TLS record header");
        assert_eq!(record[0], 0x16, "a TLS handshake record, not plaintext");
        let mut hello = vec![0u8; usize::from(u16::from_be_bytes([record[3], record[4]]))];
        tunnel.read_exact(&mut hello).expect("ClientHello");
        let sni = b"ingest.invalid";
        assert!(
            hello.windows(sni.len()).any(|w| w == sni),
            "the ClientHello names the destination"
        );

        drop(tunnel);
        let status = child.wait().expect("child exits");
        assert!(status.success(), "child test failed: {status}");
    }

    /// The child half of the proxy test: one post through the production
    /// constructor, to a host that cannot resolve, so a client that skipped
    /// the proxy fails at DNS instead of reaching anything real.
    #[test]
    fn proxy_probe_child() {
        if std::env::var_os(PROXY_PROBE_ENV).is_none() {
            return;
        }
        let _ = UreqClient::new().post(&Request {
            url: "https://ingest.invalid/v1/events",
            headers: Vec::new(),
            body: "{}",
        });
    }

    /// The trust anchor has no cheap behaviour test (it would need a
    /// certificate authority of its own), so the setting itself is pinned.
    #[test]
    fn the_agent_validates_against_the_bundled_roots_only() {
        let client = UreqClient::new();
        let tls = client.agent.config().tls_config();
        assert!(matches!(tls.root_certs(), ureq::tls::RootCerts::WebPki));
        assert!(!tls.disable_verification());
    }

    #[test]
    fn battery_deferral_applies_below_fifteen_percent_unless_charging() {
        assert!(battery_defers(Some(14), false));
        assert!(!battery_defers(Some(14), true));
        assert!(!battery_defers(Some(15), false));
        assert!(!battery_defers(None, false));
    }
}
