use std::sync::Arc;

use mt_core::agent_artifacts::context::{
    CODEX_PROFILE_ID, ContextContent, ContextInput, ContextProfile, InclusionRule, ProjectTrust,
    ResolutionStatus, ResolvedContext, SourceOccurrence,
};
use mt_core::agent_artifacts::skill::Origin;
use mt_core::credentials::{CredentialError, CredentialVault, Secret, SecureCredentialStore};
use mt_core::model::{
    ConsentCapability, ConsentDecision, ConsentError, EndpointIdentity, EndpointLocation,
    ModelOperation, OutboundScopeKind, Provider,
};
use mt_core::recovery::RevisionRecovery;
use mt_core::review::provider::{
    PreparedReview, REVIEW_MAX_FILE_BYTES, ReviewError, ReviewLanguage, ReviewRequestError,
    RevisionAnswer, RevisionAnswers, decode_revision_capture, inspect_document_request,
    revision_request_binding,
};
use mt_core::review::{
    ArtifactLens, ByteRange, ReviewModelOutput, ReviewRequest, ReviewScope, ReviewSections,
    ReviewSource, ReviewValidationError, SkillPackage, SkillPackageFile, SkillPackageOmission,
    SourceAnchor, SourceLocation, SourceSnapshot,
};
use mt_core::settings::SettingsData;
use sha2::{Digest as _, Sha256};

struct EmptyStore;

impl SecureCredentialStore for EmptyStore {
    fn is_supported(&self) -> bool {
        true
    }

    fn read(&self, _: &str) -> Result<Option<Secret>, CredentialError> {
        Ok(None)
    }

    fn write(&self, _: &str, _: &Secret) -> Result<(), CredentialError> {
        Ok(())
    }

    fn delete(&self, _: &str) -> Result<(), CredentialError> {
        Ok(())
    }
}

fn resolved() -> ResolvedContext {
    let base = std::env::current_dir().unwrap();
    let workspace = base.join("context-review-workspace");
    let cwd = workspace.join("nested");
    let codex_home = base.join("context-review-home");
    let texts = [
        "selected-global-sentinel",
        "unselected-root-sentinel",
        "selected-project-sentinel",
    ];
    let sources = [
        (
            codex_home.clone(),
            Origin::Global,
            InclusionRule::GlobalAgents,
        ),
        (
            workspace.clone(),
            Origin::Workspace,
            InclusionRule::ProjectAgents,
        ),
        (cwd.clone(), Origin::Workspace, InclusionRule::ProjectAgents),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (scope, origin, rule))| SourceOccurrence {
        path: scope.join("AGENTS.md"),
        physical_path: None,
        origin,
        scope,
        rule,
        raw_bytes: texts[index].len(),
        included_bytes: texts[index].len(),
        project_bytes_before: if index == 2 { texts[1].len() } else { 0 },
        content_index: Some(index),
    })
    .collect();
    ResolvedContext {
        input: ContextInput {
            workspace: workspace.clone(),
            target: workspace.join("src/auth.rs"),
            cwd,
            profile_id: CODEX_PROFILE_ID.to_owned(),
            codex_home,
            codex_home_override: false,
            fallback_filenames: Vec::new(),
            project_doc_max_bytes: 32_768,
            project_root_markers: vec![".git".to_owned()],
            project_trust: ProjectTrust::Trusted,
        },
        profile: Some(ContextProfile::codex()),
        project_root: Some(workspace),
        status: ResolutionStatus::Resolved,
        assumptions: Vec::new(),
        discovery_digest: Some([1; 32]),
        sources,
        contents: texts
            .into_iter()
            .map(|text| ContextContent {
                text: text.to_owned(),
            })
            .collect(),
        diagnostics: Vec::new(),
        available_skills: Vec::new(),
    }
}

fn document() -> ReviewRequest {
    ReviewRequest::new(
        ArtifactLens::Plan,
        ReviewScope::Document,
        ReviewSource::document_at("# Plan\nold body\n", "docs/task.md"),
        SourceSnapshot::new(7, 3),
    )
    .unwrap()
}

fn context_request(context: &ResolvedContext, indices: &[usize]) -> ReviewRequest {
    document()
        .with_effective_agent_context(context.select_sources(indices).unwrap())
        .unwrap()
}

fn skill_request() -> ReviewRequest {
    let raw = vec![0xff, 0x00, 0x80];
    ReviewRequest::agent_skill(
        SkillPackage::new(
            vec![
                SkillPackageFile::text("SKILL.md", "entrypoint old", "entrypoint").unwrap(),
                SkillPackageFile::text(
                    "references/support.md",
                    "supporting-sentinel",
                    "supporting context",
                )
                .unwrap(),
                SkillPackageFile::binary_raw(
                    "assets/raw.bin",
                    raw.clone(),
                    format!("{:x}", Sha256::digest(raw)),
                    "explicit raw selection",
                )
                .unwrap(),
                SkillPackageFile::binary_metadata(
                    "assets/default.bin",
                    7,
                    "11".repeat(32),
                    "binary metadata only",
                )
                .unwrap(),
            ],
            vec![
                SkillPackageOmission::symlink("links/outside.md", "outside authenticated root")
                    .unwrap(),
            ],
        )
        .unwrap(),
        SourceSnapshot::new(7, 3),
    )
    .unwrap()
}

fn skill_context_request(context: &ResolvedContext) -> ReviewRequest {
    skill_request()
        .with_effective_agent_context(context.select_sources(&[0, 2]).unwrap())
        .unwrap()
}

fn prepared(provider: Provider, base_url: &str) -> PreparedReview {
    let mut settings = SettingsData::default();
    settings.model_provider = provider.key().to_owned();
    settings.model_name = "context-review-fixture".to_owned();
    settings.model_base_url = base_url.to_owned();
    let vault = CredentialVault::with_store(Arc::new(EmptyStore));
    let endpoint = EndpointIdentity::parse(provider, Some(base_url)).unwrap();
    vault
        .replace_session(
            endpoint.credential_target().to_string(),
            "fixture-secret".to_owned(),
        )
        .unwrap();
    PreparedReview::from_settings(&settings, &vault).unwrap()
}

fn output(request: &ReviewRequest) -> ReviewModelOutput {
    ReviewModelOutput {
        schema_version: "review-v1".to_owned(),
        scope: request.scope,
        understood_intent: ReviewSections {
            stated_goal: "review the plan".into(),
            relevant_context: Vec::new(),
            constraints: Vec::new(),
            non_goals: Vec::new(),
            expected_deliverable: "an executable plan".into(),
            success_evidence: Vec::new(),
            inferred_assumptions: Vec::new(),
            unresolved_decisions: Vec::new(),
        },
        findings: Vec::new(),
        clarification_questions: Vec::new(),
    }
}

#[test]
fn selected_context_is_frozen_without_changing_document_identity() {
    let original = document();
    let mut context = resolved();
    let request = context_request(&context, &[2, 0]);
    context.contents[0].text = "later-local-edit".to_owned();
    assert_eq!(request.source, original.source);
    assert_eq!(request.scope, original.scope);
    assert_eq!(request.snapshot, original.snapshot);
    assert_eq!(request.outbound_bytes(), original.outbound_bytes());
    let selected = request.effective_agent_context.as_ref().unwrap();
    assert_eq!(
        selected.input().target,
        context.input.workspace.join("src/auth.rs")
    );
    assert_eq!(
        selected
            .sources()
            .iter()
            .map(|s| s.source_index())
            .collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(
        selected.source_content(&selected.sources()[0]),
        Some("selected-global-sentinel")
    );
    let wire = serde_json::to_string(&request).unwrap();
    assert_eq!(ReviewRequest::decode_json(&wire).unwrap(), request);
    assert!(!wire.contains("unselected-root-sentinel"));
    assert!(!wire.contains("available_skills"));
}

#[test]
fn context_attachments_fail_closed_without_expanding_selection_scope() {
    let context = resolved();
    let empty = context.select_sources(&[]).unwrap();
    assert_eq!(
        document().with_effective_agent_context(empty),
        Err(ReviewValidationError::EmptyEffectiveContext)
    );
    let selected = context.select_sources(&[0]).unwrap();
    let selection = ReviewRequest::selection(
        ArtifactLens::Plan,
        "body",
        ByteRange::new(0, 4).unwrap(),
        SourceSnapshot::default(),
    )
    .unwrap();
    assert_eq!(
        selection.with_effective_agent_context(selected.clone()),
        Err(ReviewValidationError::EffectiveContextRequiresWholeArtifact)
    );
    let package = SkillPackage::new(
        vec![SkillPackageFile::text("SKILL.md", "skill", "entrypoint").unwrap()],
        Vec::new(),
    )
    .unwrap();
    let skill = ReviewRequest::agent_skill(package, SourceSnapshot::default()).unwrap();
    let with_context = skill
        .clone()
        .with_effective_agent_context(selected)
        .unwrap();
    assert_eq!(with_context.source, skill.source);
    assert_eq!(with_context.scope, ReviewScope::AgentSkillPackage);
    assert_eq!(with_context.snapshot, skill.snapshot);
    assert!(with_context.effective_agent_context.is_some());

    let request = context_request(&context, &[0, 2]);
    let wire = serde_json::to_value(&request).unwrap();
    let mut unsupported = wire.clone();
    unsupported["effective_agent_context"]["input"]["profile_id"] =
        serde_json::json!("unsupported-profile");
    let mut forged_profile = wire.clone();
    forged_profile["effective_agent_context"]["profile"]["content_digest"] =
        serde_json::json!("0".repeat(64));
    let mut forged = wire.clone();
    forged["effective_agent_context"]["sources"][1]["source_index"] = serde_json::json!(0);
    let mut hidden = wire.clone();
    hidden["effective_agent_context"]["contents"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"text": "unselected-hidden-sentinel"}));
    let mut empty = wire.clone();
    empty["effective_agent_context"]["sources"] = serde_json::json!([]);
    empty["effective_agent_context"]["contents"] = serde_json::json!([]);
    let mut selection = wire;
    selection["scope"] =
        serde_json::to_value(ReviewScope::selection(ByteRange::new(0, 4).unwrap())).unwrap();
    for invalid in [
        unsupported,
        forged_profile,
        forged,
        hidden,
        empty,
        selection,
    ] {
        assert!(ReviewRequest::decode_json(&invalid.to_string()).is_err());
    }
}

#[test]
fn disclosure_names_exact_selected_sources_and_canonical_digest() {
    let context = resolved();
    let request = context_request(&context, &[2, 0]);
    let selected = request.effective_agent_context.as_ref().unwrap();
    let inspection = inspect_document_request(&request).unwrap();
    let repeated = context_request(&context, &[0, 2]);
    assert_eq!(inspection, inspect_document_request(&repeated).unwrap());
    let mut canonical = b"markturbo-effective-context-review-v1\0".to_vec();
    let source = request.outbound_bytes();
    let mut disclosed = serde_json::to_value(selected).unwrap();
    disclosed
        .as_object_mut()
        .unwrap()
        .remove("discovery_digest");
    disclosed.sort_all_objects();
    let context_bytes = serde_json::to_vec(&disclosed).unwrap();
    canonical.extend_from_slice(&(source.len() as u64).to_be_bytes());
    canonical.extend_from_slice(&source);
    canonical.extend_from_slice(&(context_bytes.len() as u64).to_be_bytes());
    canonical.extend_from_slice(&context_bytes);
    assert_eq!(
        inspection.canonical_sha256(),
        format!("{:x}", Sha256::digest(&canonical))
    );
    assert_eq!(inspection.canonical_byte_size(), canonical.len() as u64);
    assert_eq!(
        inspection.source_byte_size(),
        (source.len()
            + selected
                .contents()
                .iter()
                .map(|c| c.text.len())
                .sum::<usize>()) as u64
    );

    for (url, location) in [
        ("https://PROXY.example:443/v1", EndpointLocation::Remote),
        ("http://127.0.0.1:1/v1/", EndpointLocation::Local),
    ] {
        let frozen = prepared(Provider::OpenAiResponses, url)
            .bind_document_request(request.clone(), ReviewLanguage::English)
            .unwrap();
        let disclosure = frozen.disclosure();
        assert_eq!(disclosure.operation(), ModelOperation::Review);
        assert_eq!(disclosure.endpoint().location(), location);
        assert_eq!(disclosure.endpoint().provider(), Provider::OpenAiResponses);
        assert_eq!(
            disclosure.endpoint(),
            &EndpointIdentity::parse(Provider::OpenAiResponses, Some(url)).unwrap()
        );
        assert_eq!(
            disclosure.scope().kind(),
            OutboundScopeKind::DocumentWithEffectiveAgentContext
        );
        assert_eq!(
            disclosure.scope().primary_content_byte_size(),
            source.len() as u64
        );
        let mut names = selected
            .sources()
            .iter()
            .map(|s| s.occurrence().path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        assert_eq!(disclosure.scope().effective_context_sources(), names);
        assert_eq!(
            disclosure.scope().review_request_digest(),
            Some(Sha256::digest(&canonical).into())
        );
        let mut consent = ConsentCapability::from_decision(disclosure, ConsentDecision::Approve);
        let authorization = frozen.authorize(&mut consent).unwrap();
        assert!(authorization.matches(disclosure));
        assert!(matches!(
            consent.authorize(disclosure),
            Err(ConsentError::Consumed)
        ));
    }
}

#[test]
fn context_content_identity_and_selection_changes_invalidate_consent_and_digest() {
    let context = resolved();
    let original = context_request(&context, &[0, 2]);
    let url = "https://proxy.example/v1/";
    let frozen = prepared(Provider::OpenAiResponses, url)
        .bind_document_request(original.clone(), ReviewLanguage::English)
        .unwrap();
    let mut changed_content = context.clone();
    changed_content.contents[0].text = "SELECTED-global-sentinel".to_owned();
    let mut changed_source = context.clone();
    changed_source.sources[2].path = changed_source.input.cwd.join("AGENTS.override.md");
    changed_source.sources[2].rule = InclusionRule::ProjectOverride;
    let mut changed_metadata = context.clone();
    changed_metadata
        .assumptions
        .push("explicit-configuration-sentinel".to_owned());
    let mut changed_target = context.clone();
    changed_target.input.target = changed_target.input.workspace.join("src/session.rs");
    let mut changed_document = original.clone();
    changed_document.source = ReviewSource::document_at("# Plan\nnew body\n", "docs/task.md");
    for changed in [
        context_request(&changed_content, &[0, 2]),
        context_request(&changed_source, &[0, 2]),
        context_request(&changed_metadata, &[0, 2]),
        context_request(&changed_target, &[0, 2]),
        context_request(&context, &[0]),
        changed_document,
    ] {
        let different = prepared(Provider::OpenAiResponses, url)
            .bind_document_request(changed, ReviewLanguage::English)
            .unwrap();
        assert_ne!(
            frozen.inspection().unwrap().canonical_sha256(),
            different.inspection().unwrap().canonical_sha256()
        );
        let mut consent =
            ConsentCapability::from_decision(frozen.disclosure(), ConsentDecision::Approve);
        assert!(matches!(
            consent.authorize(different.disclosure()),
            Err(ConsentError::Mismatch)
        ));
        assert!(matches!(
            consent.authorize(frozen.disclosure()),
            Err(ConsentError::Consumed)
        ));
    }
    for (provider, endpoint) in [
        (Provider::OpenAiResponses, "https://elsewhere.example/v1/"),
        (Provider::OpenAiChat, url),
    ] {
        let different = prepared(provider, endpoint)
            .bind_document_request(original.clone(), ReviewLanguage::English)
            .unwrap();
        assert_eq!(
            frozen.inspection().unwrap(),
            different.inspection().unwrap()
        );
        let mut consent =
            ConsentCapability::from_decision(frozen.disclosure(), ConsentDecision::Approve);
        assert!(matches!(
            consent.authorize(different.disclosure()),
            Err(ConsentError::Mismatch)
        ));
    }
}

#[test]
fn undisclosed_discovery_state_preserves_canonical_review_and_revision_identity() {
    let context = resolved();
    let request = context_request(&context, &[0, 2]);
    let mut changed = context;
    changed.discovery_digest = Some([2; 32]);
    changed.contents[1].text = "UNSELECTED-root-sentinel".to_owned();
    let different = context_request(&changed, &[0, 2]);
    assert_ne!(request, different);
    assert_eq!(
        inspect_document_request(&request).unwrap(),
        inspect_document_request(&different).unwrap()
    );
    let review = output(&request);
    let answers = RevisionAnswers::empty();
    assert_eq!(
        revision_request_binding(&request, &review, &answers).unwrap(),
        revision_request_binding(&different, &review, &answers).unwrap()
    );
    let url = "https://proxy.example/v1/";
    let first = prepared(Provider::OpenAiResponses, url)
        .bind_document_request(request, ReviewLanguage::English)
        .unwrap();
    let second = prepared(Provider::OpenAiResponses, url)
        .bind_document_request(different.clone(), ReviewLanguage::English)
        .unwrap();
    assert_eq!(
        first.disclosure().scope().review_request_digest(),
        second.disclosure().scope().review_request_digest()
    );
    let local = serde_json::to_string(&different).unwrap();
    assert_eq!(ReviewRequest::decode_json(&local).unwrap(), different);
    assert_eq!(
        serde_json::to_value(second.request()).unwrap()["effective_agent_context"]["discovery_digest"],
        serde_json::to_value([2_u8; 32]).unwrap()
    );
}

#[test]
fn context_never_becomes_a_document_anchor() {
    let request = context_request(&resolved(), &[0, 2]);
    let mut wire = serde_json::to_value(output(&request)).unwrap();
    wire["findings"] = serde_json::json!([{
        "kind": "source",
        "text": "provider paraphrase",
        "anchor": {"kind": "document_quote", "quote": "old body"}
    }]);
    let decoded = ReviewModelOutput::decode_json(&wire.to_string(), &request).unwrap();
    assert_eq!(
        decoded.findings[0].anchor,
        SourceAnchor::document(SourceLocation::bytes(7, 15))
    );
    wire["findings"][0]["anchor"]["quote"] = serde_json::json!("selected-global-sentinel");
    assert!(ReviewModelOutput::decode_json(&wire.to_string(), &request).is_err());
}

#[test]
fn context_source_limits_are_enforced_before_disclosure() {
    let mut context = resolved();
    context.contents[0].text = "x".repeat(REVIEW_MAX_FILE_BYTES + 1);
    context.sources[0].raw_bytes = REVIEW_MAX_FILE_BYTES + 1;
    context.sources[0].included_bytes = REVIEW_MAX_FILE_BYTES + 1;
    let skill = skill_request()
        .with_effective_agent_context(context.select_sources(&[0]).unwrap())
        .unwrap();
    for request in [context_request(&context, &[0]), skill] {
        assert!(matches!(
            prepared(Provider::OpenAiResponses, "https://proxy.example/v1/")
                .bind_document_request(request, ReviewLanguage::English),
            Err(ReviewError::InvalidRequest {
                reason: ReviewRequestError::FileTooLarge { .. }
            })
        ));
    }

    context.contents = (0..9)
        .map(|index| ContextContent {
            text: index.to_string().repeat(REVIEW_MAX_FILE_BYTES),
        })
        .collect();
    context.sources.truncate(1);
    context.sources[0].raw_bytes = REVIEW_MAX_FILE_BYTES;
    context.sources[0].included_bytes = REVIEW_MAX_FILE_BYTES;
    let mut scope = context.input.workspace.clone();
    for index in 1..9 {
        context.sources.push(SourceOccurrence {
            path: scope.join("AGENTS.md"),
            physical_path: None,
            origin: Origin::Workspace,
            scope: scope.clone(),
            rule: InclusionRule::ProjectAgents,
            raw_bytes: REVIEW_MAX_FILE_BYTES,
            included_bytes: REVIEW_MAX_FILE_BYTES,
            project_bytes_before: (index - 1) * REVIEW_MAX_FILE_BYTES,
            content_index: Some(index),
        });
        if index < 8 {
            scope = scope.join("nested");
        }
    }
    context.input.cwd = scope.clone();
    context.input.target = scope.join("plan.md");
    context.input.project_doc_max_bytes = 8 * REVIEW_MAX_FILE_BYTES;
    let indices = (0..9).collect::<Vec<_>>();
    let skill = skill_request()
        .with_effective_agent_context(context.select_sources(&indices).unwrap())
        .unwrap();
    for request in [context_request(&context, &indices), skill] {
        assert!(matches!(
            prepared(Provider::OpenAiResponses, "https://proxy.example/v1/")
                .bind_document_request(request, ReviewLanguage::English),
            Err(ReviewError::InvalidRequest {
                reason: ReviewRequestError::SourceTooLarge { .. }
            })
        ));
    }
}

#[test]
fn revision_keeps_prior_context_binding_but_edits_only_the_document() {
    let context = resolved();
    let request = context_request(&context, &[0, 2]);
    let review = output(&request);
    let answers = RevisionAnswers::empty();
    let binding = revision_request_binding(&request, &review, &answers).unwrap();
    assert_eq!(
        binding.source_sha256(),
        &<[u8; 32]>::from(Sha256::digest(request.outbound_bytes()))
    );
    let changed = context_request(&context, &[0]);
    let changed_binding = revision_request_binding(&changed, &review, &answers).unwrap();
    assert_ne!(
        binding.review_context_digest(),
        changed_binding.review_context_digest()
    );
    assert_eq!(binding.answers_digest(), changed_binding.answers_digest());
    let frozen = prepared(Provider::OpenAiResponses, "https://proxy.example/v1/")
        .bind_revision_request(
            request.clone(),
            review.clone(),
            answers.clone(),
            ReviewLanguage::English,
        )
        .unwrap();
    assert_eq!(
        frozen.disclosure().scope().kind(),
        OutboundScopeKind::Document
    );
    assert_eq!(
        frozen.disclosure().scope().primary_content_byte_size(),
        request.outbound_bytes().len() as u64
    );
    assert_eq!(frozen.disclosure().revision_binding(), Some(&binding));
    assert!(
        frozen
            .disclosure()
            .scope()
            .effective_context_sources()
            .is_empty()
    );
    let different = prepared(Provider::OpenAiResponses, "https://proxy.example/v1/")
        .bind_revision_request(
            changed,
            review.clone(),
            answers.clone(),
            ReviewLanguage::English,
        )
        .unwrap();
    let mut consent =
        ConsentCapability::from_decision(frozen.disclosure(), ConsentDecision::Approve);
    assert!(matches!(
        consent.authorize(different.disclosure()),
        Err(ConsentError::Mismatch)
    ));
    let response = serde_json::json!({
        "schema_version": "revision-v1",
        "groups": [{"rationale": "clarify the plan", "edits": [{
            "range": {"start": 7, "end": 15}, "expected_source": "old body", "replacement": "new body"
        }]}],
        "question_coverage": []
    });
    let capture =
        decode_revision_capture(&request, &review, &answers, &response.to_string()).unwrap();
    assert_eq!(capture.binding(), &binding);
    assert_eq!(
        capture.proposal().hunks()[0].source(),
        ByteRange::new(7, 15).unwrap()
    );
    let mut context_edit = response;
    context_edit["groups"][0]["edits"][0]["expected_source"] =
        serde_json::json!("selected-global-sentinel");
    assert!(
        decode_revision_capture(&request, &review, &answers, &context_edit.to_string()).is_err()
    );

    let drafts = RevisionAnswers::for_recovery(vec![RevisionAnswer::answered("")]).unwrap();
    assert!(revision_request_binding(&request, &review, &drafts).is_ok());
    assert!(
        prepared(Provider::OpenAiResponses, "https://proxy.example/v1/")
            .bind_revision_request(request, review, drafts, ReviewLanguage::English)
            .is_err()
    );
}

#[test]
fn context_changes_leave_recovered_answer_drafts_bound_to_prior_review() {
    let context = resolved();
    let request = context_request(&context, &[0, 2]);
    let review = output(&request);
    let drafts = RevisionAnswers::for_recovery(vec![RevisionAnswer::answered("")]).unwrap();
    let prior = revision_request_binding(&request, &review, &drafts).unwrap();
    let recovered = RevisionRecovery::new(prior, drafts.clone());
    let mut changed_target = context;
    changed_target.input.target = changed_target.input.workspace.join("src/session.rs");
    let current =
        revision_request_binding(&context_request(&changed_target, &[0, 2]), &review, &drafts)
            .unwrap();
    assert_eq!(recovered.answers(), &drafts);
    assert_eq!(recovered.binding(), &prior);
    assert_ne!(recovered.binding(), &current);
    assert_eq!(prior.source_sha256(), current.source_sha256());
    assert_eq!(prior.source_revision(), request.snapshot.revision);
    assert_eq!(
        prior.source_generation(),
        request.snapshot.source_generation
    );
    assert!(!format!("{recovered:?}").contains("selected-global-sentinel"));
}

#[test]
fn skill_context_disclosure_preserves_full_inventory_and_selected_sources() {
    let request = skill_context_request(&resolved());
    let package = request.source.package().unwrap();
    let frozen = prepared(Provider::OpenAiResponses, "https://proxy.example/v1/")
        .bind_document_request(request.clone(), ReviewLanguage::English)
        .unwrap();
    let scope = frozen.disclosure().scope();
    assert_eq!(scope.kind(), OutboundScopeKind::AgentSkillPackage);
    assert_eq!(scope.primary_content_byte_size(), package.total_byte_size());
    let inventory = scope.agent_skill_inventory().unwrap();
    assert_eq!(inventory.files().len(), package.files().len());
    for (actual, expected) in inventory.files().iter().zip(package.files()) {
        assert_eq!(actual.normalized_relative_path(), expected.path);
        assert_eq!(actual.byte_size(), expected.byte_size);
        assert_eq!(actual.inclusion_reason(), expected.inclusion_reason);
    }
    assert_eq!(inventory.omissions().len(), 1);
    assert_eq!(scope.effective_context_sources().len(), 2);
    let selected = request.effective_agent_context.as_ref().unwrap();
    for source in selected.sources() {
        assert!(
            scope
                .effective_context_sources()
                .contains(&source.occurrence().path.to_string_lossy().into_owned())
        );
    }
    assert!(scope.review_request_digest().is_some());
    assert!(scope.permits_review());
    assert!(!scope.permits_revision());
    assert_eq!(
        frozen.inspection().unwrap(),
        inspect_document_request(&request).unwrap()
    );
    assert!(frozen.agent_skill_payload_proof().unwrap().is_exact());
    let mut consent =
        ConsentCapability::from_decision(frozen.disclosure(), ConsentDecision::Approve);
    assert!(
        frozen
            .authorize(&mut consent)
            .unwrap()
            .matches(frozen.disclosure())
    );
    assert!(matches!(
        consent.authorize(frozen.disclosure()),
        Err(ConsentError::Consumed)
    ));

    let mut wire = serde_json::to_value(output(&request)).unwrap();
    wire["findings"] = serde_json::json!([{
        "kind": "source", "text": "provider paraphrase",
        "anchor": {"kind": "agent_skill_file_quote", "path": "references/support.md", "quote": "supporting-sentinel"}
    }]);
    let result = ReviewModelOutput::decode_json(&wire.to_string(), &request).unwrap();
    assert_eq!(
        result.findings[0].anchor,
        SourceAnchor::agent_skill_file(
            "references/support.md",
            SourceLocation::bytes(0, "supporting-sentinel".len() as u64)
        )
        .unwrap()
    );
    wire["findings"][0]["anchor"]["quote"] = serde_json::json!("selected-global-sentinel");
    assert!(ReviewModelOutput::decode_json(&wire.to_string(), &request).is_err());
}

#[test]
fn skill_package_inventory_and_context_changes_invalidate_combined_consent() {
    let context = resolved();
    let request = skill_context_request(&context);
    let url = "https://proxy.example/v1/";
    let first = prepared(Provider::OpenAiResponses, url)
        .bind_document_request(request.clone(), ReviewLanguage::English)
        .unwrap();
    let mut changed_context = context.clone();
    changed_context.contents[0].text = "SELECTED-global-sentinel".to_owned();
    let mut requests = vec![skill_context_request(&changed_context)];
    for (path, content, reason) in [
        (
            "references/support.md",
            "SUPPORTING-sentinel",
            "supporting context",
        ),
        (
            "references/renamed.md",
            "supporting-sentinel",
            "supporting context",
        ),
        (
            "references/support.md",
            "supporting-sentinel",
            "explicit support selection",
        ),
    ] {
        let package = request.source.package().unwrap();
        let mut files = package.files().to_vec();
        *files
            .iter_mut()
            .find(|file| file.path == "references/support.md")
            .unwrap() = SkillPackageFile::text(path, content, reason).unwrap();
        let mut changed = request.clone();
        changed.source = ReviewSource::agent_skill_package(
            SkillPackage::new(files, package.omissions().to_vec()).unwrap(),
        );
        requests.push(changed);
    }
    for changed in requests {
        let different = prepared(Provider::OpenAiResponses, url)
            .bind_document_request(changed, ReviewLanguage::English)
            .unwrap();
        assert_ne!(
            first.inspection().unwrap().canonical_sha256(),
            different.inspection().unwrap().canonical_sha256()
        );
        let mut consent =
            ConsentCapability::from_decision(first.disclosure(), ConsentDecision::Approve);
        assert!(matches!(
            consent.authorize(different.disclosure()),
            Err(ConsentError::Mismatch)
        ));
        assert!(matches!(
            consent.authorize(first.disclosure()),
            Err(ConsentError::Consumed)
        ));
    }
    let source_only = prepared(Provider::OpenAiResponses, url)
        .bind_document_request(skill_request(), ReviewLanguage::English)
        .unwrap();
    let mut consent =
        ConsentCapability::from_decision(source_only.disclosure(), ConsentDecision::Approve);
    assert!(matches!(
        consent.authorize(first.disclosure()),
        Err(ConsentError::Mismatch)
    ));
    let mut only_local = context;
    only_local.discovery_digest = Some([2; 32]);
    only_local.contents[1].text = "UNSELECTED-root-sentinel".to_owned();
    let identical_disclosure = skill_context_request(&only_local);
    assert_ne!(request, identical_disclosure);
    assert_eq!(
        inspect_document_request(&request).unwrap(),
        inspect_document_request(&identical_disclosure).unwrap()
    );
}

#[test]
fn skill_revision_resends_package_only_and_retains_prior_selected_context_binding() {
    let context = resolved();
    let request = skill_context_request(&context);
    let review = output(&request);
    let answers = RevisionAnswers::empty();
    let binding = revision_request_binding(&request, &review, &answers).unwrap();
    let revision = prepared(Provider::OpenAiResponses, "https://proxy.example/v1/")
        .bind_revision_request(
            request.clone(),
            review.clone(),
            answers.clone(),
            ReviewLanguage::English,
        )
        .unwrap();
    assert_eq!(
        revision.disclosure().scope().kind(),
        OutboundScopeKind::AgentSkillPackage
    );
    assert_eq!(
        revision
            .disclosure()
            .scope()
            .agent_skill_inventory()
            .unwrap()
            .files()
            .len(),
        4
    );
    assert!(
        revision
            .disclosure()
            .scope()
            .effective_context_sources()
            .is_empty()
    );
    assert!(
        revision
            .disclosure()
            .scope()
            .review_request_digest()
            .is_none()
    );
    assert_eq!(revision.disclosure().revision_binding(), Some(&binding));
    let mut changed_context = context;
    changed_context.contents[0].text = "SELECTED-global-sentinel".to_owned();
    let changed_binding =
        revision_request_binding(&skill_context_request(&changed_context), &review, &answers)
            .unwrap();
    assert_eq!(binding.source_sha256(), changed_binding.source_sha256());
    assert_ne!(
        binding.review_context_digest(),
        changed_binding.review_context_digest()
    );
    let raw = serde_json::json!({
        "schema_version": "revision-v1",
        "groups": [{"rationale": "clarify the entrypoint", "edits": [{
            "range": {"start": 0, "end": "entrypoint old".len()}, "expected_source": "entrypoint old", "replacement": "entrypoint new"
        }]}], "question_coverage": []
    });
    let capture = decode_revision_capture(&request, &review, &answers, &raw.to_string()).unwrap();
    assert_eq!(capture.proposal().source(), "entrypoint old");
    assert_eq!(capture.proposal().accept_all(), "entrypoint new");
    let mut other_source = raw;
    other_source["groups"][0]["edits"][0]["expected_source"] =
        serde_json::json!("selected-global-sentinel");
    assert!(
        decode_revision_capture(&request, &review, &answers, &other_source.to_string()).is_err()
    );
}
