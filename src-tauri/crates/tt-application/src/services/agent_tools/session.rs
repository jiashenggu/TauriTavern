use serde::{Deserialize, Serialize};
use std::collections::{HashMap, hash_map::Entry};

use tt_domain::models::skill::{SkillIndexEntry, SkillScope};
use tt_domain::text_lines::TextLineSelection;
use tt_ports::repositories::workspace_repository::WorkspaceFile;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceReadState {
    pub sha256: String,
    pub full_read: bool,
    observed_texts: Vec<String>,
    #[serde(default)]
    line_ranges: Vec<(usize, usize)>,
    patch_requires_full_read: bool,
}

impl WorkspaceReadState {
    fn new(sha256: String, full_read: bool) -> Self {
        Self {
            sha256,
            full_read,
            observed_texts: Vec::new(),
            line_ranges: Vec::new(),
            patch_requires_full_read: false,
        }
    }

    pub fn old_string_was_observed(&self, old_string: &str) -> bool {
        self.full_read
            || self
                .observed_texts
                .iter()
                .any(|text| text.contains(old_string))
    }

    pub fn patch_requires_full_read(&self) -> bool {
        self.patch_requires_full_read
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolSession {
    #[serde(skip)]
    pub(crate) frozen_macros: std::sync::Arc<tt_domain::frozen_macros::FrozenMacros>,
    read_state: HashMap<String, WorkspaceReadState>,
    skill_read_chars: usize,
    #[serde(skip)]
    pub(crate) effective_skills: Vec<SkillIndexEntry>,
}

impl AgentToolSession {
    pub fn new(effective_skills: Vec<SkillIndexEntry>) -> Self {
        Self {
            effective_skills,
            ..Self::default()
        }
    }

    pub fn remember_file(&mut self, file: &WorkspaceFile, full_read: bool) {
        let path = file.path.as_str().to_string();
        if full_read {
            self.read_state
                .insert(path, WorkspaceReadState::new(file.sha256.clone(), true));
            return;
        }

        let previous = self.read_state.get(&path);
        let full_read =
            previous.is_some_and(|state| state.sha256 == file.sha256 && state.full_read);
        let patch_requires_full_read =
            previous.is_some_and(WorkspaceReadState::patch_requires_full_read);
        self.read_state.insert(
            path,
            WorkspaceReadState {
                sha256: file.sha256.clone(),
                full_read,
                observed_texts: Vec::new(),
                line_ranges: Vec::new(),
                patch_requires_full_read,
            },
        );
    }

    pub fn remember_file_read(&mut self, file: &WorkspaceFile, full_read: bool, text: &str) {
        if full_read {
            self.remember_file(file, true);
            return;
        }

        let path = file.path.as_str().to_string();
        match self.read_state.entry(path) {
            Entry::Occupied(mut entry) => {
                let state = entry.get_mut();
                if state.sha256 != file.sha256 {
                    let patch_requires_full_read = state.patch_requires_full_read;
                    *state = WorkspaceReadState {
                        sha256: file.sha256.clone(),
                        full_read: false,
                        observed_texts: Vec::new(),
                        line_ranges: Vec::new(),
                        patch_requires_full_read,
                    };
                }
                if !state.full_read && !text.is_empty() {
                    state.observed_texts.push(text.to_string());
                }
            }
            Entry::Vacant(entry) => {
                let mut state = WorkspaceReadState::new(file.sha256.clone(), false);
                if !text.is_empty() {
                    state.observed_texts.push(text.to_string());
                }
                entry.insert(state);
            }
        }
    }

    pub fn remember_file_selection(
        &mut self,
        file: &WorkspaceFile,
        selection: &TextLineSelection,
    ) -> bool {
        self.remember_file_read(file, !selection.truncated(), &selection.content);
        let state = self
            .read_state
            .get_mut(file.path.as_str())
            .expect("read was recorded");
        if state.full_read || selection.line_truncated {
            return state.full_read;
        }
        state
            .line_ranges
            .push((selection.start_line, selection.end_line));
        state.line_ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for &(start, end) in &state.line_ranges {
            if let Some(previous) = merged
                .last_mut()
                .filter(|previous| start <= previous.1.saturating_add(1))
            {
                previous.1 = previous.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        state.line_ranges = merged;
        if state.line_ranges.as_slice() == [(1, selection.total_lines)] {
            state.full_read = true;
            state.patch_requires_full_read = false;
            state.observed_texts.clear();
            state.line_ranges.clear();
        }
        state.full_read
    }

    pub fn remember_partial_patch(
        &mut self,
        file: &WorkspaceFile,
        old_string: &str,
        new_string: &str,
    ) {
        let path = file.path.as_str().to_string();
        let observed_texts = self
            .read_state
            .get(&path)
            .map(|state| {
                state
                    .observed_texts
                    .iter()
                    .map(|text| text.replacen(old_string, new_string, 1))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.read_state.insert(
            path,
            WorkspaceReadState {
                sha256: file.sha256.clone(),
                full_read: false,
                observed_texts,
                line_ranges: Vec::new(),
                patch_requires_full_read: false,
            },
        );
    }

    pub fn require_full_read_before_patch(&mut self, path: &str) {
        if let Some(state) = self.read_state.get_mut(path) {
            state.patch_requires_full_read = true;
        }
    }

    pub fn read_state(&self, path: &str) -> Option<&WorkspaceReadState> {
        self.read_state.get(path)
    }

    pub fn skill_read_chars(&self) -> usize {
        self.skill_read_chars
    }

    pub fn remember_skill_read_chars(&mut self, chars: usize) {
        self.skill_read_chars += chars;
    }

    pub fn effective_skills(&self) -> &[SkillIndexEntry] {
        &self.effective_skills
    }

    pub fn effective_skill_scope(&self, name: &str) -> Option<SkillScope> {
        self.effective_skills
            .iter()
            .find(|skill| skill.name == name)
            .map(|skill| skill.scope.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tt_domain::models::agent::WorkspacePath;

    #[test]
    fn paged_reads_require_gap_free_current_version_coverage_after_restore() {
        let file = WorkspaceFile {
            path: WorkspacePath::parse("persist/state.json").unwrap(),
            text: "first\nmiddle\nlast".into(),
            bytes: 17,
            sha256: "version-one".into(),
        };
        let page = |file: &WorkspaceFile, line| {
            TextLineSelection::select(&file.text, line, Some(1), 1, 100).unwrap()
        };
        let mut session = AgentToolSession::default();
        assert!(!session.remember_file_selection(&file, &page(&file, 1)));
        assert!(!session.remember_file_selection(&file, &page(&file, 3)));
        session = serde_json::from_value(serde_json::to_value(&session).unwrap()).unwrap();
        assert!(!session.remember_file_selection(&file, &page(&file, 1)));
        let newer = WorkspaceFile {
            text: "changed\nmiddle\nlast".into(),
            sha256: "version-two".into(),
            bytes: 19,
            ..file.clone()
        };
        assert!(!session.remember_file_selection(&newer, &page(&newer, 2)));
        assert!(!session.remember_file_selection(&newer, &page(&newer, 1)));
        assert!(session.remember_file_selection(&newer, &page(&newer, 3)));
    }

    #[test]
    fn truncated_lines_never_complete_paged_read_coverage() {
        let file = WorkspaceFile {
            path: WorkspacePath::parse("scratch/long.txt").unwrap(),
            text: "abcdef\nlast".into(),
            bytes: 11,
            sha256: "version".into(),
        };
        let mut session = AgentToolSession::default();
        let truncated = TextLineSelection::select(&file.text, 1, Some(1), 1, 3).unwrap();
        assert!(!session.remember_file_selection(&file, &truncated));
        let end = TextLineSelection::select(&file.text, 2, Some(1), 1, 100).unwrap();
        assert!(!session.remember_file_selection(&file, &end));
        let first = TextLineSelection::select(&file.text, 1, Some(1), 1, 100).unwrap();
        assert!(session.remember_file_selection(&file, &first));
    }
}
