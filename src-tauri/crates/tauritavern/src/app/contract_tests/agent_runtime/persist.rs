use super::*;

#[tokio::test]
async fn agent_state_editor_versions_edits_preserves_swipes_and_rebases_revision() {
    use tt_application::dto::agent_dto::{
        AgentCancelRunDto, AgentOutputRevisionDto, AgentResumeRunDto, AgentSaveStateDto,
        AgentStateFieldDto, AgentStateTargetDto,
    };
    use tt_ports::repositories::chat_payload_commit_repository::{
        ChatPayloadCommitRepository, ChatPayloadTarget,
    };
    let root = temp_root("state-editor");
    let source = r#"{"schemaVersion":1,"revision":1,"scene":{"location":"old"},"balance":123456789012345678901,"fraction":0.123456789012345678901}"#;
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "state",
                    "workspace_write_file",
                    json!({"path":"persist/state.json","content":source}),
                ),
                model_tool_call(
                    "archive",
                    "workspace_write_file",
                    json!({"path":"persist/archive.json","content":"{\"keep\":true}"}),
                ),
                model_tool_call("finish", "workspace_finish", json!({})),
            ]),
            model_tool_response(vec![
                model_tool_call(
                    "read_manual",
                    "workspace_read_file",
                    json!({"path":"persist/state.json"}),
                ),
                model_tool_call("finish_revision", "workspace_finish", json!({})),
            ]),
        ],
    );
    let profile = resolve_contract_profile(&fixture).await;
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Background,
        "state-editor",
        Some(false),
    )
    .await;
    let checkpoint = super::resume::wait_for_checkpoint(&fixture, &handle.run_id).await;
    let run = fixture
        .agent_repository
        .load_run(&handle.run_id)
        .await
        .unwrap();
    let events = read_agent_events(&fixture.agent_repository, &run.id).await;
    let state_id = events
        .iter()
        .find(|event| event.event_type == "persistent_changes_committed")
        .unwrap()
        .payload["stateId"]
        .as_str()
        .unwrap()
        .to_owned();
    let chat_target = ChatPayloadTarget::Character {
        character_id: "Alice".into(),
        file_name: "state-editor.jsonl".into(),
    };
    let metadata =
        json!({"runId":run.id,"persistStateStatus":"committed","persistStateId":state_id});
    let header = json!({"user_name":"User","character_name":"Alice","chat_metadata":{"integrity":run.stable_chat_id,"unrelated":{"keep":true}}});
    let message = json!({"name":"Alice","is_user":false,"is_system":false,"mes":"Unchanged body","swipe_id":0,
        "swipes":["Unchanged body","Other body"],"extra":{"custom":{"keep":true},"tauritavern":{"agent":metadata}},
        "swipe_info":[{"extra":{"custom":{"keep":true},"tauritavern":{"agent":metadata}}},{"extra":{"tauritavern":{"agent":metadata}}}]});
    let payload = format!("{header}\n{message}\n");
    let session = fixture
        .chat_repository
        .begin(chat_target.clone(), true, None)
        .await
        .unwrap();
    fixture
        .chat_repository
        .append(&session.session_id, 0, payload.as_bytes())
        .await
        .unwrap();
    fixture
        .chat_repository
        .finish(&session.session_id, payload.len() as u64)
        .await
        .unwrap();
    let target = AgentStateTargetDto {
        chat_ref: run.chat_ref.clone(),
        stable_chat_id: run.stable_chat_id.clone(),
        message_index: 0,
        swipe_id: Some(0),
    };
    let document = fixture
        .service
        .read_state_document(target.clone())
        .await
        .unwrap();
    assert!(!document.read_only);
    assert!(document.fields.iter().any(|field| field.pointer == "/balance" && field.value_json == "123456789012345678901"));
    let request = |text: &str| -> AgentSaveStateDto {
        serde_json::from_value(json!({"editId":uuid::Uuid::new_v4().to_string(),"target":target,"expectedStateId":document.state_id,"expectedSha256":document.sha256,"expectedChatSha256":document.chat_sha256,"text":text})).unwrap()
    };
    assert!(
        fixture
            .service
            .save_state_document(request("{invalid"))
            .await
            .is_err()
    );
    let mut form = request(source);
    form.text = None;
    form.edits = Some(vec![AgentStateFieldDto {
        pointer: "/scene/location".into(),
        kind: "string".into(),
        value_json: "\"manual\"".into(),
    }]);
    let (first, second) = tokio::join!(
        fixture
            .service
            .save_state_document(request(&source.replace("old", "manual"))),
        fixture.service.save_state_document(form),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let saved = first.or(second).unwrap();
    let mut replay = request(&source.replace("old", "manual"));
    replay.edit_id = saved.agent_metadata["manualStateEdit"]["editId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        fixture
            .service
            .save_state_document(replay)
            .await
            .unwrap()
            .state_id,
        saved.state_id
    );
    assert_ne!(saved.state_id, state_id);
    assert!(
        fixture
            .service
            .save_state_document(request(&source.replace("old", "stale")))
            .await
            .is_err()
    );
    let original = fixture
        .agent_repository
        .read_persistent_file(
            &run.workspace_id,
            &state_id,
            &WorkspacePath::parse("persist/state.json").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(original.text, source);
    let edited = fixture
        .service
        .read_state_document(target.clone())
        .await
        .unwrap();
    let value: Value = serde_json::from_str(&edited.text).unwrap();
    assert_eq!(value["scene"]["location"], "manual");
    assert_eq!(value["revision"], 2);
    assert_eq!(value["balance"].to_string(), "123456789012345678901");
    assert_eq!(value["fraction"].to_string(), "0.123456789012345678901");
    assert_eq!(
        fixture
            .agent_repository
            .read_persistent_file(
                &run.workspace_id,
                &saved.state_id,
                &WorkspacePath::parse("persist/archive.json").unwrap()
            )
            .await
            .unwrap()
            .text,
        "{\"keep\":true}"
    );
    assert!(
        fixture
            .chat_repository
            .replace_message_guarded(
                chat_target.clone(),
                0,
                &document.chat_sha256,
                message.clone()
            )
            .await
            .is_err()
    );
    let snapshot = fixture
        .chat_repository
        .read_message_snapshot(chat_target, 0)
        .await
        .unwrap();
    assert_eq!(snapshot.chat_metadata, header["chat_metadata"]);
    assert_eq!(snapshot.message["mes"], message["mes"]);
    assert_eq!(snapshot.message["swipes"], message["swipes"]);
    assert_eq!(
        snapshot.message["extra"]["custom"],
        message["extra"]["custom"]
    );
    assert_eq!(snapshot.message["swipe_info"][1], message["swipe_info"][1]);
    assert_eq!(
        snapshot.message["swipe_info"][0]["extra"],
        snapshot.message["extra"]
    );
    assert_eq!(fixture.model_gateway.requests().await.len(), 1);
    fixture
        .service
        .resume_run(AgentResumeRunDto {
            run_id: run.id.clone(),
            expected_terminal_seq: checkpoint.terminal_seq,
            chat_ref: run.chat_ref.clone(),
            stable_chat_id: run.stable_chat_id.clone(),
            additional_rounds: 0,
            host_presentation: false,
            revision: Some(AgentOutputRevisionDto {
                guidance: "Revise prose only".into(),
                previous_output: "Unchanged body".into(),
            }),
        })
        .await
        .unwrap();
    let revised = super::resume::wait_for_checkpoint(&fixture, &run.id).await;
    assert_eq!(revised.run.status, AgentRunStatus::Completed);
    let state = read_workspace_json(&fixture.agent_repository, &run.id, "persist/state.json").await;
    assert_eq!(state, value);
    fixture
        .model_gateway
        .wait_for_cancel_on_request
        .store(3, Ordering::SeqCst);
    let next = fixture.service.start_run(serde_json::from_value(json!({
        "chatRef":run.chat_ref,"stableChatId":run.stable_chat_id,"profileId":profile.id.as_str(),
        "promptSnapshot":{"contextPolicy":profile.context,"chatCompletionPayload":chat_request("continue after edit").payload},
        "options":{"presentation":"background"}
    })).unwrap()).await.unwrap();
    let mut requests = fixture.model_gateway.request_count.subscribe();
    tokio::time::timeout(
        AGENT_CONTRACT_ASYNC_TIMEOUT,
        requests.wait_for(|count| *count >= 3),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        fixture
            .agent_repository
            .load_run(&next.run_id)
            .await
            .unwrap()
            .persist_base_state_id
            .as_deref(),
        Some(saved.state_id.as_str())
    );
    assert_eq!(
        read_workspace_json(
            &fixture.agent_repository,
            &next.run_id,
            "persist/state.json"
        )
        .await,
        value
    );
    assert!(
        fixture
            .service
            .read_state_document(target.clone())
            .await
            .unwrap()
            .read_only
    );
    assert!(
        fixture
            .service
            .save_state_document(request(&source.replace("old", "blocked")))
            .await
            .unwrap_err()
            .to_string()
            .contains("read-only")
    );
    fixture
        .service
        .cancel_run(AgentCancelRunDto {
            run_id: next.run_id.clone(),
        })
        .await
        .unwrap();
    super::resume::wait_for_checkpoint(&fixture, &next.run_id).await;
    fs::remove_dir_all(root).await.unwrap();
}

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
