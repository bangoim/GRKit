//! Bounded, in-memory contracts shared by v1 resource handlers.
//!
//! Callers serialize access to the request cache and change hub with the same
//! coordinator that publishes runtime-acknowledged configuration changes.

use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

/// An RFC 9457 response body. The HTTP response uses `application/problem+json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub title: &'static str,
    pub status: u16,
    pub code: &'static str,
    pub detail: String,
}

impl Problem {
    pub fn new(status: u16, code: &'static str, detail: impl Into<String>) -> Self {
        let title = crate::reason(status);
        Self {
            kind: "about:blank",
            title,
            status,
            code,
            detail: detail.into(),
        }
    }
}

/// Hash the exact bytes sent by GET, including every representation field.
pub fn strong_etag(representation: &[u8]) -> String {
    let hash: String = Sha256::digest(representation)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("\"sha256:{hash}\"")
}

/// Require an explicit version for replacement, or `If-None-Match: *` for creation.
/// Weak validators and `If-Match: *` cannot authorize overwriting an unknown version.
pub fn check_precondition(
    current: Option<&str>,
    if_match: Option<&str>,
    if_none_match: Option<&str>,
) -> Result<(), Problem> {
    if if_match.is_some() && if_none_match.is_some() {
        return Err(Problem::new(
            400,
            "invalid_precondition",
            "Use one write precondition.",
        ));
    }
    if let Some(condition) = if_none_match {
        if condition.trim() != "*" {
            return Err(Problem::new(
                400,
                "invalid_precondition",
                "Creation requires If-None-Match: *.",
            ));
        }
        return if current.is_none() {
            Ok(())
        } else {
            Err(Problem::new(
                412,
                "precondition_failed",
                "The resource already exists.",
            ))
        };
    }
    let Some(condition) = if_match else {
        return Err(Problem::new(
            428,
            "precondition_required",
            "Supply the resource write precondition.",
        ));
    };
    if condition.trim() == "*" {
        return Err(Problem::new(
            400,
            "invalid_precondition",
            "Replacement requires a specific strong ETag.",
        ));
    }
    if current.is_some_and(|tag| {
        condition
            .split(',')
            .any(|candidate| candidate.trim() == tag && !tag.starts_with("W/"))
    }) {
        Ok(())
    } else {
        Err(Problem::new(
            412,
            "precondition_failed",
            "The resource version changed or no longer exists.",
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Intent {
    pub domain: String,
    pub device_id: String,
    pub boot_id: String,
    pub card_id: String,
    pub method: String,
    /// Includes the query string, since it can change mutation semantics.
    pub uri: String,
    pub if_match: Option<String>,
    pub if_none_match: Option<String>,
    pub content_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecordedResult {
    pub status: u16,
    pub body: Value,
}

struct Record {
    intent: Intent,
    result: RecordedResult,
    completed: Option<Instant>,
}

#[derive(Debug, PartialEq)]
pub enum Admission {
    New,
    Existing(RecordedResult),
}

/// Active requests are never evicted. Terminal records survive at least retention.
/// No record is persisted; a new cache belongs to a new service boot identity.
pub struct RequestCache {
    boot_id: String,
    capacity: usize,
    max_record_bytes: usize,
    retention: Duration,
    records: HashMap<String, Record>,
}

impl RequestCache {
    pub fn new(
        boot_id: String,
        capacity: usize,
        max_record_bytes: usize,
        retention: Duration,
    ) -> Self {
        Self {
            boot_id,
            capacity,
            max_record_bytes,
            retention,
            records: HashMap::new(),
        }
    }

    /// Call only after the complete body has passed its length and digest checks.
    /// Check this before resource preconditions, so a retry can retrieve its result.
    pub fn admit(
        &mut self,
        id: &str,
        intent: Intent,
        current_card_id: &str,
        initial: RecordedResult,
        now: Instant,
    ) -> Result<Admission, Problem> {
        if !valid_request_id(id) {
            return Err(Problem::new(
                400,
                "invalid_request_id",
                "The request ID must be a canonical UUID.",
            ));
        }
        if intent.boot_id != self.boot_id || intent.card_id != current_card_id {
            return Err(Problem::new(
                409,
                "context_changed",
                "Read the current boot and card identities before writing.",
            ));
        }
        self.records.retain(|_, record| {
            record
                .completed
                .is_none_or(|at| now.saturating_duration_since(at) < self.retention)
        });
        // UUID hex case does not distinguish two request identities.
        let id = id.to_ascii_lowercase();
        if let Some(record) = self.records.get(&id) {
            return if record.intent == intent {
                Ok(Admission::Existing(record.result.clone()))
            } else {
                Err(Problem::new(
                    409,
                    "request_id_conflict",
                    "The request ID belongs to a different mutation intent.",
                ))
            };
        }
        self.check_size(&intent, &initial)?;
        if self.records.len() >= self.capacity {
            return Err(Problem::new(
                503,
                "request_capacity",
                "The request cache cannot retain another result yet.",
            ));
        }
        self.records.insert(
            id,
            Record {
                intent,
                result: initial,
                completed: None,
            },
        );
        Ok(Admission::New)
    }

    pub fn update(
        &mut self,
        id: &str,
        result: RecordedResult,
        terminal: bool,
        now: Instant,
    ) -> Result<(), Problem> {
        let key = id.to_ascii_lowercase();
        let record = self.records.get(&key).ok_or_else(unknown_request)?;
        if record.completed.is_some() {
            return Err(Problem::new(
                409,
                "request_finished",
                "A terminal request result cannot change.",
            ));
        }
        self.check_size(&record.intent, &result)?;
        let record = self.records.get_mut(&key).expect("record checked above");
        record.result = result;
        record.completed = terminal.then_some(now);
        Ok(())
    }

    pub fn get(&self, id: &str, now: Instant) -> Result<&RecordedResult, Problem> {
        self.records
            .get(&id.to_ascii_lowercase())
            .filter(|record| {
                record
                    .completed
                    .is_none_or(|at| now.saturating_duration_since(at) < self.retention)
            })
            .map(|record| &record.result)
            .ok_or_else(unknown_request)
    }

    pub fn get_for_card(
        &self,
        id: &str,
        card_id: &str,
        now: Instant,
    ) -> Result<&RecordedResult, Problem> {
        let record = self
            .records
            .get(&id.to_ascii_lowercase())
            .ok_or_else(unknown_request)?;
        if record.intent.card_id != card_id || record.intent.boot_id != self.boot_id {
            return Err(unknown_request());
        }
        self.get(id, now)
    }

    fn check_size(&self, intent: &Intent, result: &RecordedResult) -> Result<(), Problem> {
        let size = serde_json::to_vec(&(intent, result))
            .expect("protocol values serialize")
            .len();
        if size > self.max_record_bytes {
            return Err(Problem::new(
                413,
                "request_record_too_large",
                "The request summary exceeds the retention limit.",
            ));
        }
        Ok(())
    }
}

fn unknown_request() -> Problem {
    Problem::new(
        404,
        "request_result_unknown",
        "Read the resource to reconcile this request.",
    )
}

pub fn valid_request_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Watermark {
    pub boot_id: String,
    pub card_id: String,
    pub snapshot_seq: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub boot_id: String,
    pub card_id: String,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

#[derive(Debug, PartialEq)]
pub enum NextChange {
    Event(Change),
    Empty,
    /// Close the socket. The client must subscribe again and read snapshots.
    ResyncRequired,
    UnknownSubscriber,
}

struct Subscriber {
    queue: VecDeque<Change>,
    overflow: bool,
}

/// Owns visible snapshot state, so its watermark cannot race with publication.
/// Wrap the hub in the runtime coordinator's mutex when shared between workers.
pub struct ChangeHub<T> {
    state: T,
    watermark: Watermark,
    max_subscribers: usize,
    queue_capacity: usize,
    max_event_bytes: usize,
    next_subscriber: u64,
    subscribers: HashMap<u64, Subscriber>,
}

impl<T: Clone> ChangeHub<T> {
    pub fn new(
        state: T,
        boot_id: String,
        card_id: String,
        max_subscribers: usize,
        queue_capacity: usize,
        max_event_bytes: usize,
    ) -> Self {
        Self {
            state,
            watermark: Watermark {
                boot_id,
                card_id,
                snapshot_seq: 0,
            },
            max_subscribers,
            queue_capacity,
            max_event_bytes,
            next_subscriber: 0,
            subscribers: HashMap::new(),
        }
    }

    /// A card change invalidates every card-scoped snapshot without resetting
    /// the boot-global reliable sequence. Existing subscribers see the change.
    pub fn replace_card(&mut self, card_id: String, state: T) -> Result<u64, Problem> {
        let previous = std::mem::replace(&mut self.watermark.card_id, card_id);
        let fields = serde_json::json!({"type":"resourceChanged","changed":"storage"})
            .as_object()
            .expect("event object")
            .clone();
        match self.publish(state, fields) {
            Ok(seq) => Ok(seq),
            Err(error) => {
                self.watermark.card_id = previous;
                Err(error)
            }
        }
    }

    pub fn snapshot(&self) -> (T, Watermark) {
        (self.state.clone(), self.watermark.clone())
    }

    /// Registration occurs before the caller sends hello or fetches snapshots.
    pub fn subscribe(&mut self) -> Result<(u64, Watermark), Problem> {
        if self.subscribers.len() >= self.max_subscribers {
            return Err(Problem::new(
                503,
                "subscriber_capacity",
                "The change subscription limit has been reached.",
            ));
        }
        let id = self
            .next_subscriber
            .checked_add(1)
            .ok_or_else(sequence_exhausted)?;
        self.next_subscriber = id;
        self.subscribers.insert(
            id,
            Subscriber {
                queue: VecDeque::new(),
                overflow: false,
            },
        );
        Ok((id, self.watermark.clone()))
    }

    pub fn unsubscribe(&mut self, id: u64) {
        self.subscribers.remove(&id);
    }

    /// Publish only after durable commit and runtime acknowledgement. Terminal
    /// request events pass an unchanged state; they still consume a reliable seq.
    pub fn publish(&mut self, state: T, fields: Map<String, Value>) -> Result<u64, Problem> {
        let seq = self
            .watermark
            .snapshot_seq
            .checked_add(1)
            .ok_or_else(sequence_exhausted)?;
        let event = self.event(Some(seq), fields)?;
        self.state = state;
        self.watermark.snapshot_seq = seq;
        self.enqueue(event);
        Ok(seq)
    }

    /// Best-effort progress does not advance the snapshot watermark.
    pub fn progress(&mut self, fields: Map<String, Value>) -> Result<(), Problem> {
        let event = self.event(None, fields)?;
        self.enqueue(event);
        Ok(())
    }

    pub fn next(&mut self, id: u64) -> NextChange {
        let Some(subscriber) = self.subscribers.get_mut(&id) else {
            return NextChange::UnknownSubscriber;
        };
        if subscriber.overflow {
            self.subscribers.remove(&id);
            return NextChange::ResyncRequired;
        }
        subscriber
            .queue
            .pop_front()
            .map_or(NextChange::Empty, NextChange::Event)
    }

    fn event(&self, seq: Option<u64>, fields: Map<String, Value>) -> Result<Change, Problem> {
        if ["seq", "bootId", "cardId"]
            .iter()
            .any(|key| fields.contains_key(*key))
        {
            return Err(Problem::new(
                400,
                "invalid_event",
                "Event fields cannot replace the coordinator identity.",
            ));
        }
        let event = Change {
            seq,
            boot_id: self.watermark.boot_id.clone(),
            card_id: self.watermark.card_id.clone(),
            fields,
        };
        if serde_json::to_vec(&event)
            .expect("protocol values serialize")
            .len()
            > self.max_event_bytes
        {
            return Err(Problem::new(
                413,
                "event_too_large",
                "The change event exceeds the queue limit.",
            ));
        }
        Ok(event)
    }

    fn enqueue(&mut self, event: Change) {
        for subscriber in self
            .subscribers
            .values_mut()
            .filter(|subscriber| !subscriber.overflow)
        {
            if subscriber.queue.len() >= self.queue_capacity {
                if event.seq.is_none() {
                    continue;
                }
                // Progress can be dropped to make room for a reliable event.
                subscriber.queue.retain(|queued| queued.seq.is_some());
                if subscriber.queue.len() >= self.queue_capacity {
                    subscriber.queue.clear();
                    subscriber.overflow = true;
                    continue;
                }
            }
            subscriber.queue.push_back(event.clone());
        }
    }
}

fn sequence_exhausted() -> Problem {
    Problem::new(
        503,
        "sequence_exhausted",
        "Restart the service before accepting more changes.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "6b23bb06-b505-4fa6-bdd4-754018494129";
    const OTHER: &str = "7b23bb06-b505-4fa6-bdd4-754018494129";
    fn intent() -> Intent {
        Intent {
            domain: "looks".into(),
            device_id: "device".into(),
            boot_id: "boot".into(),
            card_id: "card".into(),
            method: "PUT".into(),
            uri: "/v1/looks/test/content".into(),
            if_match: None,
            if_none_match: Some("*".into()),
            content_sha256: "digest".into(),
        }
    }
    fn result(status: u16) -> RecordedResult {
        RecordedResult {
            status,
            body: json!({"status": "pending"}),
        }
    }
    fn fields(kind: &str) -> Map<String, Value> {
        json!({"type":kind}).as_object().unwrap().clone()
    }

    #[test]
    fn strong_validators_cover_bytes_and_reject_stale_or_weak_conditions() {
        let tag = strong_etag(b"hello");
        assert_eq!(
            tag,
            "\"sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824\""
        );
        assert_ne!(tag, strong_etag(b"Hello"));
        assert!(check_precondition(Some(&tag), Some(&tag), None).is_ok());
        assert!(check_precondition(Some(&tag), Some(&format!("\"other\", {tag}")), None).is_ok());
        assert_eq!(
            check_precondition(Some(&tag), Some(&format!("W/{tag}")), None)
                .unwrap_err()
                .status,
            412
        );
        assert_eq!(
            check_precondition(Some(&tag), None, None)
                .unwrap_err()
                .status,
            428
        );
        assert_eq!(
            check_precondition(Some(&tag), Some("*"), None)
                .unwrap_err()
                .status,
            400
        );
        assert_eq!(
            check_precondition(None, Some(&tag), None)
                .unwrap_err()
                .status,
            412
        );
        assert!(check_precondition(None, None, Some("*")).is_ok());
        assert_eq!(
            check_precondition(Some(&tag), None, Some("*"))
                .unwrap_err()
                .status,
            412
        );
        assert_eq!(
            check_precondition(None, Some(&tag), Some("*"))
                .unwrap_err()
                .status,
            400
        );
    }

    #[test]
    fn retry_returns_result_but_changed_intent_or_context_is_rejected() {
        let now = Instant::now();
        let mut cache = RequestCache::new("boot".into(), 2, 2048, Duration::from_secs(60));
        assert_eq!(
            cache.admit(ID, intent(), "card", result(202), now).unwrap(),
            Admission::New
        );
        cache.update(ID, result(201), true, now).unwrap();
        assert_eq!(
            cache
                .admit(&ID.to_uppercase(), intent(), "card", result(202), now)
                .unwrap(),
            Admission::Existing(result(201))
        );
        let mut changed = intent();
        changed.uri.push_str("?active=off");
        assert_eq!(
            cache
                .admit(ID, changed, "card", result(202), now)
                .unwrap_err()
                .code,
            "request_id_conflict"
        );
        assert_eq!(
            cache
                .admit(ID, intent(), "other-card", result(202), now)
                .unwrap_err()
                .code,
            "context_changed"
        );
        let mut old = intent();
        old.boot_id = "old".into();
        assert_eq!(
            cache
                .admit(ID, old, "card", result(202), now)
                .unwrap_err()
                .code,
            "context_changed"
        );
        assert_eq!(
            cache.update(ID, result(500), true, now).unwrap_err().code,
            "request_finished"
        );
    }

    #[test]
    fn retention_never_evicts_active_requests_and_bounds_record_size() {
        let now = Instant::now();
        let later = now + Duration::from_secs(60);
        let mut cache = RequestCache::new("boot".into(), 1, 1024, Duration::from_secs(60));
        cache.admit(ID, intent(), "card", result(202), now).unwrap();
        assert_eq!(
            cache
                .admit(OTHER, intent(), "card", result(202), later)
                .unwrap_err()
                .code,
            "request_capacity"
        );
        cache.update(ID, result(200), true, later).unwrap();
        assert_eq!(
            cache
                .admit(OTHER, intent(), "card", result(202), later)
                .unwrap_err()
                .code,
            "request_capacity"
        );
        let expired = later + Duration::from_secs(60);
        assert_eq!(
            cache.get(ID, expired).unwrap_err().code,
            "request_result_unknown"
        );
        assert_eq!(
            cache
                .admit(OTHER, intent(), "card", result(202), expired)
                .unwrap(),
            Admission::New
        );
        let huge = RecordedResult {
            status: 200,
            body: json!("x".repeat(2048)),
        };
        assert_eq!(
            cache.update(OTHER, huge, true, expired).unwrap_err().status,
            413
        );
        assert_eq!(cache.get(OTHER, expired).unwrap().status, 202);
        assert!(!valid_request_id("../invalid"));
    }

    #[test]
    fn reliable_events_and_snapshots_share_one_global_sequence() {
        let mut hub = ChangeHub::new(0, "boot".into(), "card".into(), 2, 4, 1024);
        let (a, hello) = hub.subscribe().unwrap();
        assert_eq!(hello.snapshot_seq, 0);
        let (b, _) = hub.subscribe().unwrap();
        assert_eq!(hub.subscribe().unwrap_err().code, "subscriber_capacity");
        assert_eq!(hub.publish(1, fields("resourceChanged")).unwrap(), 1);
        assert_eq!(hub.snapshot().0, 1);
        assert_eq!(hub.snapshot().1.snapshot_seq, 1);
        assert_eq!(hub.next(a), hub.next(b));
        hub.progress(fields("requestProgress")).unwrap();
        let NextChange::Event(progress) = hub.next(a) else {
            panic!("missing progress")
        };
        assert!(serde_json::to_value(progress).unwrap().get("seq").is_none());
        assert_eq!(hub.snapshot().1.snapshot_seq, 1);
        assert_eq!(hub.publish(1, fields("requestChanged")).unwrap(), 2);
    }

    #[test]
    fn overflow_requires_new_subscription_without_blocking_publication() {
        let mut hub = ChangeHub::new(0, "boot".into(), "card".into(), 2, 1, 1024);
        let (a, _) = hub.subscribe().unwrap();
        hub.progress(fields("requestProgress")).unwrap();
        hub.publish(1, fields("resourceChanged")).unwrap();
        assert!(matches!(
            hub.next(a),
            NextChange::Event(Change { seq: Some(1), .. })
        ));
        hub.publish(2, fields("resourceChanged")).unwrap();
        hub.publish(3, fields("resourceChanged")).unwrap();
        assert_eq!(hub.snapshot().0, 3);
        assert_eq!(hub.next(a), NextChange::ResyncRequired);
        assert_eq!(hub.next(a), NextChange::UnknownSubscriber);
        assert_eq!(hub.subscribe().unwrap().1.snapshot_seq, 3);
    }

    #[test]
    fn invalid_event_does_not_change_state_or_watermark() {
        let mut hub = ChangeHub::new(0, "boot".into(), "card".into(), 1, 1, 100);
        let mut event = fields("resourceChanged");
        event.insert("seq".into(), json!(8));
        assert_eq!(hub.publish(1, event).unwrap_err().code, "invalid_event");
        assert_eq!(
            hub.publish(1, fields(&"x".repeat(200))).unwrap_err().status,
            413
        );
        assert_eq!(hub.snapshot().0, 0);
        assert_eq!(hub.snapshot().1.snapshot_seq, 0);
    }
}
