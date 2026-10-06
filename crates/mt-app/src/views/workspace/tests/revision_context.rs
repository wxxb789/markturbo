use gpui_kit::{ClipboardItem, Entity, TestAppContext, VisualTestContext};
use mt_core::agent_artifacts::context::{CODEX_PROFILE_ID, ContextInput, ProjectTrust, resolve};
use mt_core::agent_artifacts::package::ReviewTarget;
use mt_core::model::Provider;
use mt_core::review::provider::{
    REVISION_SCHEMA_VERSION, ReviewMetadata, ReviewTransportResult, RevisionAnswers,
    decode_revision_capture,
};
use mt_core::review::revision::ChangeId;
use mt_core::review::{
    ArtifactLens, ReviewModelOutput, ReviewResult, ReviewSections, SourceSnapshot,
};
use std::fs;
use std::path::PathBuf;

use super::super::Workspace;
use super::super::review::{
    WorkspaceReviewResult, WorkspaceRevisionContext, WorkspaceRevisionResult,
};
use super::build_document_review_request;

fn context_revision_fixture(
    cx: &mut TestAppContext,
    self_source: bool,
) -> (
    tempfile::TempDir,
    Entity<Workspace>,
    &mut VisualTestContext,
    PathBuf,
) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let source = root.join(if self_source {
        "AGENTS.md"
    } else {
        "document.md"
    });
    let instructions = root.join("AGENTS.md");
    fs::create_dir(root.join(".git")).unwrap();
    fs::create_dir(root.join("home")).unwrap();
    fs::write(&source, "old\n").unwrap();
    fs::write(
        &instructions,
        if self_source {
            "old\n"
        } else {
            "Preserve the original document.\n"
        },
    )
    .unwrap();
    let input = ContextInput {
        workspace: root.clone(),
        target: source.clone(),
        cwd: root.clone(),
        profile_id: CODEX_PROFILE_ID.to_owned(),
        codex_home: root.join("home"),
        codex_home_override: true,
        fallback_filenames: Vec::new(),
        project_doc_max_bytes: 32768,
        project_root_markers: vec![".git".to_owned()],
        project_trust: ProjectTrust::Unspecified,
    };
    let selected = resolve(&input, &[]).select_sources(&[0]).unwrap();
    let (workspace, cx) = super::open_test_workspace(cx, source.clone());
    let harness = workspace.read_with(cx, |workspace, _| workspace.harness.clone().unwrap());
    harness.update(cx, |harness, cx| {
        harness.set_context_input_for_test(input.clone(), cx)
    });
    cx.run_until_parked();
    let (document_id, snapshot, revision) = workspace.read_with(cx, |workspace, app| {
        let document = workspace.active_document().unwrap().read(app);
        (
            document.id(),
            document.async_snapshot(app),
            document.revision(),
        )
    });
    let request = build_document_review_request(
        ReviewTarget::Document,
        ArtifactLens::Prompt,
        Some(&source),
        "old\n",
        None,
        SourceSnapshot::new(revision, snapshot.source_generation()),
    )
    .unwrap()
    .with_effective_agent_context(selected)
    .unwrap();
    let output = ReviewModelOutput {
        schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
        scope: request.scope,
        understood_intent: ReviewSections {
            stated_goal: "update the text".into(),
            relevant_context: Vec::new(),
            constraints: Vec::new(),
            non_goals: Vec::new(),
            expected_deliverable: "updated text".into(),
            success_evidence: Vec::new(),
            inferred_assumptions: Vec::new(),
            unresolved_decisions: Vec::new(),
        },
        findings: Vec::new(),
        clarification_questions: Vec::new(),
    };
    let raw = serde_json::json!({
        "schema_version": REVISION_SCHEMA_VERSION,
        "groups": [{
            "rationale": "Apply the requested wording",
            "edits": [{
                "range": {"start": 0, "end": 3},
                "expected_source": "old",
                "replacement": "new"
            }]
        }],
        "question_coverage": []
    })
    .to_string();
    let transport = decode_revision_capture(
        &request,
        &output,
        &RevisionAnswers::new(Vec::new()).unwrap(),
        &raw,
    )
    .unwrap()
    .into_transport_result_for_test();
    workspace.update(cx, |workspace, _| {
        // The guards must work even when filesystem observation is unavailable.
        workspace.watcher = None;
        workspace.right_panel_open = true;
        workspace.review_flow.set_review_panel_open(true);
        workspace
            .review_flow
            .install_review_context_for_test(request.effective_agent_context.clone().unwrap());
        workspace.review_flow.install_review_result(
            WorkspaceReviewResult {
                document_id,
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
        workspace
            .review_flow
            .install_revision_context(WorkspaceRevisionContext {
                document_id,
                source_snapshot: snapshot.clone(),
                request,
                review_output: output,
                skill_package: None,
                supporting_sources_current: true,
                applied: None,
                answers_exported: false,
                answer_states: Vec::new(),
                answer_inputs: Vec::new(),
            });
        workspace
            .review_flow
            .replace_revision_result(WorkspaceRevisionResult {
                document_id,
                source_snapshot: snapshot,
                result: transport,
                decisions: vec![(ChangeId(0), true)],
                preview: "new\n".to_owned(),
            });
    });
    (directory, workspace, cx, instructions)
}

#[cfg(windows)]
#[gpui_kit::test]
fn kit_revision_repaint_uses_cached_effective_context(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    use std::os::windows::fs::OpenOptionsExt as _;

    let (_directory, workspace, cx, instructions) = context_revision_fixture(cx, false);
    let _locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(instructions)
        .unwrap();

    cx.update(|window, app| {
        window.render_frame(app);
        window.click("revision-reject-all", app);
    });
    cx.run_until_parked();

    workspace.read_with(cx, |workspace, _| {
        let revision = workspace.review_flow.revision_result().unwrap();
        assert_eq!(revision.decisions, vec![(ChangeId(0), false)]);
        assert_eq!(revision.preview, "old\n");
    });
}

#[gpui_kit::test]
fn changed_effective_context_blocks_revision_export_without_watcher(cx: &mut TestAppContext) {
    let (_directory, workspace, cx, instructions) = context_revision_fixture(cx, false);
    cx.write_to_clipboard(ClipboardItem::new_string("sentinel".to_owned()));
    fs::write(instructions, "Changed supporting instructions.\n").unwrap();

    workspace.update(cx, |workspace, cx| workspace.copy_revision(cx));

    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("sentinel".to_owned())
    );
    workspace.read_with(cx, |workspace, app| {
        assert_eq!(
            workspace.active_document().unwrap().read(app).text(app),
            "old\n"
        );
        assert!(workspace.review_flow.revision_diagnostic().is_some());
    });
}

#[gpui_kit::test]
fn kit_effective_context_dirty_alias_invalidates_cached_revision_without_watcher(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;

    let (_directory, workspace, cx, instructions) = context_revision_fixture(cx, false);
    let (request, snapshot) = workspace.read_with(cx, |workspace, _| {
        let context = workspace.review_flow.revision_context().unwrap();
        (context.request.clone(), context.source_snapshot.clone())
    });
    let alias = instructions.parent().unwrap().join("home/../AGENTS.md");
    cx.update(|window, app| {
        workspace.update(app, |workspace, cx| {
            assert!(workspace.open_file(alias.clone(), window, cx));
        });
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, app| {
        assert_eq!(
            workspace.document_at(1).unwrap().read(app).source_path(),
            Some(alias.as_path())
        );
        assert!(
            workspace
                .review_flow
                .review_result()
                .unwrap()
                .supporting_sources_current
        );
        assert!(
            workspace
                .review_flow
                .revision_context()
                .unwrap()
                .supporting_sources_current
        );
    });
    super::replace_document(&workspace, 1, "Unsaved supporting instructions.\n", cx);
    workspace.read_with(cx, |workspace, app| {
        assert!(
            !workspace
                .review_flow
                .review_result()
                .unwrap()
                .supporting_sources_current
        );
        let context = workspace.review_flow.revision_context().unwrap();
        assert!(!context.supporting_sources_current);
        assert_eq!(context.request, request);
        assert_eq!(context.source_snapshot, snapshot);
        assert_eq!(
            workspace.document_at(0).unwrap().read(app).text(app),
            "old\n"
        );
        assert_eq!(
            workspace.review_flow.revision_result().unwrap().decisions,
            vec![(ChangeId(0), true)]
        );
    });
    assert_eq!(
        fs::read_to_string(&instructions).unwrap(),
        "Preserve the original document.\n"
    );
    let original = workspace.read_with(cx, |workspace, app| {
        workspace
            .document_at(0)
            .unwrap()
            .read(app)
            .source_path()
            .unwrap()
            .to_path_buf()
    });
    cx.update(|window, app| {
        workspace.update(app, |workspace, cx| {
            assert!(workspace.open_file(original, window, cx));
        });
        window.render_frame(app);
        window.click("revision-reject-all", app);
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(
            workspace.review_flow.revision_result().unwrap().decisions,
            vec![(ChangeId(0), true)]
        );
    });
}

#[gpui_kit::test]
fn kit_effective_context_self_source_edit_uses_original_snapshot_safety(cx: &mut TestAppContext) {
    let (_directory, workspace, cx, _instructions) = context_revision_fixture(cx, true);
    cx.write_to_clipboard(ClipboardItem::new_string("sentinel".to_owned()));
    super::replace_document(&workspace, 0, "Edited original artifact.\n", cx);
    workspace.read_with(cx, |workspace, _| {
        assert!(
            !workspace
                .review_flow
                .review_result()
                .unwrap()
                .supporting_sources_current
        );
        assert_eq!(
            workspace
                .review_flow
                .review_result()
                .unwrap()
                .result
                .result
                .status,
            mt_core::review::ReviewStatus::Stale
        );
        assert!(
            workspace
                .review_flow
                .revision_context()
                .unwrap()
                .supporting_sources_current
        );
        assert_eq!(
            workspace.review_flow.revision_result().unwrap().decisions,
            vec![(ChangeId(0), true)]
        );
    });
    workspace.update(cx, |workspace, cx| workspace.copy_revision(cx));
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("sentinel".to_owned())
    );
}
