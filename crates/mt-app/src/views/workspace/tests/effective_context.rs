use super::super::review::ReviewFlow;
use crate::i18n;
use mt_core::agent_artifacts::context::{CODEX_PROFILE_ID, ContextInput, ProjectTrust, resolve};
use mt_core::agent_artifacts::package::ReviewTarget;
use mt_core::document::lifecycle::{AsyncSnapshot, DocumentId};
use std::fs;

fn context_fixture() -> (tempfile::TempDir, ContextInput) {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    let home = directory.path().join("home");
    fs::create_dir_all(workspace.join(".git")).unwrap();
    fs::create_dir(&home).unwrap();
    fs::write(workspace.join("document.md"), "# Artifact\n").unwrap();
    fs::write(workspace.join("AGENTS.md"), "Preserve source identity.\n").unwrap();
    fs::write(home.join("AGENTS.md"), "Name every outbound source.\n").unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let input = ContextInput {
        target: workspace.join("document.md"),
        cwd: workspace.clone(),
        workspace,
        profile_id: CODEX_PROFILE_ID.to_owned(),
        codex_home: fs::canonicalize(home).unwrap(),
        codex_home_override: true,
        fallback_filenames: Vec::new(),
        project_doc_max_bytes: 32768,
        project_root_markers: vec![".git".to_owned()],
        project_trust: ProjectTrust::Unspecified,
    };
    (directory, input)
}

fn install_context(flow: &mut ReviewFlow, input: &ContextInput) {
    let generation = flow.begin_context_resolution(Some(input.clone()));
    assert!(flow.accept_context_resolution(generation, resolve(input, &[])));
}

#[cfg(feature = "model-transport")]
fn observe_review_state(
    workspace: &gpui_kit::Entity<super::Workspace>,
    cx: &gpui_kit::VisualTestContext,
    ready: impl Fn(&super::Workspace, &gpui_kit::App) -> bool + 'static,
) -> (gpui_kit::Subscription, std::sync::mpsc::Receiver<()>) {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let mut sender = Some(sender);
    let subscription = cx.cx.update(|app| {
        app.observe(workspace, move |workspace, app| {
            if ready(workspace.read(app), app)
                && let Some(sender) = sender.take()
            {
                sender.send(()).unwrap();
            }
        })
    });
    (subscription, receiver)
}

#[cfg(feature = "model-transport")]
#[gpui_kit::test]
fn kit_effective_context_review_rejects_source_changed_during_consent_without_watcher(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    use mt_core::review::ReviewDiagnosticCode;
    use std::{io::ErrorKind, net::TcpListener, time::Duration};

    let (_directory, input) = context_fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (workspace, cx) = super::open_test_workspace(cx, input.target.clone());
    super::configure_test_translation(cx, &base_url);
    let harness = workspace.read_with(cx, |workspace, _| workspace.harness.clone().unwrap());
    harness.update(cx, |harness, cx| {
        harness.set_context_input_for_test(input.clone(), cx)
    });
    cx.run_until_parked();
    workspace.update(cx, |workspace, cx| {
        // Consent-time revalidation must work even when no watcher is delivering events.
        workspace.watcher = None;
        workspace.open_review_panel(ReviewTarget::Document, cx);
    });
    cx.update(|window, app| {
        window.render_frame(app);
        window.click("review-context-source-1", app);
    });
    let (document_id, snapshot, frozen) = workspace.read_with(cx, |workspace, app| {
        let document = workspace.active_document().unwrap().read(app);
        (
            document.id(),
            document.async_snapshot(app),
            workspace
                .review_flow
                .selected_context(document.id())
                .unwrap(),
        )
    });
    assert_eq!(frozen.selected().sources().len(), 1);
    assert_eq!(
        frozen.selected().sources()[0].occurrence().path,
        input.workspace.join("AGENTS.md")
    );
    assert_eq!(
        frozen.selected().contents()[0].text,
        "Preserve source identity.\n"
    );
    let prompt_cx = cx.cx.clone();
    let (_prompt_subscription, prompt_opened) =
        observe_review_state(&workspace, cx, move |workspace, _| {
            workspace.review_flow.is_reviewing() && prompt_cx.has_pending_prompt()
        });
    let (_completion_subscription, completed) =
        observe_review_state(&workspace, cx, |workspace, _| {
            !workspace.review_flow.is_reviewing()
                && workspace.review_flow.review_diagnostic().is_some()
        });
    cx.update(|window, app| {
        workspace.update(app, |workspace, cx| {
            workspace.review(ReviewTarget::Document, window, cx);
        });
    });
    cx.run_until_parked();
    prompt_opened.recv_timeout(Duration::from_secs(10)).unwrap();
    let (_, disclosure) = cx
        .pending_prompt()
        .expect("the actual Review consent prompt");
    assert!(disclosure.contains(input.workspace.join("AGENTS.md").to_str().unwrap()));
    assert!(!disclosure.contains(input.codex_home.join("AGENTS.md").to_str().unwrap()));
    assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);

    fs::write(
        input.workspace.join("AGENTS.md"),
        "Changed after disclosure.\n",
    )
    .unwrap();
    workspace.read_with(cx, |workspace, app| {
        assert!(workspace.watcher.is_none());
        assert!(
            workspace
                .review_flow
                .context_request_is_current(document_id, &frozen)
        );
        assert_eq!(
            workspace
                .review_flow
                .selected_context(document_id)
                .unwrap()
                .selected(),
            frozen.selected()
        );
        assert_eq!(
            workspace
                .active_document()
                .unwrap()
                .read(app)
                .async_snapshot(app),
            snapshot
        );
    });
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Send");
    cx.run_until_parked();
    completed.recv_timeout(Duration::from_secs(10)).unwrap();

    assert!(!cx.has_pending_prompt());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        ErrorKind::WouldBlock,
        "obsolete selected source must be rejected before any outbound connection"
    );
    workspace.read_with(cx, |workspace, app| {
        let flow = &workspace.review_flow;
        let diagnostic = flow.review_diagnostic().unwrap();
        assert_eq!(diagnostic.document_id, document_id);
        assert_eq!(
            diagnostic.diagnostic.code,
            ReviewDiagnosticCode::InvalidRequest
        );
        assert!(!flow.is_reviewing());
        assert!(flow.review_result().is_none());
        assert!(flow.revision_context().is_none());
        let document = workspace.active_document().unwrap().read(app);
        assert_eq!(document.async_snapshot(app), snapshot);
        assert_eq!(document.text(app), "# Artifact\n");
        assert!(!document.is_dirty());
    });
    assert_eq!(fs::read(&input.target).unwrap(), b"# Artifact\n");
}

#[cfg(feature = "model-transport")]
#[gpui_kit::test]
fn kit_effective_context_review_approval_sends_one_selected_only_request(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    use mt_core::agent_artifacts::context::SelectedContext;
    use mt_core::review::ReviewStatus;
    use std::{
        io::{BufRead as _, BufReader, ErrorKind, Read as _, Write as _},
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    let (_directory, input) = context_fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (workspace, cx) = super::open_test_workspace(cx, input.target.clone());
    super::configure_test_translation(cx, &base_url);
    let harness = workspace.read_with(cx, |workspace, _| workspace.harness.clone().unwrap());
    harness.update(cx, |harness, cx| {
        harness.set_context_input_for_test(input.clone(), cx)
    });
    cx.run_until_parked();
    workspace.update(cx, |workspace, cx| {
        workspace.watcher = None;
        workspace.open_review_panel(ReviewTarget::Document, cx);
    });
    cx.update(|window, app| {
        window.render_frame(app);
        window.click("review-context-source-1", app);
    });
    let (document_id, snapshot, frozen) = workspace.read_with(cx, |workspace, app| {
        let document = workspace.active_document().unwrap().read(app);
        (
            document.id(),
            document.async_snapshot(app),
            workspace
                .review_flow
                .selected_context(document.id())
                .unwrap(),
        )
    });
    let prompt_cx = cx.cx.clone();
    let (_prompt_subscription, prompt_opened) =
        observe_review_state(&workspace, cx, move |workspace, _| {
            workspace.review_flow.is_reviewing() && prompt_cx.has_pending_prompt()
        });
    let (_completion_subscription, completed) =
        observe_review_state(&workspace, cx, |workspace, _| {
            !workspace.review_flow.is_reviewing()
                && (workspace.review_flow.review_result().is_some()
                    || workspace.review_flow.review_diagnostic().is_some())
        });
    cx.update(|window, app| {
        workspace.update(app, |workspace, cx| {
            workspace.review(ReviewTarget::Document, window, cx);
        });
    });
    cx.run_until_parked();
    prompt_opened.recv_timeout(Duration::from_secs(10)).unwrap();
    let (_, disclosure) = cx
        .pending_prompt()
        .expect("the actual Review consent prompt");
    assert!(disclosure.contains(input.workspace.join("AGENTS.md").to_str().unwrap()));
    assert!(!disclosure.contains(input.codex_home.join("AGENTS.md").to_str().unwrap()));
    assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);

    // Use the same HTTP fixture framing as the provider/Translation tests, but
    // retain the listener to detect extra connections after Review completes.
    let incoming = smol::Async::new(listener.try_clone().unwrap()).unwrap();
    let (request_sender, received) = std::sync::mpsc::sync_channel(1);
    let request_count = Arc::new(AtomicUsize::new(0));
    let count_for_server = request_count.clone();
    let server = std::thread::spawn(move || {
        let (stream, _) = smol::block_on(smol::future::or(incoming.accept(), async {
            smol::Timer::after(Duration::from_secs(10)).await;
            Err(std::io::Error::new(
                ErrorKind::TimedOut,
                "Review did not connect",
            ))
        }))
        .unwrap();
        count_for_server.fetch_add(1, Ordering::SeqCst);
        let mut stream = stream.into_inner().unwrap();
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut reader = BufReader::new(&stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut content_length = None;
        loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0);
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                content_length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0; content_length.expect("the provider sends a sized JSON body")];
        reader.read_exact(&mut body).unwrap();
        let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
        request_sender.send((request_line, request)).unwrap();
        drop(reader);
        let output = serde_json::json!({
            "schema_version": mt_core::review::REVIEW_SCHEMA_VERSION,
            "scope": {"kind": "document"},
            "understood_intent": {
                "stated_goal": "preserve the artifact",
                "relevant_context": [], "constraints": [], "non_goals": [],
                "expected_deliverable": "a read-only Review",
                "success_evidence": [], "inferred_assumptions": [], "unresolved_decisions": []
            },
            "findings": [], "clarification_questions": []
        })
        .to_string();
        let response = serde_json::json!({
            "id": "chatcmpl-review",
            "model": "review-response-model",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": output}, "finish_reason": "stop"}]
        }).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
            response.len()
        ).unwrap();
        stream.flush().unwrap();
    });
    assert_eq!(request_count.load(Ordering::SeqCst), 0);
    cx.simulate_prompt_answer("Send");
    cx.run_until_parked();
    let (request_line, wire) = received.recv_timeout(Duration::from_secs(10)).unwrap();
    completed.recv_timeout(Duration::from_secs(10)).unwrap();
    server.join().unwrap();

    assert_eq!(
        request_line.trim_end(),
        "POST /v1/chat/completions HTTP/1.1"
    );
    assert_eq!(request_count.load(Ordering::SeqCst), 1);
    assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    assert!(!cx.has_pending_prompt());
    let user_messages = wire["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .collect::<Vec<_>>();
    assert_eq!(user_messages.len(), 1);
    let payload_text = user_messages[0]["content"].as_str().unwrap();
    let payload: serde_json::Value = serde_json::from_str(payload_text).unwrap();
    assert_eq!(payload["operation"], "read_only_review");
    assert_eq!(payload["scope"], "document_with_effective_agent_context");
    assert_eq!(payload["frames"].as_array().unwrap().len(), 1);
    assert_eq!(payload["frames"][0]["content"], snapshot.text());
    assert_eq!(payload["frames"][0]["content_bytes"], snapshot.text().len());
    let context_wire = &payload["effective_agent_context"];
    let sent_context: SelectedContext = serde_json::from_value(context_wire.clone()).unwrap();
    assert_eq!(sent_context.sources().len(), 1);
    assert_eq!(sent_context.sources()[0].source_index(), 1);
    assert_eq!(sent_context.sources(), frozen.selected().sources());
    assert_eq!(sent_context.contents(), frozen.selected().contents());
    assert_eq!(sent_context.input(), &input);
    assert_eq!(
        sent_context.contents()[0].text,
        "Preserve source identity.\n"
    );
    assert!(context_wire.get("discovery_digest").is_none());
    assert!(context_wire.get("available_skills").is_none());
    assert!(!payload_text.contains("Name every outbound source."));
    assert!(!payload_text.contains(
        &serde_json::to_string(&input.codex_home.join("AGENTS.md").to_string_lossy()).unwrap()
    ));
    workspace.read_with(cx, |workspace, app| {
        let flow = &workspace.review_flow;
        let review = flow
            .review_result()
            .expect("the real provider response was validated");
        assert_eq!(review.document_id, document_id);
        assert_eq!(review.source_snapshot, snapshot);
        assert_eq!(review.result.result.status, ReviewStatus::Ready);
        assert_eq!(
            review.result.metadata.response_model(),
            "review-response-model"
        );
        assert!(review.supporting_sources_current);
        assert!(flow.review_diagnostic().is_none());
        assert!(!flow.is_reviewing());
        let revision = flow.revision_context().unwrap();
        assert_eq!(
            revision.request.effective_agent_context.as_ref(),
            Some(frozen.selected())
        );
        let document = workspace.active_document().unwrap().read(app);
        assert_eq!(document.id(), document_id);
        assert_eq!(document.async_snapshot(app), snapshot);
        assert_eq!(document.text(app), "# Artifact\n");
        assert!(!document.is_dirty());
    });
    assert_eq!(fs::read(&input.target).unwrap(), b"# Artifact\n");
}

#[gpui_kit::test]
fn effective_context_inventory_notification_does_not_restart_pending_resolution(
    cx: &mut gpui_kit::TestAppContext,
) {
    use mt_core::agent_artifacts::skill::{Origin, Skill, SkillMeta};
    let (_directory, input) = context_fixture();
    let skill_dir = input.workspace.join(".agents/skills/test-skill");
    let skills = vec![Skill {
        entry: skill_dir.join("SKILL.md"),
        dir: skill_dir,
        root: input.workspace.join(".agents/skills"),
        origin: Origin::Workspace,
        aliases: Vec::new(),
        name: "test-skill".to_owned(),
        meta: SkillMeta::default(),
        diagnostics: Vec::new(),
        support_dirs: Vec::new(),
    }];
    let (workspace, cx) = super::open_test_workspace(cx, input.target.clone());
    let harness = workspace.read_with(cx, |workspace, _| workspace.harness.clone().unwrap());
    harness.update(cx, |harness, cx| {
        harness.set_context_input_for_test(input.clone(), cx)
    });
    cx.run_until_parked();
    let generation = workspace.update(cx, |workspace, _| {
        workspace
            .review_flow
            .begin_context_resolution(Some(input.clone()))
    });
    harness.update(cx, |harness, cx| {
        harness.set_context_pending(true, cx);
        harness.apply(skills.clone(), Vec::new(), cx);
    });
    cx.run_until_parked();
    // Accept the same ticket: inventory delivery must not restart the pending job.
    let current = resolve(&input, &[]);
    workspace.update(cx, |workspace, cx| {
        assert!(
            workspace
                .review_flow
                .accept_context_resolution(generation, current.clone())
        );
        assert!(harness.update(cx, |harness, cx| {
            harness.apply_resolved_context(current, cx)
        }));
    });
    cx.run_until_parked();
    harness.read_with(cx, |harness, _| {
        assert!(!harness.context_is_pending());
        assert_eq!(harness.resolved_context().unwrap().available_skills, skills);
    });
    harness.update(cx, |harness, cx| harness.apply(Vec::new(), Vec::new(), cx));
    cx.run_until_parked();
    harness.read_with(cx, |harness, _| {
        assert!(!harness.context_is_pending());
        assert!(
            harness
                .resolved_context()
                .unwrap()
                .available_skills
                .is_empty()
        );
    });
}

#[test]
fn effective_context_model_change_accepts_pending_filesystem_resolution() {
    let (_directory, input) = context_fixture();
    for change_endpoint in [true, false] {
        let mut settings = crate::settings::AppSettings::default();
        let mut flow = ReviewFlow::default();
        flow.observe_model_configuration(&settings);
        let generation = flow.begin_context_resolution(Some(input.clone()));
        if change_endpoint {
            settings.model_base_url = "https://changed.example/v1".to_owned();
        } else {
            settings.model_name = "changed-model".to_owned();
        }
        flow.observe_model_configuration(&settings);
        assert!(flow.accept_context_resolution(generation, resolve(&input, &[])));
        assert!(flow.choose_context_source(DocumentId::next(), 1));
    }
}

#[gpui_kit::test]
fn kit_effective_context_inventory_preserves_current_workflow(cx: &mut gpui_kit::TestAppContext) {
    use mt_core::agent_artifacts::skill::{Origin, Skill, SkillMeta};
    use mt_core::model::Provider;
    use mt_core::review::provider::{ReviewMetadata, ReviewTransportResult};
    use mt_core::review::{ArtifactLens, ReviewResult, ReviewStatus};
    use mt_core::workspace::watcher::Change;
    use std::sync::atomic::Ordering;

    let (_directory, input) = context_fixture();
    let (workspace, cx) = super::open_test_workspace(cx, input.target.clone());
    let harness = workspace.read_with(cx, |workspace, _| workspace.harness.clone().unwrap());
    harness.update(cx, |harness, cx| {
        harness.apply(Vec::new(), Vec::new(), cx);
        harness.set_context_input_for_test(input.clone(), cx);
    });
    cx.run_until_parked();
    let (document, frozen, snapshot, request, answers, review_ticket, revision_ticket) = workspace
        .update(cx, |workspace, cx| {
            let active = workspace.active_document().unwrap().read(cx);
            let document = active.id();
            let snapshot = active.async_snapshot(cx);
            let mut context = revision_context(document, &input);
            context.source_snapshot = snapshot.clone();
            let request = context.request.clone();
            let answers = context.answer_states.clone();
            workspace.review_flow.install_review_result(
                super::super::review::WorkspaceReviewResult {
                    document_id: document,
                    source_snapshot: snapshot.clone(),
                    target: ReviewTarget::Document,
                    selection: None,
                    lens: ArtifactLens::Prompt,
                    partial: false,
                    skill_package: None,
                    supporting_sources_current: true,
                    result: ReviewTransportResult {
                        result: ReviewResult::ready(&request, context.review_output.clone())
                            .unwrap(),
                        metadata: ReviewMetadata::from_response(
                            Provider::OpenAiResponses,
                            "test-model",
                            "test-model",
                        )
                        .unwrap(),
                    },
                },
                false,
            );
            workspace
                .review_flow
                .open_review_panel(ReviewTarget::Document, document, true, false);
            assert!(workspace.review_flow.choose_context_source(document, 1));
            let frozen = workspace.review_flow.selected_context(document).unwrap();
            workspace
                .review_flow
                .install_review_context_for_test(frozen.selected().clone());
            workspace.review_flow.install_revision_context(context);
            let review_ticket = workspace
                .review_flow
                .begin_review_request(document, ArtifactLens::Prompt);
            let revision_ticket = workspace.review_flow.begin_revision_request(document);
            (
                document,
                frozen,
                snapshot,
                request,
                answers,
                review_ticket,
                revision_ticket,
            )
        });
    let skill_dir = input.workspace.join(".agents/skills/unrelated");
    let skill = Skill {
        entry: skill_dir.join("SKILL.md"),
        dir: skill_dir,
        root: input.workspace.join(".agents/skills"),
        origin: Origin::Workspace,
        aliases: Vec::new(),
        name: "unrelated".to_owned(),
        meta: SkillMeta::default(),
        diagnostics: Vec::new(),
        support_dirs: Vec::new(),
    };
    for skills in [vec![skill], Vec::new()] {
        harness.update(cx, |harness, cx| {
            harness.apply(skills.clone(), Vec::new(), cx)
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, app| {
            let flow = &workspace.review_flow;
            assert!(flow.context_request_is_current(document, &frozen));
            assert_eq!(
                flow.selected_context(document).unwrap().selected(),
                frozen.selected()
            );
            let review = flow.review_result().unwrap();
            assert!(review.supporting_sources_current);
            assert_eq!(review.result.result.status, ReviewStatus::Ready);
            assert!(flow.revision_context_is_current(Some(document), Some(&snapshot), true));
            let revision = flow.revision_context().unwrap();
            assert_eq!(revision.request, request);
            assert_eq!(revision.answer_states, answers);
            assert!(flow.review_request_is_current(review_ticket.0, &review_ticket.1));
            assert!(flow.revision_request_is_current(revision_ticket.0, &revision_ticket.1));
            assert!(!review_ticket.1.load(Ordering::Acquire));
            assert!(!revision_ticket.1.load(Ordering::Acquire));
            let harness = harness.read(app);
            assert!(!harness.context_is_pending());
            assert_eq!(harness.resolved_context().unwrap().available_skills, skills);
        });
    }
    workspace.update(cx, |workspace, cx| {
        workspace.apply_watcher_changes(
            &input.workspace,
            &[Change::Modified(input.workspace.join("AGENTS.md"))],
            cx,
        );
        let flow = &workspace.review_flow;
        assert!(!flow.context_request_is_current(document, &frozen));
        assert!(flow.selected_context(document).is_none());
        assert!(!flow.review_result().unwrap().supporting_sources_current);
        assert_eq!(
            flow.review_result().unwrap().result.result.status,
            ReviewStatus::Stale
        );
        assert!(!flow.revision_context_is_current(Some(document), Some(&snapshot), true));
        assert!(review_ticket.1.load(Ordering::Acquire));
        assert!(revision_ticket.1.load(Ordering::Acquire));
    });
    cx.run_until_parked();
}

#[test]
fn effective_context_choices_are_unchecked_document_scoped_and_explicit() {
    let (_directory, input) = context_fixture();
    let mut flow = ReviewFlow::default();
    install_context(&mut flow, &input);
    let first = DocumentId::next();
    let second = DocumentId::next();
    flow.open_review_panel(ReviewTarget::Document, first, false, false);
    assert!(flow.selected_context(first).is_none());
    assert!(!flow.is_reviewing());
    assert!(flow.choose_context_source(first, 1));
    let frozen = flow.selected_context(first).unwrap();
    assert_eq!(frozen.selected().sources().len(), 1);
    assert_eq!(frozen.selected().sources()[0].source_index(), 1);
    assert_eq!(
        frozen.selected().sources()[0].occurrence().path,
        input.workspace.join("AGENTS.md")
    );
    assert!(flow.selected_context(second).is_none());
    assert!(flow.choose_context_source(first, 1));
    assert!(flow.selected_context(first).is_none());
    assert_eq!(frozen.selected().sources().len(), 1);
    assert!(!flow.choose_context_source(first, 99));
}

#[gpui_kit::test]
fn kit_effective_context_watcher_matches_candidate_identity(cx: &mut gpui_kit::TestAppContext) {
    use mt_core::workspace::watcher::Change;
    let (_directory, mut input) = context_fixture();
    fs::remove_file(input.workspace.join("AGENTS.md")).unwrap();
    #[cfg(windows)]
    let candidate = input.workspace.join("TEAM.md");
    #[cfg(not(windows))]
    let candidate = input.workspace.join("team.md");
    fs::write(&candidate, "Original team instructions.\n").unwrap();
    input.fallback_filenames = vec!["team.md".to_owned()];
    #[cfg(windows)]
    let event = {
        let canonical = candidate.to_str().unwrap();
        std::path::PathBuf::from(
            canonical
                .strip_prefix(r"\\?\")
                .unwrap()
                .to_ascii_uppercase(),
        )
    };
    #[cfg(not(windows))]
    let event = candidate.clone();
    let (workspace, cx) = super::open_test_workspace(cx, input.target.clone());
    let harness = workspace.read_with(cx, |workspace, _| workspace.harness.clone().unwrap());
    harness.update(cx, |harness, cx| {
        harness.set_context_input_for_test(input.clone(), cx)
    });
    cx.run_until_parked();
    let (document, frozen) = workspace.update(cx, |workspace, cx| {
        let document = workspace.active_document().unwrap().read(cx).id();
        assert!(workspace.review_flow.choose_context_source(document, 1));
        let frozen = workspace.review_flow.selected_context(document).unwrap();
        assert_eq!(
            frozen.selected().sources()[0].occurrence().path,
            input.workspace.join("team.md")
        );
        #[cfg(windows)]
        assert!(
            harness
                .read(cx)
                .resolved_context()
                .unwrap()
                .watch_paths()
                .iter()
                .all(|path| path != &event)
        );
        (document, frozen)
    });
    let unrelated = vec![
        Change::Modified(event.with_extension("md.bak")),
        Change::Removed(std::path::PathBuf::from(format!(
            "{}-sibling",
            event.parent().unwrap().display()
        ))),
        Change::Removed(event.with_extension("md.bak")),
    ];
    #[cfg(not(windows))]
    let unrelated = {
        let mut unrelated = unrelated;
        unrelated.extend([
            Change::Modified(input.workspace.join("TEAM.md")),
            Change::Removed(input.workspace.join("TEAM.md")),
        ]);
        unrelated
    };
    workspace.update(cx, |workspace, cx| {
        workspace.apply_watcher_changes(&input.workspace, &unrelated, cx);
        assert!(
            workspace
                .review_flow
                .context_request_is_current(document, &frozen)
        );
        assert!(!harness.read(cx).context_is_pending());
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert!(
            workspace
                .review_flow
                .context_request_is_current(document, &frozen)
        );
    });

    fs::write(&candidate, "Updated team instructions.\n").unwrap();
    workspace.update(cx, |workspace, cx| {
        workspace.apply_watcher_changes(&input.workspace, &[Change::Modified(event.clone())], cx);
        assert!(
            !workspace
                .review_flow
                .context_request_is_current(document, &frozen)
        );
        assert!(workspace.review_flow.selected_context(document).is_none());
        assert!(harness.read(cx).context_is_pending());
    });
    cx.run_until_parked();
    let updated = workspace.update(cx, |workspace, cx| {
        assert!(!harness.read(cx).context_is_pending());
        assert!(workspace.review_flow.choose_context_source(document, 1));
        let selected = workspace.review_flow.selected_context(document).unwrap();
        assert_eq!(
            selected.selected().contents()[0].text,
            "Updated team instructions.\n"
        );
        selected
    });

    fs::remove_file(&candidate).unwrap();
    assert!(!event.exists());
    workspace.update(cx, |workspace, cx| {
        workspace.apply_watcher_changes(&input.workspace, &[Change::Removed(event)], cx);
        assert!(
            !workspace
                .review_flow
                .context_request_is_current(document, &updated)
        );
        assert!(workspace.review_flow.selected_context(document).is_none());
        assert!(harness.read(cx).context_is_pending());
    });
    cx.run_until_parked();
    harness.read_with(cx, |harness, _| {
        assert!(!harness.context_is_pending());
        let context = harness.resolved_context().unwrap();
        assert_eq!(context.sources.len(), 1);
        assert_eq!(context.sources[0].path, input.codex_home.join("AGENTS.md"));
        assert!(
            context
                .contents
                .iter()
                .all(|content| !content.text.contains("team instructions"))
        );
    });
}

#[test]
fn effective_context_resolution_generation_rejects_a_to_b_to_a_completion() {
    let (_directory, input) = context_fixture();
    let mut flow = ReviewFlow::default();
    let first = flow.begin_context_resolution(Some(input.clone()));
    let mut changed = input.clone();
    changed.project_doc_max_bytes = 1;
    flow.begin_context_resolution(Some(changed));
    let latest = flow.begin_context_resolution(Some(input.clone()));
    assert!(!flow.accept_context_resolution(first, resolve(&input, &[])));
    assert!(flow.accept_context_resolution(latest, resolve(&input, &[])));
}

#[test]
fn effective_context_new_resolution_invalidates_identical_old_selection() {
    let (_directory, input) = context_fixture();
    let mut flow = ReviewFlow::default();
    install_context(&mut flow, &input);
    let document = DocumentId::next();
    assert!(flow.choose_context_source(document, 0));
    let frozen = flow.selected_context(document).unwrap();
    assert!(flow.context_request_is_current(document, &frozen));
    install_context(&mut flow, &input);
    assert!(!flow.context_request_is_current(document, &frozen));
    assert!(flow.selected_context(document).is_none());
    assert!(flow.choose_context_source(document, 0));
    assert!(!flow.context_request_is_current(document, &frozen));
}

#[test]
fn effective_context_input_change_cancels_consent_ticket_before_authorization() {
    let (_directory, input) = context_fixture();
    let mut flow = ReviewFlow::default();
    install_context(&mut flow, &input);
    let document = DocumentId::next();
    flow.begin_review_attempt(ReviewTarget::Document, document);
    assert!(flow.choose_context_source(document, 0));
    let (generation, cancelled) =
        flow.begin_review_request(document, mt_core::review::ArtifactLens::Prompt);
    assert!(flow.review_request_is_current(generation, &cancelled));
    flow.begin_context_resolution(None);
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
    assert!(!flow.review_request_is_current(generation, &cancelled));
}

#[test]
fn effective_context_dirty_open_source_invalidates_other_document_selection() {
    let (_directory, input) = context_fixture();
    let mut flow = ReviewFlow::default();
    install_context(&mut flow, &input);
    let reviewed = DocumentId::next();
    assert!(flow.choose_context_source(reviewed, 0));
    let frozen = flow.selected_context(reviewed).unwrap();
    assert!(flow.observe_document_change(
        DocumentId::next(),
        &AsyncSnapshot::new(1, "Changed locally".to_owned(), 1),
        Some(&input.codex_home.join("AGENTS.md")),
        true
    ));
    assert!(!flow.context_request_is_current(reviewed, &frozen));
    assert!(flow.selected_context(reviewed).is_none());
}

#[test]
fn effective_context_filesystem_revalidation_rejects_new_override_and_keeps_frozen_content() {
    let (_directory, input) = context_fixture();
    let mut flow = ReviewFlow::default();
    install_context(&mut flow, &input);
    let document = DocumentId::next();
    assert!(flow.choose_context_source(document, 1));
    let frozen = flow.selected_context(document).unwrap();
    let original = frozen.selected().contents().to_vec();
    assert!(frozen.selected().revalidate().is_ok());
    fs::write(input.workspace.join("AGENTS.override.md"), "New precedence").unwrap();
    assert!(frozen.selected().revalidate().is_err());
    assert_eq!(frozen.selected().contents(), original);
}

fn revision_context(
    document: DocumentId,
    input: &ContextInput,
) -> super::super::review::WorkspaceRevisionContext {
    use mt_core::review::{
        ArtifactLens, ClarificationPriority, ClarificationQuestion, ReviewModelOutput,
        ReviewRequest, ReviewSections, SourceSnapshot,
    };
    let selected = resolve(input, &[]).select_sources(&[1]).unwrap();
    super::super::review::WorkspaceRevisionContext {
        document_id: document,
        source_snapshot: AsyncSnapshot::new(1, "Artifact".to_owned(), 1),
        request: ReviewRequest::document(
            ArtifactLens::Prompt,
            "Artifact",
            SourceSnapshot::new(1, 1),
        )
        .unwrap()
        .with_effective_agent_context(selected)
        .unwrap(),
        review_output: ReviewModelOutput {
            schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
            scope: mt_core::review::ReviewScope::Document,
            understood_intent: ReviewSections {
                stated_goal: "Explain the artifact".into(),
                relevant_context: Vec::new(),
                constraints: Vec::new(),
                non_goals: Vec::new(),
                expected_deliverable: "An explicit artifact".into(),
                success_evidence: Vec::new(),
                inferred_assumptions: Vec::new(),
                unresolved_decisions: Vec::new(),
            },
            findings: Vec::new(),
            clarification_questions: vec![ClarificationQuestion::new(
                "What should be explicit?",
                ClarificationPriority::High,
            )],
        },
        skill_package: None,
        supporting_sources_current: true,
        applied: None,
        answers_exported: false,
        answer_states: vec![mt_core::review::provider::RevisionAnswer::Answered(
            "Source identity".to_owned(),
        )],
        answer_inputs: Vec::new(),
    }
}

#[test]
fn effective_context_model_change_cancels_requests_and_retains_stale_evidence() {
    use mt_core::model::Provider;
    use mt_core::review::provider::{ReviewMetadata, ReviewTransportResult};
    use mt_core::review::{ArtifactLens, ReviewResult, ReviewStatus};
    use std::sync::atomic::Ordering;

    let (_directory, input) = context_fixture();
    let mut settings = crate::settings::AppSettings::default();
    let mut flow = ReviewFlow::default();
    flow.observe_model_configuration(&settings);
    install_context(&mut flow, &input);
    let document = DocumentId::next();
    let context = revision_context(document, &input);
    let request = context.request.clone();
    let output = context.review_output.clone();
    let answers = context.answer_states.clone();
    let snapshot = context.source_snapshot.clone();
    flow.install_review_result(
        super::super::review::WorkspaceReviewResult {
            document_id: document,
            source_snapshot: snapshot.clone(),
            target: ReviewTarget::Document,
            selection: None,
            lens: ArtifactLens::Prompt,
            partial: false,
            skill_package: None,
            supporting_sources_current: true,
            result: ReviewTransportResult {
                result: ReviewResult::ready(&request, output.clone()).unwrap(),
                metadata: ReviewMetadata::from_response(
                    Provider::OpenAiResponses,
                    "test-model",
                    "test-model",
                )
                .unwrap(),
            },
        },
        false,
    );
    flow.open_review_panel(ReviewTarget::Document, document, true, false);
    assert!(flow.choose_context_source(document, 1));
    let frozen = flow.selected_context(document).unwrap();
    flow.install_revision_context(context);
    assert!(flow.review_result().unwrap().supporting_sources_current);
    assert!(flow.revision_context_is_current(Some(document), Some(&snapshot), true));
    let (review_generation, review_cancelled) =
        flow.begin_review_request(document, ArtifactLens::Prompt);
    let (revision_generation, revision_cancelled) = flow.begin_revision_request(document);
    assert!(flow.review_request_is_current(review_generation, &review_cancelled));
    assert!(flow.revision_request_is_current(revision_generation, &revision_cancelled));

    settings.model_name = "changed-model".to_owned();
    flow.observe_model_configuration(&settings);

    assert!(flow.selected_context(document).is_none());
    assert!(!flow.context_request_is_current(document, &frozen));
    assert!(review_cancelled.load(Ordering::Acquire));
    assert!(revision_cancelled.load(Ordering::Acquire));
    assert!(!flow.review_request_is_current(review_generation, &review_cancelled));
    assert!(!flow.revision_request_is_current(revision_generation, &revision_cancelled));
    let retained_review = flow.review_result().unwrap();
    assert!(!retained_review.supporting_sources_current);
    assert_eq!(retained_review.result.result.status, ReviewStatus::Stale);
    assert_eq!(retained_review.result.result.output.as_ref(), Some(&output));
    assert!(!flow.revision_context_is_current(Some(document), Some(&snapshot), true));
    let retained_revision = flow.revision_context().unwrap();
    assert!(!retained_revision.supporting_sources_current);
    assert_eq!(retained_revision.request, request);
    assert_eq!(retained_revision.answer_states, answers);
}

#[test]
fn effective_context_supporting_edit_keeps_revision_request_and_answers_as_stale_evidence() {
    let (_directory, input) = context_fixture();
    let mut flow = ReviewFlow::default();
    install_context(&mut flow, &input);
    let document = DocumentId::next();
    let context = revision_context(document, &input);
    let request = context.request.clone();
    let answers = context.answer_states.clone();
    let snapshot = context.source_snapshot.clone();
    flow.install_revision_context(context);
    assert!(flow.revision_context_is_current(Some(document), Some(&snapshot), true));
    flow.observe_document_change(
        DocumentId::next(),
        &AsyncSnapshot::new(2, "Changed".to_owned(), 1),
        Some(&input.workspace.join("AGENTS.md")),
        true,
    );
    assert!(!flow.revision_context_is_current(Some(document), Some(&snapshot), true));
    let retained = flow.revision_context().unwrap();
    assert_eq!(retained.request, request);
    assert_eq!(retained.answer_states, answers);
    assert!(!retained.supporting_sources_current);
}

#[test]
fn effective_context_original_editable_source_uses_snapshot_safety_not_supporting_source_rule() {
    let (_directory, input) = context_fixture();
    let mut flow = ReviewFlow::default();
    install_context(&mut flow, &input);
    let document = DocumentId::next();
    flow.install_revision_context(revision_context(document, &input));
    let changed = AsyncSnapshot::new(2, "Changed".to_owned(), 1);
    flow.observe_document_change(
        document,
        &changed,
        Some(&input.workspace.join("AGENTS.md")),
        true,
    );
    assert!(flow.revision_context().unwrap().supporting_sources_current);
    assert!(!flow.revision_context_is_current(Some(document), Some(&changed), true));
}

#[gpui_kit::test]
fn kit_effective_context_controls_fit_review_pane_and_keep_source_identity(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    let (_directory, mut input) = context_fixture();
    let long_workspace = input
        .workspace
        .join("effective-context-source-with-a-long-directory-name")
        .join("another-long-directory-component-for-pane-regression");
    fs::create_dir_all(long_workspace.join(".git")).unwrap();
    fs::write(long_workspace.join("document.md"), "# Artifact\n").unwrap();
    fs::write(
        long_workspace.join("AGENTS.md"),
        "Preserve source identity.\n",
    )
    .unwrap();
    input.workspace = fs::canonicalize(long_workspace).unwrap();
    input.cwd = input.workspace.clone();
    input.target = input.workspace.join("document.md");
    let source_path = input.workspace.join("AGENTS.md");
    let source_path = source_path.display().to_string();
    assert!(source_path.len() > 100);
    let (workspace, cx) = super::open_test_workspace(cx, input.target.clone());
    workspace.update(cx, |workspace, cx| {
        install_context(&mut workspace.review_flow, &input);
        workspace.open_review_panel(ReviewTarget::Document, cx);
    });
    let (document_id, target) = workspace.read_with(cx, |workspace, app| {
        assert!(!workspace.review_flow.is_reviewing());
        (
            workspace.active_document().unwrap().read(app).id(),
            workspace
                .harness
                .as_ref()
                .unwrap()
                .read(app)
                .selected_target(app),
        )
    });
    cx.update(|window, app| {
        window.render_frame(app);
        let panel = window.find("review-panel").bounds();
        let source = window.find("review-context-source-1");
        let open = window.find("review-context-open-1");
        assert!(source.visible());
        assert!(open.visible());
        assert_eq!(
            source.label(),
            Some(
                format!(
                    "{}: {source_path}",
                    i18n::effective_context_choice_label(false, app)
                )
                .as_str()
            )
        );
        for control in [source.bounds(), open.bounds()] {
            assert!(control.origin.x >= panel.origin.x);
            assert!(control.origin.y >= panel.origin.y);
            assert!(control.origin.x + control.size.width <= panel.origin.x + panel.size.width);
            assert!(control.origin.y + control.size.height <= panel.origin.y + panel.size.height);
        }
        window.click("review-context-source-1", app);
        window.render_frame(app);
        let source = window.find("review-context-source-1");
        assert_eq!(
            source.label(),
            Some(
                format!(
                    "{}: {source_path}",
                    i18n::effective_context_choice_label(true, app)
                )
                .as_str()
            )
        );
        let panel = window.find("review-panel").bounds();
        let bounds = source.bounds();
        assert!(bounds.origin.x >= panel.origin.x);
        assert!(bounds.origin.y >= panel.origin.y);
        assert!(bounds.origin.x + bounds.size.width <= panel.origin.x + panel.size.width);
        assert!(bounds.origin.y + bounds.size.height <= panel.origin.y + panel.size.height);
    });
    workspace.read_with(cx, |workspace, _| {
        assert!(
            workspace
                .review_flow
                .selected_context(document_id)
                .is_some()
        );
        assert!(!workspace.review_flow.is_reviewing());
    });
    cx.update(|window, app| {
        window.render_frame(app);
        window.click("review-context-open-1", app);
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, app| {
        assert_eq!(
            workspace
                .harness
                .as_ref()
                .unwrap()
                .read(app)
                .selected_target(app),
            target
        );
        let active = workspace.active_document().unwrap().read(app);
        assert_ne!(active.id(), document_id);
        assert!(
            workspace
                .review_flow
                .selected_context(active.id())
                .is_none()
        );
        assert!(!workspace.review_flow.is_reviewing());
    });
}

#[gpui_kit::test]
fn kit_effective_context_new_ancestor_marker_refreshes_chain_and_clears_selection(
    cx: &mut gpui_kit::TestAppContext,
) {
    use mt_core::workspace::watcher::Change;

    let (_directory, mut input) = context_fixture();
    fs::remove_dir(input.workspace.join(".git")).unwrap();
    input.cwd = input.workspace.join("nested");
    fs::create_dir(&input.cwd).unwrap();
    fs::write(input.cwd.join("AGENTS.md"), "Nested instructions.\n").unwrap();
    let (workspace, cx) = super::open_test_workspace(cx, input.target.clone());
    let harness = workspace.read_with(cx, |workspace, _| workspace.harness.clone().unwrap());
    harness.update(cx, |harness, cx| {
        harness.set_context_input_for_test(input.clone(), cx)
    });
    cx.run_until_parked();
    let document_id = workspace.update(cx, |workspace, cx| {
        let document_id = workspace.active_document().unwrap().read(cx).id();
        assert_eq!(
            harness.read(cx).resolved_context().unwrap().sources.len(),
            2
        );
        assert!(workspace.review_flow.choose_context_source(document_id, 1));
        assert!(
            workspace
                .review_flow
                .selected_context(document_id)
                .is_some()
        );
        document_id
    });
    let marker = input.workspace.join(".git");
    fs::create_dir(&marker).unwrap();

    cx.update(|_, app| {
        workspace.update(app, |workspace, cx| {
            workspace.apply_watcher_changes(
                &input.workspace,
                &[Change::Created(marker.clone())],
                cx,
            );
        });
    });
    cx.run_until_parked();

    workspace.read_with(cx, |workspace, app| {
        let context = harness.read(app).resolved_context().unwrap();
        assert_eq!(context.project_root.as_ref(), Some(&input.workspace));
        assert_eq!(context.sources.len(), 3);
        assert_eq!(context.sources[1].path, input.workspace.join("AGENTS.md"));
        assert_eq!(context.sources[2].path, input.cwd.join("AGENTS.md"));
        assert!(
            workspace
                .review_flow
                .selected_context(document_id)
                .is_none()
        );
    });
}
