use super::super::review::ReviewFlow;
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

#[test]
fn effective_context_inventory_notification_does_not_restart_pending_resolution() {
    use mt_core::agent_artifacts::skill::{Origin, Skill, SkillMeta};
    let (_directory, input) = context_fixture();
    let previous = resolve(&input, &[]);
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
    assert!(super::super::context_inventory_needs_refresh(
        Some(&previous),
        &skills,
        false,
    ));
    let mut flow = ReviewFlow::default();
    let generation = flow.begin_context_resolution(Some(input.clone()));
    // set_context_pending notifies with the previous Harness result still installed.
    if super::super::context_inventory_needs_refresh(Some(&previous), &skills, true) {
        flow.begin_context_resolution(Some(input.clone()));
    }
    let current = resolve(&input, &skills);
    assert!(flow.accept_context_resolution(generation, current.clone()));
    assert!(!super::super::context_inventory_needs_refresh(
        Some(&current),
        &skills,
        false,
    ));
    // An inventory change during the job must still trigger a refresh after it lands.
    assert!(super::super::context_inventory_needs_refresh(
        Some(&current),
        &[],
        false,
    ));
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

// Compiled with the app tests; execution is reserved for the integrated GUI milestone.
#[gpui_kit::test]
fn kit_effective_context_controls_select_without_sending_and_navigation_does_not_retarget(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    let (_directory, input) = context_fixture();
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
        window.click("review-context-source-1", app);
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
