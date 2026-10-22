use async_trait::async_trait;

use crate::services::agent_workspace_scope::AGENT_TOOL_RESULTS_ROOT;
use tt_domain::errors::DomainError;
use tt_domain::models::agent::profile::ResolvedAgentProfile;
use tt_domain::models::agent::{
    AgentRun, WorkspaceManifest, WorkspacePath, WorkspacePersistentChangeSet,
};
use tt_ports::repositories::workspace_repository::{
    WorkspaceAppendResult, WorkspaceFile, WorkspaceFileList, WorkspaceRepository,
    WorkspaceWriteGuard,
};

pub(in crate::services::agent_runtime_service) struct InvocationWorkspaceRepository<'a> {
    inner: &'a dyn WorkspaceRepository,
    profile: &'a ResolvedAgentProfile,
}

impl<'a> InvocationWorkspaceRepository<'a> {
    pub(in crate::services::agent_runtime_service) fn new(
        inner: &'a dyn WorkspaceRepository,
        profile: &'a ResolvedAgentProfile,
    ) -> Self {
        Self { inner, profile }
    }

    fn is_event_archive_state(&self, path: &WorkspacePath) -> Result<bool, DomainError> {
        const STATE_PATH: &str = "persist/state.json";
        if self.profile.context.event_archive.is_none()
            || !path.as_str().eq_ignore_ascii_case(STATE_PATH)
        {
            return Ok(false);
        }
        // Reserve one spelling on every platform: case variants can overwrite
        // the same file on Windows and create divergent state files on Linux.
        if path.as_str() != STATE_PATH {
            return Err(DomainError::InvalidData(format!(
                "agent.event_archive_invalid: use the canonical state path `{STATE_PATH}`"
            )));
        }
        Ok(true)
    }

    fn validate_state_write(&self, path: &WorkspacePath, text: &str) -> Result<(), DomainError> {
        if self.is_event_archive_state(path)? {
            tt_domain::models::agent::event_archive::validate_event_archive_state(text)?;
        }
        Ok(())
    }
}

#[async_trait]
impl WorkspaceRepository for InvocationWorkspaceRepository<'_> {
    async fn validate_persistent_state(
        &self,
        workspace_id: &str,
        state_id: &str,
    ) -> Result<(), DomainError> {
        self.inner
            .validate_persistent_state(workspace_id, state_id)
            .await
    }

    async fn initialize_run(
        &self,
        run: &AgentRun,
        manifest: &WorkspaceManifest,
        prompt_snapshot: &serde_json::Value,
        resolved_profile: &ResolvedAgentProfile,
    ) -> Result<(), DomainError> {
        self.inner
            .initialize_run(run, manifest, prompt_snapshot, resolved_profile)
            .await
    }

    async fn read_manifest(&self, run_id: &str) -> Result<WorkspaceManifest, DomainError> {
        let mut manifest = self.inner.read_manifest(run_id).await?;
        for root in &mut manifest.roots {
            if root.path == AGENT_TOOL_RESULTS_ROOT {
                root.visible = true;
                root.writable = false;
                continue;
            }
            if root.path == "baseline" {
                root.visible = !self.profile.context.initial_baseline_files.is_empty()
                    && self.profile.workspace.visible_roots.iter().any(|root| root == "persist");
                root.writable = false;
                continue;
            }
            root.visible = self
                .profile
                .workspace
                .visible_roots
                .iter()
                .any(|visible| visible == &root.path);
            root.writable = self
                .profile
                .workspace
                .writable_roots
                .iter()
                .any(|writable| writable == &root.path);
        }
        Ok(manifest)
    }

    async fn write_text(
        &self,
        run_id: &str,
        path: &WorkspacePath,
        text: &str,
    ) -> Result<WorkspaceFile, DomainError> {
        self.validate_state_write(path, text)?;
        self.inner.write_text(run_id, path, text).await
    }

    async fn write_text_guarded(
        &self,
        run_id: &str,
        path: &WorkspacePath,
        text: &str,
        guard: WorkspaceWriteGuard,
    ) -> Result<WorkspaceFile, DomainError> {
        self.validate_state_write(path, text)?;
        self.inner
            .write_text_guarded(run_id, path, text, guard)
            .await
    }

    async fn append_text(
        &self,
        run_id: &str,
        path: &WorkspacePath,
        text: &str,
    ) -> Result<WorkspaceAppendResult, DomainError> {
        if self.is_event_archive_state(path)? {
            return Err(DomainError::InvalidData(
                "agent.event_archive_invalid: persist/state.json requires a complete JSON replacement; append is not supported"
                    .into(),
            ));
        }
        self.inner.append_text(run_id, path, text).await
    }

    async fn read_text(
        &self,
        run_id: &str,
        path: &WorkspacePath,
    ) -> Result<WorkspaceFile, DomainError> {
        self.inner.read_text(run_id, path).await
    }

    async fn list_files(
        &self,
        run_id: &str,
        path: Option<&WorkspacePath>,
        depth: usize,
        max_entries: usize,
    ) -> Result<WorkspaceFileList, DomainError> {
        self.inner
            .list_files(run_id, path, depth, max_entries)
            .await
    }

    async fn commit_persistent_changes(
        &self,
        run_id: &str,
        previous_state_id: Option<&str>,
    ) -> Result<WorkspacePersistentChangeSet, DomainError> {
        self.inner
            .commit_persistent_changes(run_id, previous_state_id)
            .await
    }
}
