//! `pa-core::jev` tests: chain semantics (cooldown, native-only, abort,
//! no-key), the answer cache behind the native judge, the session gates,
//! settings resolution, and the tool's sugar actions.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pa_agent::abort::AbortController;
use pa_jev::{
    Answer, JevClient, JevClientConfig, JudgmentRequest, JudgmentResult, Question, Usage,
    WireDialect,
};

use crate::jev::cache::AnswerCache;
use crate::jev::chain::{ChainJudge, JudgeCandidate, JudgeKind, CANDIDATE_REJECTION_COOLDOWN};
use crate::jev::native::NativeJudge;
use crate::jev::tool::JevTool;
use crate::jev::{should_install_jev_tool, JevResolvedSettings, Judge, JudgeError};
use crate::settings::load::from_value_lenient;
use crate::settings::{JevEnabledSetting, Settings, SettingsManager};

// ────────────────────────────── chain ──────────────────────────────

/// What one mock candidate answers.
#[derive(Clone)]
enum MockOutcome {
    Ok,
    Api(u16),
    Abort,
    Timeout,
    Fail,
}

/// A scriptable judge candidate: counts calls, records requests.
struct MockJudge {
    identity: &'static str,
    kind: JudgeKind,
    calls: AtomicUsize,
    requests: Mutex<Vec<JudgmentRequest>>,
    outcome: Mutex<MockOutcome>,
}

impl MockJudge {
    fn new(identity: &'static str, kind: JudgeKind, outcome: MockOutcome) -> Arc<Self> {
        Arc::new(MockJudge {
            identity,
            kind,
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            outcome: Mutex::new(outcome),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn set_outcome(&self, outcome: MockOutcome) {
        *self.outcome.lock().expect("outcome") = outcome;
    }
}

#[async_trait::async_trait]
impl Judge for MockJudge {
    fn label(&self) -> &'static str {
        self.identity
    }

    async fn judge(&self, request: &JudgmentRequest) -> Result<JudgmentResult, JudgeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .expect("requests")
            .push(request.clone());
        match &*self.outcome.lock().expect("outcome") {
            MockOutcome::Ok => Ok(judgment_result(self.identity)),
            MockOutcome::Api(status) => Err(JudgeError::Candidate {
                identity: self.identity.to_string(),
                message: format!("scripted status {status}"),
                status: Some(*status),
            }),
            MockOutcome::Abort => Err(JudgeError::Aborted),
            MockOutcome::Timeout => Err(JudgeError::Timeout { attempts: 3 }),
            MockOutcome::Fail => Err(JudgeError::Candidate {
                identity: self.identity.to_string(),
                message: "scripted failure".to_string(),
                status: None,
            }),
        }
    }
}

/// A flat success result for `identity`.
fn judgment_result(identity: &str) -> JudgmentResult {
    JudgmentResult {
        api: "typesafe".to_string(),
        provider: identity.split('/').next().unwrap_or(identity).to_string(),
        model: identity.to_string(),
        answers: BTreeMap::from([("q0".to_string(), Answer::Noul { noul: 0.9 })]),
        usage: Usage::default(),
    }
}

/// One judgment request over a fixed state.
fn one_question() -> JudgmentRequest {
    JudgmentRequest {
        state: pa_jev::JudgmentState::from("state"),
        questions: BTreeMap::from([(
            "q0".to_string(),
            Question::Noul {
                instructions: "is this fine".to_string(),
                criteria: None,
            },
        )]),
    }
}

/// A candidate over a mock judge.
fn candidate(judge: Arc<MockJudge>) -> JudgeCandidate {
    let kind = judge.kind;
    let identity = judge.identity.to_string();
    JudgeCandidate {
        identity,
        kind,
        judge: Some(judge),
    }
}

#[tokio::test]
async fn account_rejection_cools_down_and_moves_to_the_next_candidate() {
    let first = MockJudge::new("typesafe/a", JudgeKind::Native, MockOutcome::Api(401));
    let second = MockJudge::new("typesafe/b", JudgeKind::Native, MockOutcome::Ok);
    let chain = ChainJudge::new(vec![
        candidate(Arc::clone(&first)),
        candidate(Arc::clone(&second)),
    ]);
    // First judgment: a rejects (401), b answers.
    chain.judge(&one_question()).await.expect("b answers");
    assert_eq!(first.calls(), 1);
    assert_eq!(second.calls(), 1);
    // Second judgment: a is cooling down - b answers directly even though a
    // would now succeed.
    first.set_outcome(MockOutcome::Ok);
    chain.judge(&one_question()).await.expect("b answers again");
    assert_eq!(first.calls(), 1, "the rejected candidate stays skipped");
    assert_eq!(second.calls(), 2);
}

#[tokio::test]
async fn a_cooldown_expires_after_five_minutes() {
    let judge = MockJudge::new("typesafe/a", JudgeKind::Native, MockOutcome::Ok);
    let rejections: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
    rejections.lock().expect("ledger").insert(
        "typesafe/a".to_string(),
        Instant::now()
            .checked_sub(CANDIDATE_REJECTION_COOLDOWN + Duration::from_secs(1))
            .expect("now is after the cooldown"),
    );
    let chain = ChainJudge::with_rejections(vec![candidate(Arc::clone(&judge))], rejections);
    chain
        .judge(&one_question())
        .await
        .expect("expired entry clears");
    assert_eq!(judge.calls(), 1);
}

#[tokio::test]
async fn prompted_candidates_never_run_after_a_native_candidate() {
    let native = MockJudge::new("typesafe/a", JudgeKind::Native, MockOutcome::Fail);
    let prompted = MockJudge::new("openai/b", JudgeKind::Prompted, MockOutcome::Ok);
    let chain = ChainJudge::new(vec![
        candidate(Arc::clone(&native)),
        candidate(Arc::clone(&prompted)),
    ]);
    let error = chain
        .judge(&one_question())
        .await
        .expect_err("native failed");
    assert!(matches!(error, JudgeError::AllCandidatesFailed { .. }));
    assert_eq!(prompted.calls(), 0, "native-only: no prompted stand-in");
    // The reverse order is fine: a prompted candidate before the first
    // native one still serves.
    let prompted_ok = MockJudge::new("openai/c", JudgeKind::Prompted, MockOutcome::Ok);
    let native_fail = MockJudge::new("typesafe/d", JudgeKind::Native, MockOutcome::Fail);
    let chain = ChainJudge::new(vec![
        candidate(Arc::clone(&prompted_ok)),
        candidate(Arc::clone(&native_fail)),
    ]);
    chain
        .judge(&one_question())
        .await
        .expect("prompted first serves");
    assert_eq!(native_fail.calls(), 0);
}

#[tokio::test]
async fn aborts_and_timeouts_rethrow_without_fallback() {
    for outcome in [MockOutcome::Abort, MockOutcome::Timeout] {
        let first = MockJudge::new("typesafe/a", JudgeKind::Native, outcome.clone());
        let second = MockJudge::new("typesafe/b", JudgeKind::Native, MockOutcome::Ok);
        let chain = ChainJudge::new(vec![
            candidate(Arc::clone(&first)),
            candidate(Arc::clone(&second)),
        ]);
        let error = chain.judge(&one_question()).await.expect_err("rethrows");
        let expected = match outcome {
            MockOutcome::Abort => JudgeError::Aborted,
            MockOutcome::Timeout => JudgeError::Timeout { attempts: 3 },
            _ => unreachable!("only abort and timeout run here"),
        };
        assert_eq!(error, expected);
        assert_eq!(second.calls(), 0, "a cancellation never spends a candidate");
    }
}

#[tokio::test]
async fn keyless_candidates_are_skipped_and_reported() {
    let keyless = JudgeCandidate {
        identity: "typesafe/no-key".to_string(),
        kind: JudgeKind::Native,
        judge: None,
    };
    let working = MockJudge::new("typesafe/b", JudgeKind::Native, MockOutcome::Ok);
    let chain = ChainJudge::new(vec![keyless, candidate(Arc::clone(&working))]);
    chain.judge(&one_question()).await.expect("b answers");
    // A chain of only keyless candidates is a typed failure naming the gap.
    let chain = ChainJudge::new(vec![JudgeCandidate {
        identity: "typesafe/no-key".to_string(),
        kind: JudgeKind::Native,
        judge: None,
    }]);
    let error = chain
        .judge(&one_question())
        .await
        .expect_err("nothing serves");
    match error {
        JudgeError::AllCandidatesFailed {
            last_unavailable: Some(unavailable),
            ..
        } => assert!(unavailable.contains("no API key"), "{unavailable}"),
        other => panic!("expected AllCandidatesFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn every_candidate_failing_is_a_typed_error_naming_the_last_failure() {
    let first = MockJudge::new("typesafe/a", JudgeKind::Native, MockOutcome::Fail);
    let second = MockJudge::new("typesafe/b", JudgeKind::Native, MockOutcome::Api(500));
    let chain = ChainJudge::new(vec![
        candidate(Arc::clone(&first)),
        candidate(Arc::clone(&second)),
    ]);
    let error = chain.judge(&one_question()).await.expect_err("all failed");
    match error {
        JudgeError::AllCandidatesFailed {
            last_failure: Some(failure),
            ..
        } => assert!(failure.contains("status 500"), "{failure}"),
        other => panic!("expected AllCandidatesFailed, got {other:?}"),
    }
}

// ─────────────────────── answer cache + native judge ───────────────────────

/// The loopback scripted server (the pa-ai mock pattern, trimmed).
struct MockServer {
    port: u16,
    queue: Arc<Mutex<VecDeque<(u16, String)>>>,
    bodies: Arc<Mutex<Vec<String>>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MockServer {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let (queue, bodies, shutdown) = (
                Arc::clone(&queue),
                Arc::clone(&bodies),
                Arc::clone(&shutdown),
            );
            move || {
                for stream in listener.incoming() {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Ok(stream) = stream {
                        let _ = serve(stream, &queue, &bodies);
                    }
                }
            }
        });
        MockServer {
            port,
            queue,
            bodies,
            shutdown,
            thread: Some(thread),
        }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Queue a success answering exactly `ids`, billing `input_tokens`.
    fn push_judgment(&self, ids: &[&str], input_tokens: u64) {
        let answers: BTreeMap<String, serde_json::Value> = ids
            .iter()
            .map(|id| {
                (
                    (*id).to_string(),
                    serde_json::json!({"type": "noul", "noul": 0.5}),
                )
            })
            .collect();
        self.queue.lock().expect("queue").push_back((
            200,
            serde_json::json!({
                "model": "jev-1.13.0",
                "answers": answers,
                "usage": {"input_tokens": input_tokens, "output_tokens": 0}
            })
            .to_string(),
        ));
    }

    fn request_count(&self) -> usize {
        self.bodies.lock().expect("bodies").len()
    }

    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().expect("bodies").clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(
    mut stream: TcpStream,
    queue: &Mutex<VecDeque<(u16, String)>>,
    bodies: &Mutex<Vec<String>>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let content_length: usize = head
        .split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    bodies
        .lock()
        .expect("bodies")
        .push(String::from_utf8_lossy(&body).into_owned());
    let (status, body) = queue
        .lock()
        .expect("queue")
        .pop_front()
        .unwrap_or((500, r#"{"error":"queue exhausted"}"#.to_string()));
    let reason = if status == 200 { "OK" } else { "Error" };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// A cached native judge over the mock server.
fn native_judge(server: &MockServer, cache: Option<Arc<AnswerCache>>) -> NativeJudge {
    NativeJudge::new(
        JevClient::new(JevClientConfig::new(
            WireDialect::Typesafe,
            server.base_url(),
            "jev-1.13.0",
            "typesafe",
            "test-key",
        )),
        "typesafe/jev-1.13.0".to_string(),
        cache,
    )
}

/// A noul-question request over the given ids.
fn ask(ids: &[&str]) -> JudgmentRequest {
    JudgmentRequest {
        state: pa_jev::JudgmentState::from("the same state"),
        questions: ids
            .iter()
            .map(|id| {
                (
                    (*id).to_string(),
                    Question::Noul {
                        instructions: format!("question {id}"),
                        criteria: None,
                    },
                )
            })
            .collect(),
    }
}

#[tokio::test]
async fn a_repeated_question_is_answered_locally_with_zero_out() {
    let server = MockServer::new();
    server.push_judgment(&["q0"], 100);
    let judge = native_judge(&server, Some(Arc::new(AnswerCache::default())));
    let first = judge.judge(&ask(&["q0"])).await.expect("first answers");
    assert_eq!(first.usage.input_tokens, 100);
    let second = judge.judge(&ask(&["q0"])).await.expect("cache answers");
    assert_eq!(
        server.request_count(),
        1,
        "the repeat never reaches the wire"
    );
    assert_eq!(
        second.usage.input_tokens, 0,
        "a full cache hit bills nothing"
    );
    assert_eq!(second.answers.len(), 1);
}

#[tokio::test]
async fn a_partially_cached_request_sends_only_the_pending_questions() {
    let server = MockServer::new();
    server.push_judgment(&["q0"], 50);
    server.push_judgment(&["q1"], 70);
    let judge = native_judge(&server, Some(Arc::new(AnswerCache::default())));
    judge.judge(&ask(&["q0"])).await.expect("first answers");
    let merged = judge
        .judge(&ask(&["q0", "q1"]))
        .await
        .expect("second answers");
    assert_eq!(server.request_count(), 2);
    // The second request carried only the pending question.
    let second_body: serde_json::Value =
        serde_json::from_str(&server.bodies()[1]).expect("json body");
    let questions = second_body["questions"].as_object().expect("questions");
    assert_eq!(questions.len(), 1);
    assert!(questions.contains_key("q1"));
    // The merged answer set covers both ids; usage covers the fresh part.
    assert_eq!(merged.answers.len(), 2);
    assert_eq!(merged.usage.input_tokens, 70);
}

#[tokio::test]
async fn the_cache_keys_on_state_model_and_question_identity() {
    let server = MockServer::new();
    server.push_judgment(&["q0"], 10);
    server.push_judgment(&["q0"], 10);
    let judge = native_judge(&server, Some(Arc::new(AnswerCache::default())));
    judge
        .judge(&ask(&["q0"]))
        .await
        .expect("first state answers");
    // A different state: same question id, fresh judgment.
    let mut other_state = ask(&["q0"]);
    other_state.state = pa_jev::JudgmentState::from("a different state");
    judge
        .judge(&other_state)
        .await
        .expect("different state answers");
    assert_eq!(server.request_count(), 2);
}

#[tokio::test]
async fn an_uncached_judge_hits_the_wire_every_time() {
    let server = MockServer::new();
    server.push_judgment(&["q0"], 10);
    server.push_judgment(&["q0"], 10);
    let judge = native_judge(&server, None);
    judge.judge(&ask(&["q0"])).await.expect("answers");
    judge.judge(&ask(&["q0"])).await.expect("answers again");
    assert_eq!(server.request_count(), 2);
}

// ────────────────────────────── gates ──────────────────────────────

fn native_chain() -> Arc<ChainJudge> {
    let judge = MockJudge::new("typesafe/a", JudgeKind::Native, MockOutcome::Ok);
    Arc::new(ChainJudge::new(vec![candidate(judge)]))
}

fn prompted_chain() -> Arc<ChainJudge> {
    let judge = MockJudge::new("openai/a", JudgeKind::Prompted, MockOutcome::Ok);
    Arc::new(ChainJudge::new(vec![candidate(judge)]))
}

#[test]
fn the_tool_gate_off_by_default_and_auto_requires_native() {
    use crate::jev::DEFAULT_JEV_MODEL;
    let _ = DEFAULT_JEV_MODEL;
    // Off: never, even with a chain (the default - RED test for the gate).
    assert!(!should_install_jev_tool(
        JevEnabledSetting::Off,
        Some(&native_chain())
    ));
    // On: whenever a chain resolved.
    assert!(should_install_jev_tool(
        JevEnabledSetting::On,
        Some(&native_chain())
    ));
    assert!(!should_install_jev_tool(JevEnabledSetting::On, None));
    // Auto: only when the chain starts native - a prompted-only chain keeps
    // the consumer on its deterministic path.
    assert!(should_install_jev_tool(
        JevEnabledSetting::Auto,
        Some(&native_chain())
    ));
    assert!(!should_install_jev_tool(
        JevEnabledSetting::Auto,
        Some(&prompted_chain())
    ));
    assert!(!should_install_jev_tool(JevEnabledSetting::Auto, None));
}

// ───────────────────────────── settings ─────────────────────────────

fn manager_with(settings: &Settings) -> SettingsManager {
    SettingsManager::in_memory(settings)
}

#[test]
fn jev_settings_default_off() {
    // The layer must stay OFF unless the user opts in (a paid endpoint).
    assert_eq!(
        manager_with(&Settings::default()).get_jev_enabled(),
        JevEnabledSetting::Off
    );
}

#[test]
fn jev_settings_parse_every_gate_and_default_the_rest() {
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "jev": {
            "enabled": "auto",
            "model": "openrouter/typesafe/jev-1.13",
            "fallbackChain": ["typesafe/jev-1.13.0", "  ", "openrouter/cloudflare/clef"],
            "timeoutMs": 2500,
            "cache": {"enabled": false}
        }
    }))
    .unwrap();
    let manager = manager_with(&settings);
    assert_eq!(manager.get_jev_enabled(), JevEnabledSetting::Auto);
    assert_eq!(manager.get_jev_model(), "openrouter/typesafe/jev-1.13");
    assert_eq!(
        manager.get_jev_fallback_chain(),
        vec![
            "typesafe/jev-1.13.0".to_string(),
            "openrouter/cloudflare/clef".to_string()
        ]
    );
    assert_eq!(manager.get_jev_timeout_ms(), 2500);
    assert!(!manager.get_jev_cache_enabled());

    let minimal: Settings =
        serde_json::from_value(serde_json::json!({"jev": {"enabled": "on"}})).unwrap();
    let manager = manager_with(&minimal);
    assert_eq!(manager.get_jev_enabled(), JevEnabledSetting::On);
    assert_eq!(manager.get_jev_model(), crate::jev::DEFAULT_JEV_MODEL);
    assert_eq!(manager.get_jev_timeout_ms(), pa_jev::DEFAULT_TIMEOUT_MS);
    assert!(manager.get_jev_cache_enabled(), "the cache defaults on");

    let unknown: Settings =
        serde_json::from_value(serde_json::json!({"jev": {"enabled": "maybe"}})).unwrap();
    assert_eq!(
        manager_with(&unknown).get_jev_enabled(),
        JevEnabledSetting::Off,
        "an unrecognized gate reads as off"
    );
}

/// Phase 2: the consumer gates default OFF and only `auto` turns them on.
#[test]
fn the_phase_two_consumer_gates_default_off() {
    use crate::settings::{JevAutoThinkingSetting, JevGraphRerankSetting};
    let unset: Settings = serde_json::from_value(serde_json::json!({"jev": {}})).unwrap();
    let manager = manager_with(&unset);
    assert_eq!(manager.get_jev_auto_thinking(), JevAutoThinkingSetting::Off);
    assert_eq!(manager.get_jev_graph_rerank(), JevGraphRerankSetting::Off);

    let on: Settings = serde_json::from_value(serde_json::json!({
        "jev": {"enabled": "auto", "autoThinking": "auto", "graphRerank": "auto"}
    }))
    .unwrap();
    let manager = manager_with(&on);
    assert_eq!(
        manager.get_jev_auto_thinking(),
        JevAutoThinkingSetting::Auto
    );
    assert_eq!(manager.get_jev_graph_rerank(), JevGraphRerankSetting::Auto);

    let typo: Settings = serde_json::from_value(serde_json::json!({
        "jev": {"autoThinking": "yes", "graphRerank": "true"}
    }))
    .unwrap();
    let manager = manager_with(&typo);
    assert_eq!(
        manager.get_jev_auto_thinking(),
        JevAutoThinkingSetting::Off,
        "a typo never enables a consumer"
    );
    assert_eq!(manager.get_jev_graph_rerank(), JevGraphRerankSetting::Off);
}

#[test]
fn a_wrong_typed_jev_block_loads_as_unset() {
    let settings = from_value_lenient(&serde_json::json!({"jev": "yes"}));
    assert!(
        settings.jev.is_none(),
        "the known field drops out entirely: {:?}",
        settings.extra
    );
    assert!(settings.extra.get("jev").is_none());
}

// ───────────────────────────── resolve ─────────────────────────────

#[test]
fn resolve_builds_the_chain_from_settings_and_the_registry() {
    // No catalog entry for `typesafe/*`: the candidate synthesizes the
    // default endpoint and (with no credential anywhere) stays keyless -
    // resolved but skipped, exactly the no-key semantics.
    if std::env::var_os(crate::jev::TYPESAFE_API_KEY_ENV).is_some() {
        // The ambient env carries a real TypeSafe key (a developer box); the
        // keyless leg of this test cannot run beside it.
        return;
    }
    let temp = tempfile::tempdir().expect("tempdir");
    let auth = crate::auth::AuthStorage::create(temp.path());
    let mut registry = crate::models::ModelRegistry::in_memory(auth);
    let settings = JevResolvedSettings {
        enabled: JevEnabledSetting::On,
        model: "typesafe/jev-1.13.0".to_string(),
        fallback_chain: vec!["typesafe/jev-1.14.0".to_string()],
        timeout: Duration::from_millis(10_000),
        cache_enabled: true,
    };
    let chain = crate::jev::resolve_judge(&settings, &mut registry).expect("chain resolves");
    assert_eq!(
        chain.identities(),
        vec!["typesafe/jev-1.13.0", "typesafe/jev-1.14.0"]
    );
    // Nothing serves without keys: the typed failure names the gap.
    let error = futures::executor::block_on(chain.judge(&one_question()))
        .expect_err("no candidate has a key");
    match error {
        JudgeError::AllCandidatesFailed {
            last_unavailable: Some(unavailable),
            ..
        } => assert!(unavailable.contains("no API key"), "{unavailable}"),
        other => panic!("expected AllCandidatesFailed, got {other:?}"),
    }
}

#[test]
fn resolve_returns_none_without_any_judgment_candidate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let auth = crate::auth::AuthStorage::create(temp.path());
    let mut registry = crate::models::ModelRegistry::in_memory(auth);
    let settings = JevResolvedSettings {
        enabled: JevEnabledSetting::On,
        model: "openai/gpt-5".to_string(),
        fallback_chain: Vec::new(),
        timeout: Duration::from_millis(10_000),
        cache_enabled: true,
    };
    // A chat model is not a judgment model: no candidate, no chain.
    assert!(
        crate::jev::resolve_judge(&settings, &mut registry).is_none(),
        "unknown providers never synthesize an endpoint"
    );
}

// ─────────────────────────────── tool ───────────────────────────────

/// A judge that records its requests and noul-answers every asked id.
struct RecordingJudge {
    requests: Mutex<Vec<JudgmentRequest>>,
}

#[async_trait::async_trait]
impl Judge for RecordingJudge {
    fn label(&self) -> &'static str {
        "recording"
    }

    async fn judge(&self, request: &JudgmentRequest) -> Result<JudgmentResult, JudgeError> {
        self.requests
            .lock()
            .expect("requests")
            .push(request.clone());
        let answers = request
            .questions
            .keys()
            .map(|id| (id.clone(), Answer::Noul { noul: 0.9 }))
            .collect();
        Ok(JudgmentResult {
            api: "typesafe".to_string(),
            provider: "typesafe".to_string(),
            model: "typesafe/jev-1.13.0".to_string(),
            answers,
            usage: Usage::default(),
        })
    }
}

fn recording_tool() -> (Arc<JevTool>, Arc<RecordingJudge>) {
    let judge = Arc::new(RecordingJudge {
        requests: Mutex::new(Vec::new()),
    });
    let judge_dyn: Arc<dyn Judge> = judge.clone();
    (Arc::new(JevTool::new(judge_dyn)), judge)
}

/// Drive the tool through the real `AgentTool::execute` contract.
async fn call(
    tool: &Arc<JevTool>,
    params: serde_json::Value,
    signal: &pa_agent::abort::AbortSignal,
) -> anyhow::Result<pa_agent::types::AgentToolResult> {
    pa_agent::types::AgentTool::execute(
        Arc::clone(tool),
        "test-call".to_string(),
        params,
        signal.clone(),
        Arc::new(|_| {}),
    )
    .await
}

fn never_signal() -> pa_agent::abort::AbortSignal {
    AbortController::new().signal()
}

/// The first text block of a tool result, for assertions.
fn text_of(result: &pa_agent::types::AgentToolResult) -> &str {
    match &result.content[0] {
        pa_agent::types::ToolResultContent::Text(text) => &text.text,
        pa_agent::types::ToolResultContent::Image(other) => {
            panic!("expected a text block, got {other:?}")
        }
    }
}

#[tokio::test]
async fn classify_appends_the_escape_option_and_answers() {
    let (tool, judge) = recording_tool();
    let result = call(
        &tool,
        serde_json::json!({
            "action": "classify",
            "state": "the build broke on CI",
            "question": "what kind of break is this",
            "options": [
                {"name": "flaky", "description": "passes on retry"},
                {"name": "real", "description": "fails consistently"}
            ]
        }),
        &never_signal(),
    )
    .await
    .expect("classify answers");
    assert!(!result.is_error);
    let requests = judge.requests.lock().expect("requests");
    let question = &requests[0].questions["classify"];
    let Question::Choice { criteria, .. } = question else {
        panic!("classify builds a choice question, got {question:?}");
    };
    assert_eq!(criteria.len(), 3, "the escape option is appended");
    assert!(criteria.contains_key("other"));
    let text = text_of(&result);
    assert!(text.contains("classify: "), "{text}");
}

#[tokio::test]
async fn classify_keeps_an_explicit_escape_option() {
    let (tool, judge) = recording_tool();
    call(
        &tool,
        serde_json::json!({
            "action": "route",
            "state": "where does this ticket go",
            "question": "route the ticket",
            "routes": ["billing", "fraud", "none"]
        }),
        &never_signal(),
    )
    .await
    .expect("route answers");
    let requests = judge.requests.lock().expect("requests");
    let Question::Choice { criteria, .. } = &requests[0].questions["classify"] else {
        panic!("choice question");
    };
    assert_eq!(criteria.len(), 3, "no duplicate escape appended");
}

#[tokio::test]
async fn check_builds_a_noul_question_with_rubrics() {
    let (tool, judge) = recording_tool();
    call(
        &tool,
        serde_json::json!({
            "action": "check",
            "state": "the log line mentions a race",
            "question": "is this a data race",
            "true": "two writers hit the same field",
            "false": "a single-threaded ordering issue"
        }),
        &never_signal(),
    )
    .await
    .expect("check answers");
    let requests = judge.requests.lock().expect("requests");
    match &requests[0].questions["check"] {
        Question::Noul {
            instructions,
            criteria,
        } => {
            assert_eq!(instructions, "is this a data race");
            let criteria = criteria.as_ref().expect("rubrics present");
            assert_eq!(
                criteria.true_.as_deref(),
                Some("two writers hit the same field")
            );
            assert_eq!(
                criteria.false_.as_deref(),
                Some("a single-threaded ordering issue")
            );
        }
        other => panic!("check builds a noul question, got {other:?}"),
    }
}

#[tokio::test]
async fn ask_passes_explicit_questions_and_rejects_bad_shapes() {
    let (tool, judge) = recording_tool();
    call(
        &tool,
        serde_json::json!({
            "action": "ask",
            "state": "two things at once",
            "questions": [
                {"id": "tone", "type": "score", "instructions": "tone",
                 "levels": ["calm", "heated"]},
                {"id": "urgent", "type": "noul", "instructions": "urgent soon"}
            ]
        }),
        &never_signal(),
    )
    .await
    .expect("ask answers");
    // The guard is scoped: the second call re-enters the recording judge,
    // which locks the same mutex (holding it across the await deadlocks).
    {
        let requests = judge.requests.lock().expect("requests");
        assert_eq!(requests[0].questions.len(), 2);
        assert!(matches!(
            requests[0].questions["tone"],
            Question::Score { .. }
        ));
    }

    // A one-level score question violates the contract's 2..=10 bound.
    let error = call(
        &tool,
        serde_json::json!({
            "action": "ask",
            "state": "bad shape",
            "questions": [
                {"id": "s", "type": "score", "instructions": "x", "levels": ["only"]}
            ]
        }),
        &never_signal(),
    )
    .await
    .expect_err("shape bounds are enforced");
    assert!(error.to_string().contains("levels"), "{error}");
}

#[tokio::test]
async fn unknown_actions_and_missing_state_are_errors() {
    let (tool, _judge) = recording_tool();
    let error = call(
        &tool,
        serde_json::json!({"action": "guess", "state": "x"}),
        &never_signal(),
    )
    .await
    .expect_err("unknown action");
    assert!(error.to_string().contains("unknown jev action"), "{error}");
    let error = call(
        &tool,
        serde_json::json!({"action": "check"}),
        &never_signal(),
    )
    .await
    .expect_err("state is required");
    assert!(error.to_string().contains("state"), "{error}");
}

#[tokio::test]
async fn an_aborted_signal_short_circuits_the_judgment() {
    struct NeverJudge;
    #[async_trait::async_trait]
    impl Judge for NeverJudge {
        fn label(&self) -> &'static str {
            "never"
        }
        async fn judge(&self, _request: &JudgmentRequest) -> Result<JudgmentResult, JudgeError> {
            std::future::pending().await
        }
    }
    let controller = AbortController::new();
    controller.abort();
    let judge_dyn: Arc<dyn Judge> = Arc::new(NeverJudge);
    let tool = Arc::new(JevTool::new(judge_dyn));
    let error = call(
        &tool,
        serde_json::json!({"action": "check", "state": "x", "question": "q"}),
        &controller.signal(),
    )
    .await
    .expect_err("an aborted signal never reaches the judge");
    assert!(
        error.to_string().to_ascii_lowercase().contains("abort"),
        "{error}"
    );
}

// ─────────────────────── create_session wiring ───────────────────────

/// The main chat model a `create_session` build needs.
fn chat_model() -> pa_agent::types::Model {
    pa_agent::types::Model {
        id: "m".into(),
        name: "m".into(),
        api: "test".into(),
        provider: "test".into(),
        base_url: "http://localhost".into(),
        reasoning: false,
        cost: pa_agent::types::UsageCost::default(),
        context_window: 1_000,
        max_tokens: 100,
        max_tokens_explicit: false,
    }
}

/// Build a session over `agent_dir` with the given `[jev]` settings.
async fn session_tool_names(agent_dir: &std::path::Path) -> (Vec<String>, Vec<String>) {
    let cwd = agent_dir.join("project");
    std::fs::create_dir_all(&cwd).expect("project dir");
    let model = chat_model();
    let provider = Arc::new(pa_agent::scripted::ScriptedProvider::new(model.clone()));
    let engine = crate::session_engine::engine::create_session(
        crate::session_engine::engine::SessionEngineConfig {
            cwd,
            agent_dir: agent_dir.to_path_buf(),
            model: Some(model),
            stream_fn: Some(provider.stream_fn()),
            ..Default::default()
        },
    )
    .await
    .expect("session builds");
    let names = engine
        .session
        .agent()
        .state()
        .await
        .tools
        .iter()
        .map(|tool| tool.name().to_string())
        .collect();
    (names, engine.native_tool_names().to_vec())
}

/// Build a session over `agent_dir` and return it (the consumer wiring
/// tests read the installed seams).
async fn session_engine_over(
    agent_dir: &std::path::Path,
) -> crate::session_engine::engine::SessionEngine {
    let cwd = agent_dir.join("project");
    std::fs::create_dir_all(&cwd).expect("project dir");
    let model = chat_model();
    let provider = Arc::new(pa_agent::scripted::ScriptedProvider::new(model.clone()));
    crate::session_engine::engine::create_session(
        crate::session_engine::engine::SessionEngineConfig {
            cwd,
            agent_dir: agent_dir.to_path_buf(),
            model: Some(model),
            stream_fn: Some(provider.stream_fn()),
            ..Default::default()
        },
    )
    .await
    .expect("session builds")
}

/// The Phase-2 auto-thinking wiring follows the `[jev]` gates: absent by
/// default, present only with `enabled` + `autoThinking = auto` + a native
/// candidate.
#[tokio::test]
async fn create_session_installs_auto_thinking_only_behind_a_native_judge() {
    let default_dir = tempfile::tempdir().expect("tempdir");
    let engine = session_engine_over(default_dir.path()).await;
    assert!(
        engine.session.auto_thinking_wiring().is_none(),
        "the default session classifies nothing"
    );

    // `autoThinking = auto` alone is not enough: the layer is still off.
    let layer_off = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        layer_off.path().join("settings.json"),
        serde_json::json!({ "jev": { "autoThinking": "auto" } }).to_string(),
    )
    .expect("settings.json");
    let engine = session_engine_over(layer_off.path()).await;
    assert!(
        engine.session.auto_thinking_wiring().is_none(),
        "[jev] enabled stays off"
    );

    // Both gates on plus a native candidate: the wiring is installed.
    let wired = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        wired.path().join("settings.json"),
        serde_json::json!({
            "jev": { "enabled": "on", "autoThinking": "auto", "graphRerank": "auto" }
        })
        .to_string(),
    )
    .expect("settings.json");
    std::fs::write(
        wired.path().join("models.json"),
        r#"{ "providers": { "typesafe": {
            "baseUrl": "http://127.0.0.1:9", "apiKey": "test-key", "api": "typesafe",
            "models": [ { "id": "jev-1.13.0" } ]
        } } }"#,
    )
    .expect("models.json");
    let engine = session_engine_over(wired.path()).await;
    let wiring = engine
        .session
        .auto_thinking_wiring()
        .expect("the classifier is wired behind a native judge");
    assert_eq!(
        wiring.fallback(),
        pa_agent::types::ThinkingLevel::Off,
        "the fallback is the session's configured level"
    );
}

/// The `[jev] enabled` gate drives the tool roster: default OFF installs
/// nothing, `on` (with a resolvable native candidate) installs `jev` and
/// records it in `native_tool_names`.
#[tokio::test]
async fn create_session_installs_jev_tool_only_when_enabled() {
    // Default (no `[jev]` block): the layer stays off - no tool, no
    // registry build, no spend.
    let default_dir = tempfile::tempdir().expect("tempdir");
    let (names, native) = session_tool_names(default_dir.path()).await;
    assert!(
        !names.iter().any(|name| name == "jev"),
        "default OFF installs no jev tool: {names:?}"
    );
    assert!(
        !native.iter().any(|name| name == "jev"),
        "default OFF records no jev name: {native:?}"
    );

    // `enabled = "on"` + a resolvable native candidate: the tool joins the
    // roster and the recorded fork roster names it.
    let enabled_dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        enabled_dir.path().join("settings.json"),
        serde_json::json!({ "jev": { "enabled": "on" } }).to_string(),
    )
    .expect("settings.json");
    std::fs::write(
        enabled_dir.path().join("models.json"),
        r#"{ "providers": { "typesafe": {
            "baseUrl": "http://127.0.0.1:9", "apiKey": "test-key", "api": "typesafe",
            "models": [ { "id": "jev-1.13.0" } ]
        } } }"#,
    )
    .expect("models.json");
    let (names, native) = session_tool_names(enabled_dir.path()).await;
    assert!(
        names.iter().any(|name| name == "jev"),
        "enabled ON with a native candidate installs jev: {names:?}"
    );
    assert!(
        native.iter().any(|name| name == "jev"),
        "the recorded fork roster names jev: {native:?}"
    );
}
