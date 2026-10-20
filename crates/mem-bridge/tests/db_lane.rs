//! W5.1 step 9 — `SerialDbLane` oracles (C2, settled point 1).
//!
//! The lane is a single ordered drain: two queues (agent, web), agent
//! drained first; writes never overlap (one drain at a time); LLM
//! summarisation work is REJECTED at the door (it never rides the lane —
//! D-33 agent-first is preserved at negligible complexity, and the
//! summariser never blocks the DB writer).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mem_bridge::db_lane::{DbLane, RejectionReason, SerialDbLane, WorkKind};

/// Records `enter:*`/`exit:*` events and flags overlapping execution.
#[derive(Clone)]
struct Recorder {
    events: Arc<Mutex<Vec<String>>>,
    live: Arc<AtomicUsize>,
}

impl Recorder {
    fn new() -> (Self, Arc<Mutex<Vec<String>>>) {
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                events: events.clone(),
                live: Arc::new(AtomicUsize::new(0)),
            },
            events,
        )
    }

    fn violations(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("events lock")
            .iter()
            .filter(|event| event.starts_with("VIOLATION"))
            .cloned()
            .collect()
    }

    /// A boxed doing-the-DB-work future labelled ~ the operation name.
    fn work(&self, label: &str, delay_ms: u64) -> mem_bridge::db_lane::Job {
        let events = Arc::clone(&self.events);
        let live = Arc::clone(&self.live);
        let label = label.to_owned();
        Box::pin(async move {
            let live_now = live.fetch_add(1, Ordering::SeqCst);
            if live_now > 0 {
                events
                    .lock()
                    .expect("events lock")
                    .push("VIOLATION: concurrent DB write".to_owned());
            }
            events
                .lock()
                .expect("events lock")
                .push(format!("enter:{label}"));
            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            events
                .lock()
                .expect("events lock")
                .push(format!("exit:{label}"));
            live.fetch_sub(1, Ordering::SeqCst);
        })
    }
}

#[tokio::test]
async fn test_db_lane_serializes_writes() {
    let (recorder, events) = Recorder::new();
    let lane = SerialDbLane::new();
    for label in ["write-0", "write-1", "write-2"] {
        lane.submit(WorkKind::Agent, recorder.work(label, 10))
            .await
            .expect("agent work is accepted");
    }
    lane.drain().await;
    let events = events.lock().expect("events lock").clone();
    assert_eq!(
        events,
        vec![
            "enter:write-0".to_owned(),
            "exit:write-0".to_owned(),
            "enter:write-1".to_owned(),
            "exit:write-1".to_owned(),
            "enter:write-2".to_owned(),
            "exit:write-2".to_owned(),
        ],
        "every write enters AND exits before the next begins"
    );
    assert!(
        recorder.violations().is_empty(),
        "no concurrent DB write: {:?}",
        events
    );
}

#[tokio::test]
async fn test_db_lane_agent_queue_drained_before_web() {
    let (recorder, events) = Recorder::new();
    let lane = SerialDbLane::new();
    // A LONG agent job holds the drain (well, arrives first); then a web
    // job and a second agent job arrive. After the in-flight agent
    // finishes, the agent queue is drained BEFORE the web queue (D-33
    // agent-first).
    lane.submit(WorkKind::Agent, recorder.work("agent-long", 40))
        .await
        .expect("agent work accepted");
    lane.submit(WorkKind::Web, recorder.work("web-question", 0))
        .await
        .expect("web work accepted");
    lane.submit(WorkKind::Agent, recorder.work("agent-fast", 0))
        .await
        .expect("agent work accepted");
    let drained = lane.drain().await;
    assert_eq!(drained.agent, 2, "two agent jobs drained");
    assert_eq!(drained.web, 1, "one web job drained");
    let events = events.lock().expect("events lock").clone();
    let positions: Vec<(String, usize)> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            if event.starts_with("enter:") {
                Some((event.trim_start_matches("enter:").to_owned(), index))
            } else {
                None
            }
        })
        .collect();
    let agent_fast = positions.iter().find(|(label, _)| label == "agent-fast");
    let web = positions.iter().find(|(label, _)| label == "web-question");
    assert!(
        agent_fast.zip(web).is_some_and(|(a, w)| a.1 < w.1),
        "agent-queue drained before web: {events:?}"
    );
    assert!(
        recorder.violations().is_empty(),
        "the serial property still holds with both queues loaded"
    );
}

#[tokio::test]
async fn test_db_lane_rejects_llm_summarisation() {
    let (recorder, _events) = Recorder::new();
    let lane = SerialDbLane::new();
    let rejection = lane
        .submit(WorkKind::LlmSummarisation, recorder.work("llm-summary", 0))
        .await
        .expect_err("LLM summarisation is rejected outright");
    assert_eq!(
        rejection.reason,
        RejectionReason::LlmSummarisationOffLane,
        "the summariser never rides the DB lane (D-33)"
    );
    let (agent, web) = lane.queue_lengths().await;
    assert_eq!((agent, web), (0, 0), "a rejected job is never queued");
    let drained = lane.drain().await;
    assert_eq!(
        (drained.agent, drained.web),
        (0, 0),
        "nothing was drained either"
    );
}
