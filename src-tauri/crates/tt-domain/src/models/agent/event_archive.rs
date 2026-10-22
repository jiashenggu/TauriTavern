use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::WorkspacePath;
use crate::errors::DomainError;

pub const EVENT_ARCHIVE_KIND: &str = "agent.eventArchive";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventArchivePolicy {
    pub max_recent_events: usize,
    pub keep_recent_events: usize,
}

impl EventArchivePolicy {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.keep_recent_events == 0
            || self.keep_recent_events >= self.max_recent_events
            || self.max_recent_events > 4096
        {
            return Err(invalid(
                "require 0 < keepRecentEvents < maxRecentEvents <= 4096",
            ));
        }
        Ok(())
    }
}

pub struct EventArchiveReference {
    pub path: WorkspacePath,
    pub sha256: String,
    pub event_ids: Vec<String>,
}

pub struct EventArchiveBatch {
    pub archive_text: String,
    pub event_ids: Vec<String>,
    pub references: Vec<EventArchiveReference>,
    state: Value,
    index: Value,
}

impl EventArchiveBatch {
    pub fn state_text(mut self, path: &WorkspacePath, sha256: &str) -> Result<String, DomainError> {
        if !path.as_str().starts_with("persist/history/")
            || !path.as_str().ends_with(".json")
            || sha256.len() != 64
            || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid("invalid archive path or SHA"));
        }
        self.index["path"] = json!(path.as_str());
        self.index["sha256"] = json!(sha256);
        self.state["archives"]
            .as_array_mut()
            .expect("validated archives")
            .push(self.index);
        let text = serde_json::to_string_pretty(&self.state)
            .map_err(|error| invalid(&error.to_string()))?;
        if text.len() > 8 * 1024 * 1024 {
            return Err(invalid("resulting state exceeds 8 MiB"));
        }
        Ok(text)
    }
}

/// Validate the structure and identities consumed by automatic event archiving,
/// even when the current event count has not reached the archival threshold.
pub fn validate_event_archive_state(text: &str) -> Result<(), DomainError> {
    parse_event_archive_state(text).map(|_| ())
}

struct ValidatedArchiveState {
    state: Value,
    event_ids: Vec<String>,
    references: Vec<EventArchiveReference>,
}

fn parse_event_archive_state(text: &str) -> Result<ValidatedArchiveState, DomainError> {
    if text.len() > 8 * 1024 * 1024 {
        return Err(invalid("state exceeds 8 MiB"));
    }
    let state: Value =
        crate::json_input::from_str(text).map_err(|error| invalid(&error.to_string()))?;
    let object = state
        .as_object()
        .ok_or_else(|| invalid("persist/state.json must be a JSON object"))?;
    for field in ["recentEvents", "archives"] {
        match object.get(field) {
            Some(Value::Array(_)) => {}
            Some(_) => return Err(invalid(&format!("{field} must be an array"))),
            None => {
                return Err(invalid(&format!(
                    "persist/state.json is missing `{field}`; eventArchive requires top-level recentEvents and archives arrays"
                )));
            }
        }
    }
    let events = state["recentEvents"].as_array().expect("validated events");
    let archives = state["archives"].as_array().expect("validated archives");
    let mut seen = BTreeSet::new();
    let mut references = Vec::new();
    for archive in archives
        .iter()
        .filter(|entry| entry["kind"] == EVENT_ARCHIVE_KIND)
    {
        let ids = archive["eventIds"]
            .as_array()
            .ok_or_else(|| invalid("managed archive has no eventIds"))?;
        let mut event_ids = Vec::new();
        for id in ids {
            let id = id
                .as_str()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| invalid("invalid archived event ID"))?;
            if !seen.insert(id.to_owned()) {
                return Err(invalid("duplicate archived event ID"));
            }
            event_ids.push(id.to_owned());
        }
        let path = WorkspacePath::parse(
            archive["path"]
                .as_str()
                .ok_or_else(|| invalid("archive path missing"))?,
        )
        .map_err(|error| invalid(&format!("invalid archive path: {error}")))?;
        let sha256 = archive["sha256"]
            .as_str()
            .ok_or_else(|| invalid("archive SHA missing"))?
            .to_owned();
        if !path.as_str().starts_with("persist/history/")
            || sha256.len() != 64
            || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid("invalid managed archive reference"));
        }
        references.push(EventArchiveReference {
            path,
            sha256,
            event_ids,
        });
    }
    let mut ids = Vec::with_capacity(events.len());
    for event in events {
        let id = event
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty() && id.len() <= 512)
            .ok_or_else(|| {
                invalid("every event must have a nonempty string ID, at most 512 bytes")
            })?;
        if !seen.insert(id.to_owned()) {
            return Err(invalid(
                "duplicate event ID; resolve history overlap before archiving",
            ));
        }
        ids.push(id.to_owned());
    }
    Ok(ValidatedArchiveState {
        state,
        event_ids: ids,
        references,
    })
}

pub fn prepare_event_archive(
    text: &str,
    policy: &EventArchivePolicy,
) -> Result<Option<EventArchiveBatch>, DomainError> {
    policy.validate()?;
    // Startup and writes share all structural checks. Deferring identity checks
    // until a batch is needed would let a successful write break a later Run.
    let ValidatedArchiveState {
        mut state,
        event_ids: mut ids,
        references,
    } = parse_event_archive_state(text)?;
    if ids.len() <= policy.max_recent_events {
        return Ok(None);
    }
    let count = ids.len() - policy.keep_recent_events;
    ids.truncate(count);
    let events = state["recentEvents"]
        .as_array_mut()
        .expect("validated events");
    let archived: Vec<Value> = events.drain(..count).collect();
    let participants: BTreeSet<&str> = archived
        .iter()
        .filter_map(|event| event.get("participants").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let index = json!({
        "kind": EVENT_ARCHIVE_KIND,
        "name": format!("Archived events ({count})"),
        "eventCount": count,
        "eventIds": ids,
        "participants": participants,
        "firstListedTime": archived.first().and_then(|event| event.get("time")),
        "lastListedTime": archived.last().and_then(|event| event.get("time")),
    });
    let archive = json!({ "schemaVersion": 1, "kind": EVENT_ARCHIVE_KIND, "events": archived });
    let archive_text =
        serde_json::to_string_pretty(&archive).map_err(|error| invalid(&error.to_string()))?;
    if archive_text.len() > 8 * 1024 * 1024 {
        return Err(invalid("archive exceeds 8 MiB"));
    }
    Ok(Some(EventArchiveBatch {
        archive_text,
        event_ids: ids,
        references,
        state,
        index,
    }))
}

fn invalid(message: &str) -> DomainError {
    DomainError::InvalidData(format!("agent.event_archive_invalid: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archival_requires_explicit_valid_bounds_and_unambiguous_event_identity() {
        for (max, keep) in [(0, 0), (32, 0), (16, 16), (16, 17), (4097, 16)] {
            assert!(
                EventArchivePolicy {
                    max_recent_events: max,
                    keep_recent_events: keep
                }
                .validate()
                .is_err()
            );
        }
        let policy = EventArchivePolicy {
            max_recent_events: 2,
            keep_recent_events: 1,
        };
        assert!(
            prepare_event_archive(
                r#"{"recentEvents":[{"id":"a"},{"id":"b"}],"archives":[]}"#,
                &policy
            )
            .unwrap()
            .is_none()
        );
        for text in [
            r#"{"turn_events":["recorded event"]}"#,
            r#"{"recentEvents":{},"archives":[]}"#,
            r#"{"recentEvents":[]}"#,
            r#"{"recentEvents":[],"archives":null}"#,
            r#"{"recentEvents":[{"id":"a"},{"id":"a"}],"archives":[]}"#,
            r#"{"recentEvents":[{}],"archives":[]}"#,
            r#"{"recentEvents":[{"id":"a"},{"id":"b"},{"id":"c"}],"archives":{}}"#,
            r#"{"recentEvents":[{"id":"a"},{"id":"b"},{"id":"c"}],"archives":[{"kind":"agent.eventArchive","eventIds":["a"]}]}"#,
        ] {
            assert!(prepare_event_archive(text, &policy).is_err());
        }
        let overlap = json!({"recentEvents":[{"id":"a"},{"id":"b"},{"id":"c"}],"archives":[{
            "kind":EVENT_ARCHIVE_KIND,"eventIds":["a"],"path":"persist/history/old.json","sha256":"0".repeat(64)
        }]}).to_string();
        let error = prepare_event_archive(&overlap, &policy)
            .err()
            .expect("overlap rejected");
        assert!(error.to_string().contains("duplicate event ID"));
    }
}
