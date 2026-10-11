use super::*;

#[tokio::test]
async fn reply_publication_projection_is_independent_of_event_pages_and_invocation_filters() {
    use tt_application::dto::agent_dto::AgentReadEventsDto;
    let root = temp_root("reply-publication-projection");
    let fixture = agent_runtime_fixture(&root);
    let profile = resolve_contract_profile(&fixture).await;
    let run = contract_run(
        "reply-publication",
        AgentRunPresentation::Foreground,
        &profile,
    );
    fixture.agent_repository.create_run(&run).await.unwrap();
    let read = || {
        fixture.service.read_events(AgentReadEventsDto {
            run_id: run.id.clone(),
            after_seq: None,
            before_seq: Some(u64::MAX),
            limit: 1,
            invocation_id: Some("unrelated-invocation".into()),
            include_timeline_projection: true,
        })
    };
    let empty = read()
        .await
        .unwrap()
        .timeline_projection
        .unwrap()
        .reply_publication;
    assert_eq!(empty.seq, 0);
    assert!(!empty.pending);
    for (event_type, pending) in [
        ("chat_commit_completed", true),
        ("persistent_state_metadata_updated", false),
        ("chat_commit_completed", true),
    ] {
        let publication = fixture
            .agent_repository
            .append_event(
                &run.id,
                AgentRunEventLevel::Info,
                event_type,
                json!({"invocationId":"inv_root"}),
            )
            .await
            .unwrap();
        for _ in 0..250 {
            fixture
                .agent_repository
                .append_event(
                    &run.id,
                    AgentRunEventLevel::Info,
                    "tool_call_completed",
                    json!({"invocationId":"inv_root"}),
                )
                .await
                .unwrap();
        }
        let result = read().await.unwrap();
        assert!(result.events.is_empty());
        let status = result.timeline_projection.unwrap().reply_publication;
        assert_eq!(status.seq, publication.seq);
        assert_eq!(status.pending, pending);
    }
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn missing_persist_requires_an_explicit_empty_start_before_creating_a_run() {
    for (label, metadata, expected_error) in [
        (
            "pending-state",
            json!({ "persistStateStatus": "not_committed", "persistStateId": "old_state" }),
            "agent.persist_state_pending",
        ),
        (
            "missing-id",
            json!({ "persistStateStatus": "committed" }),
            "agent.persist_state_missing",
        ),
        (
            "missing-version",
            json!({ "persistStateId": "state_missing" }),
            "agent.persistent_state_not_found",
        ),
        (
            "invalid-id",
            json!({ "persistStateId": "../invalid" }),
            "Invalid agent storage segment persist_state_id",
        ),
    ] {
        let root = temp_root(label);
        let fixture = agent_runtime_fixture(&root);
        let profile = resolve_contract_profile(&fixture).await;
        let mut chat = Chat::new("User", "Alice");
        chat.file_name = Some("story.jsonl".to_string());
        if label == "pending-state" {
            chat.messages.push(serde_json::from_value(json!({
                "name": "Alice", "mes": "Earlier completed reply.",
                "extra": { "tauritavern": { "agent": { "persistStateStatus": "committed", "persistStateId": "older_state" } } }
            })).unwrap());
        }
        chat.messages.push(
            serde_json::from_value(json!({
                "name": "Alice", "mes": "Keep this chat message.",
                "extra": { "tauritavern": { "agent": metadata } }
            }))
            .unwrap(),
        );
        fixture.chat_repository.save(&chat).await.unwrap();
        let mut dto: AgentStartRunDto = serde_json::from_value(json!({
            "chatRef": { "kind": "character", "characterId": "Alice", "fileName": "story.jsonl" },
            "stableChatId": "stable-story",
            "profileId": profile.id.as_str(),
            "promptSnapshot": { "contextPolicy": profile.context, "chatCompletionPayload": chat_request("continue").payload },
            "options": { "presentation": "background" }
        })).unwrap();

        let error = fixture.service.start_run(dto.clone()).await.unwrap_err();
        assert!(error.to_string().contains(expected_error), "{error}");
        assert!(
            fixture
                .agent_repository
                .list_all_runs()
                .await
                .unwrap()
                .is_empty()
        );
        assert!(fixture.model_gateway.requests().await.is_empty());

        if label != "invalid-id" {
            dto.options.start_with_empty_persist = true;
            let handle = fixture.service.start_run(dto).await.unwrap();
            // The live channel closes when runtime releases the active run; no polling sleep.
            if let Some(mut live) = fixture
                .service
                .subscribe_live_projection(&handle.run_id)
                .await
                .unwrap()
            {
                tokio::time::timeout(AGENT_CONTRACT_ASYNC_TIMEOUT, async {
                    while live.changed().await.is_ok() {}
                })
                .await
                .unwrap();
            }
            let run = fixture
                .agent_repository
                .load_run(&handle.run_id)
                .await
                .unwrap();
            assert_eq!(run.status, AgentRunStatus::Completed);
            assert_eq!(run.persist_base_state_id, None);
            assert_eq!(run.input_message_count, Some(chat.messages.len()));
            let files = fixture
                .agent_repository
                .list_files(
                    &run.id,
                    Some(&WorkspacePath::parse("persist").unwrap()),
                    1,
                    10,
                )
                .await
                .unwrap();
            assert!(files.entries.is_empty());
            assert_eq!(fixture.model_gateway.requests().await.len(), 2);
        }
        let unchanged = fixture
            .chat_repository
            .get_chat("Alice", "story.jsonl")
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(unchanged.messages).unwrap(),
            serde_json::to_value(chat.messages).unwrap()
        );
        fs::remove_dir_all(root).await.unwrap();
    }
}
