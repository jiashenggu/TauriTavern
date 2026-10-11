use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tt_domain::errors::DomainError;
use tt_ports::repositories::chat_payload_commit_repository::{
    ChatMessageSnapshot, ChatPayloadTarget,
};

use super::FileChatRepository;
use crate::file_system::persist_file_blocking;

fn invalid(error: impl std::fmt::Display) -> DomainError {
    DomainError::InvalidData(format!("chat.message_edit_failed: {error}"))
}

impl FileChatRepository {
    pub(super) async fn message_snapshot(
        &self,
        target: ChatPayloadTarget,
        index: usize,
    ) -> Result<ChatMessageSnapshot, DomainError> {
        let path = self.resolve_chat_commit_target(&target).await?;
        let guard = self.acquire_payload_mutation_lock(&path).await;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            scan_message(&path, index, None)
        })
        .await
        .map_err(invalid)?
    }

    pub(super) async fn edit_message(
        &self,
        target: ChatPayloadTarget,
        index: usize,
        expected: String,
        message: Value,
    ) -> Result<(), DomainError> {
        let path = self.resolve_chat_commit_target(&target).await?;
        let guard = self.acquire_payload_mutation_lock(&path).await;
        let write_path = path.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let temp = FileChatRepository::temp_payload_path(&write_path);
            let mut out = std::fs::File::options()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(invalid)?;
            let result = (|| {
                let replacement = serde_json::to_vec(&message).map_err(invalid)?;
                let snapshot = scan_message(&write_path, index, Some((&mut out, &replacement)))?;
                if snapshot.chat_sha256 != expected {
                    return Err(DomainError::Conflict(
                        "agent.state_conflict: chat changed; reload before saving".into(),
                    ));
                }
                persist_file_blocking(out, &temp, &write_path)
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&temp);
            }
            result
        })
        .await
        .map_err(invalid)??;
        self.invalidate_chat_caches(&target, &path).await
    }
}

fn scan_message(
    path: &Path,
    index: usize,
    mut output: Option<(&mut std::fs::File, &[u8])>,
) -> Result<ChatMessageSnapshot, DomainError> {
    let mut reader = BufReader::new(std::fs::File::open(path).map_err(invalid)?);
    let mut line = Vec::new();
    let mut hash = Sha256::new();
    let mut record = 0;
    let mut message = None;
    let mut chat_metadata = Value::Null;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).map_err(invalid)? == 0 {
            break;
        }
        hash.update(&line);
        let nonempty = line.iter().any(|byte| !byte.is_ascii_whitespace());
        let selected = nonempty && record == index.saturating_add(1);
        if nonempty {
            if record == 0 || selected {
                let decoded = super::windowed_payload_io::decode_jsonl_line_bytes(&line)?;
                let value: Value = tt_domain::json_input::from_str(&decoded).map_err(invalid)?;
                if record == 0 {
                    chat_metadata = value.get("chat_metadata").cloned().unwrap_or(Value::Null);
                } else {
                    message = Some(value);
                }
            }
            record += 1;
        }
        if let Some((out, replacement)) = output.as_mut() {
            if selected {
                out.write_all(replacement).map_err(invalid)?;
                out.write_all(b"\n").map_err(invalid)?;
            } else {
                out.write_all(&line).map_err(invalid)?;
            }
        }
    }
    Ok(ChatMessageSnapshot {
        message: message
            .ok_or_else(|| DomainError::NotFound("agent.state_message_missing".into()))?,
        chat_metadata,
        chat_sha256: hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}
