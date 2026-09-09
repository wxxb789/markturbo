//! Run Goal 06's fixed corpus through the existing in-process Review boundary.
//!
//! This binary is deliberately owner-operated. It accepts no credential value
//! or endpoint argument, reads an eligible `OPENAI_API_KEY` from its process
//! environment through `CredentialVault`, and writes source-bearing records
//! only to a caller-selected directory outside the repository.

use std::{
    collections::BTreeMap,
    env,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use mt_app::{
    credentials::{CredentialError, CredentialVault, Secret, SecureCredentialStore},
    model::{ConsentCapability, ConsentDecision, EndpointIdentity, Provider},
    review::{PreparedReview, ReviewExecutionError, ReviewExecutionRecord, ReviewLanguage},
    settings::AppSettings,
};
use mt_doc::review::{
    ArtifactLens, ReviewRequest, ReviewScope, ReviewSource, SkillPackage, SkillPackageFile,
    SourceSnapshot,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CORPUS_VERSION: &str = "goal-01-v1";
const MANIFEST_PATH: &str = "evaluation/goal-01/MANIFEST.sha256";
const APPROVED_MANIFEST_SHA256: &str =
    "290a0421753974dabb0cef1552f158cba4ddf2cafd3d91f0ea256d406f561f7c";
const EXPECTED_MANIFEST_ENTRIES: usize = 45;
const ENDPOINT: &str = "http://127.0.0.1:4141/v1/";
const MODEL: &str = "gpt-5.6-terra";
const SEND_ACKNOWLEDGEMENT: &str = "--confirm-send-goal-01-v1-to-http-127-0-0-1-4141-v1";
const SEND_AUTHORIZATION_REQUIRED: &str = "sending source content from the fixed goal-01-v1 corpus requires --confirm-send-goal-01-v1-to-http-127-0-0-1-4141-v1, which confirms the exact corpus-only source scope and OpenAI Responses endpoint http://127.0.0.1:4141/v1/ (loopback HTTP, unencrypted, no proxy), model gpt-5.6-terra, reasoning effort medium, provider-default sampling, and no tools, browsing, memory, or agent actions";

#[derive(Clone, Copy)]
enum ArtifactSource {
    Document,
    AgentSkill,
}

#[derive(Clone, Copy)]
struct Artifact {
    id: &'static str,
    lens: ArtifactLens,
    path: &'static str,
    source: ArtifactSource,
}

const ARTIFACTS: [Artifact; 12] = [
    Artifact {
        id: "TP-01",
        lens: ArtifactLens::Prompt,
        path: "evaluation/goal-01/task-prompts/01-upgrade-gpui-without-regression.md",
        source: ArtifactSource::Document,
    },
    Artifact {
        id: "TP-02",
        lens: ArtifactLens::Prompt,
        path: "evaluation/goal-01/task-prompts/02-keep-web-preview-in-one-window.md",
        source: ArtifactSource::Document,
    },
    Artifact {
        id: "TP-03",
        lens: ArtifactLens::Prompt,
        path: "evaluation/goal-01/task-prompts/03-diagnose-duplicate-git-crates.md",
        source: ArtifactSource::Document,
    },
    Artifact {
        id: "TP-04",
        lens: ArtifactLens::Prompt,
        path: "evaluation/goal-01/task-prompts/04-measure-release-profile-on-quiet-host.md",
        source: ArtifactSource::Document,
    },
    Artifact {
        id: "SP-01",
        lens: ArtifactLens::Specification,
        path: "evaluation/goal-01/snapshots/goals/02-guarantee-user-text-safety.md",
        source: ArtifactSource::Document,
    },
    Artifact {
        id: "SP-02",
        lens: ArtifactLens::Plan,
        path: "evaluation/goal-01/snapshots/goals/03-create-first-use-document-flow.md",
        source: ArtifactSource::Document,
    },
    Artifact {
        id: "SP-03",
        lens: ArtifactLens::Specification,
        path: "evaluation/goal-01/snapshots/goals/05a-protect-model-credentials-and-request-privacy.md",
        source: ArtifactSource::Document,
    },
    Artifact {
        id: "SP-04",
        lens: ArtifactLens::Plan,
        path: "evaluation/goal-01/snapshots/goals/06-deliver-read-only-intent-review.md",
        source: ArtifactSource::Document,
    },
    Artifact {
        id: "AI-01",
        lens: ArtifactLens::AgentInstructions,
        path: "evaluation/goal-01/snapshots/agent-instructions/repository-AGENTS.md",
        source: ArtifactSource::Document,
    },
    Artifact {
        id: "AI-02",
        lens: ArtifactLens::AgentInstructions,
        path: "evaluation/goal-01/snapshots/agent-instructions/sample-workspace-AGENTS.md",
        source: ArtifactSource::Document,
    },
    Artifact {
        id: "AS-01",
        lens: ArtifactLens::AgentSkill,
        path: "evaluation/goal-01/snapshots/skills/gpui",
        source: ArtifactSource::AgentSkill,
    },
    Artifact {
        id: "AS-02",
        lens: ArtifactLens::AgentSkill,
        path: "evaluation/goal-01/snapshots/skills/gpui-component",
        source: ArtifactSource::AgentSkill,
    },
];

struct Manifest {
    entries: BTreeMap<String, String>,
}

/// Deliberately prevents corpus evaluation from reading or writing the user's
/// persistent credentials. `PreparedReview` may therefore resolve only the
/// explicitly eligible credential inherited by this child process.
struct EnvironmentOnlyStore;

impl SecureCredentialStore for EnvironmentOnlyStore {
    fn is_supported(&self) -> bool {
        false
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

enum ArtifactExecutionFailure {
    Preflight,
    Review(Box<ReviewExecutionError>),
}

impl ArtifactExecutionFailure {
    fn code(&self) -> &'static str {
        match self {
            Self::Preflight => "preflight_failed",
            Self::Review(error) => error.error().code(),
        }
    }
}

fn main() -> ExitCode {
    if env::args_os()
        .skip(1)
        .any(|arg| arg == OsStr::new("--help") || arg == OsStr::new("-h"))
    {
        print_help();
        return ExitCode::SUCCESS;
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("Goal 06 evaluation did not complete: {message}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "usage: markturbo-goal06-evaluate --records-dir <owner-local-directory> {SEND_ACKNOWLEDGEMENT} [--artifact <id>]\n\
         \n\
         Sends only the immutable goal-01-v1 corpus source scope through the fixed OpenAI\n\
         Responses configuration: endpoint http://127.0.0.1:4141/v1/ (loopback HTTP,\n\
         unencrypted, no proxy), model gpt-5.6-terra, reasoning effort medium, provider-default\n\
         sampling, and no tools, browsing, memory, or agent actions. Omit --artifact to send\n\
         all 12 fixed artifacts; --artifact sends exactly one named artifact from that corpus.\n\
         The acknowledgement confirms that exact endpoint identity and corpus-only source scope\n\
         before any content is sent. Credentials and endpoints are not accepted as arguments.\n\
         A nonempty OPENAI_API_KEY must be inherited from this child process; persistent\n\
         credentials are deliberately not read or used. Source-bearing records are\n\
         written only below the new owner-local records directory."
    );
}

fn run() -> Result<(), &'static str> {
    let (records_dir, selected_artifact) = parse_arguments()?;
    let repo = repository_root()?;
    prepare_owner_local_directory(&records_dir, &repo)?;
    require_environment_credential()?;

    let endpoint = EndpointIdentity::parse(Provider::OpenAiResponses, Some(ENDPOINT))
        .map_err(|_| "the fixed local evaluation endpoint is invalid")?;
    let mut settings = AppSettings::default();
    settings.model_provider = Provider::OpenAiResponses.key().to_owned();
    settings.model_name = MODEL.to_owned();
    settings.model_base_url = ENDPOINT.to_owned();
    settings.model_environment_key_identity = endpoint.credential_target().as_str().to_owned();
    let vault = CredentialVault::with_store(Arc::new(EnvironmentOnlyStore));
    let mut completed = 0_usize;
    let mut failed = 0_usize;

    let artifacts = selected_artifact.map_or_else(|| ARTIFACTS.to_vec(), |artifact| vec![artifact]);
    for artifact in artifacts.iter().copied() {
        let artifact_dir = records_dir.join("records").join(artifact.id);
        fs::create_dir_all(&artifact_dir)
            .map_err(|_| "could not create owner-local record directory")?;

        match execute_artifact(&repo, &settings, &vault, artifact) {
            Ok(record) => {
                write_completed_record(&artifact_dir, artifact, &record)?;
                write_judgment_template(&records_dir, artifact, &record)?;
                completed += 1;
            }
            Err(error) => {
                write_failed_record(&artifact_dir, artifact, &error)?;
                failed += 1;
            }
        }
    }

    write_json_new(
        &records_dir.join("run.json"),
        &json!({
            "schema": "markturbo-goal-06-owner-local-run-v1",
            "corpus_version": CORPUS_VERSION,
            "manifest_sha256": APPROVED_MANIFEST_SHA256,
            "configuration": fixed_configuration(),
            "artifact_count": artifacts.len(),
            "completed_count": completed,
            "failed_count": failed,
        }),
    )?;

    if failed != 0 {
        return Err(
            "one or more artifacts failed; owner-local records retain no provider diagnostic",
        );
    }
    println!("GOAL06_EVALUATION_COMPLETE={completed}");
    Ok(())
}

fn parse_arguments() -> Result<(PathBuf, Option<Artifact>), &'static str> {
    parse_arguments_from(env::args_os().skip(1))
}

fn parse_arguments_from(
    arguments: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<(PathBuf, Option<Artifact>), &'static str> {
    let mut args = arguments.into_iter();
    let mut records_dir = None;
    let mut selected_artifact = None;
    let mut acknowledged = false;

    while let Some(flag) = args.next() {
        if flag == OsStr::new("--records-dir") {
            if records_dir
                .replace(PathBuf::from(
                    args.next()
                        .ok_or("owner-local records directory is required")?,
                ))
                .is_some()
            {
                return Err("owner-local records directory was specified more than once");
            }
            continue;
        }
        if flag == OsStr::new("--artifact") {
            if selected_artifact.is_some() {
                return Err("evaluation artifact identifier was specified more than once");
            }
            let id = args
                .next()
                .and_then(|id| id.into_string().ok())
                .ok_or("evaluation artifact identifier is invalid")?;
            selected_artifact = Some(
                ARTIFACTS
                    .iter()
                    .copied()
                    .find(|artifact| artifact.id == id)
                    .ok_or("evaluation artifact identifier is invalid")?,
            );
            continue;
        }
        if flag == OsStr::new(SEND_ACKNOWLEDGEMENT) {
            if acknowledged {
                return Err("fixed corpus send acknowledgement was specified more than once");
            }
            acknowledged = true;
            continue;
        }
        return Err("unrecognized evaluation argument");
    }

    let records_dir = records_dir.ok_or("owner-local records directory is required")?;
    if records_dir.as_os_str().is_empty() {
        return Err("owner-local records directory is required");
    }
    if !acknowledged {
        return Err(SEND_AUTHORIZATION_REQUIRED);
    }
    Ok((records_dir, selected_artifact))
}

fn repository_root() -> Result<PathBuf, &'static str> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or("repository root is unavailable")
}

fn prepare_owner_local_directory(records_dir: &Path, repo: &Path) -> Result<(), &'static str> {
    let absolute = absolute_path(records_dir)?;
    let parent = absolute
        .parent()
        .ok_or("owner-local records directory has no parent")?;
    let parent =
        fs::canonicalize(parent).map_err(|_| "owner-local records parent is unavailable")?;
    let repo = fs::canonicalize(repo).map_err(|_| "repository root is unavailable")?;
    if parent.starts_with(&repo) {
        return Err("owner-local records directory must be outside the repository");
    }
    if absolute.exists() {
        return Err("owner-local records directory must not already exist");
    }
    fs::create_dir(&absolute).map_err(|_| "could not create owner-local records directory")
}

fn absolute_path(path: &Path) -> Result<PathBuf, &'static str> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map_err(|_| "current directory is unavailable")
            .map(|current| current.join(path))
    }
}

fn require_environment_credential() -> Result<(), &'static str> {
    env::var(Provider::OpenAiResponses.credential_environment_variable())
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|_| ())
        .ok_or("a nonempty OpenAI credential is required in the process environment")
}

fn execute_artifact(
    repo: &Path,
    settings: &AppSettings,
    vault: &CredentialVault,
    artifact: Artifact,
) -> Result<ReviewExecutionRecord, ArtifactExecutionFailure> {
    let manifest = verify_manifest(repo).map_err(|_| ArtifactExecutionFailure::Preflight)?;
    let request = build_request(repo, &manifest, artifact)
        .map_err(|_| ArtifactExecutionFailure::Preflight)?;
    let prepared = PreparedReview::from_settings(settings, vault)
        .map_err(ReviewExecutionError::new)
        .map_err(|error| ArtifactExecutionFailure::Review(Box::new(error)))?;
    let prepared = prepared
        .bind_document_request(request, ReviewLanguage::English)
        .map_err(ReviewExecutionError::new)
        .map_err(|error| ArtifactExecutionFailure::Review(Box::new(error)))?;
    let disclosure = prepared.disclosure().clone();
    let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
    let authorization = prepared
        .authorize(&mut consent)
        .map_err(ReviewExecutionError::new)
        .map_err(|error| ArtifactExecutionFailure::Review(Box::new(error)))?;
    prepared
        .execute_with_record(
            authorization,
            &std::sync::atomic::AtomicBool::new(false),
            mt_app::review::REVIEW_REQUEST_TIMEOUT,
        )
        .map_err(|error| ArtifactExecutionFailure::Review(Box::new(error)))
}

fn write_failed_record(
    artifact_dir: &Path,
    artifact: Artifact,
    failure: &ArtifactExecutionFailure,
) -> Result<(), &'static str> {
    let (provider_user_payload, raw_model_response) = match failure {
        ArtifactExecutionFailure::Preflight => (None, None),
        ArtifactExecutionFailure::Review(error) => {
            (error.user_payload(), error.raw_model_response())
        }
    };
    write_json_new(
        &artifact_dir.join("failure.json"),
        &json!({
            "schema": "markturbo-goal-06-owner-local-failure-v1",
            "artifact_id": artifact.id,
            "corpus_version": CORPUS_VERSION,
            "manifest_sha256": APPROVED_MANIFEST_SHA256,
            "configuration": fixed_configuration(),
            "status": "failed",
            "error_code": failure.code(),
            "provider_user_payload": provider_user_payload,
            "raw_model_response": raw_model_response,
        }),
    )
}

fn verify_manifest(repo: &Path) -> Result<Manifest, &'static str> {
    let manifest_path = repo.join(MANIFEST_PATH);
    let bytes = fs::read(manifest_path).map_err(|_| "evaluation manifest is unavailable")?;
    let manifest_sha256 = sha256(&bytes);
    if manifest_sha256 != APPROVED_MANIFEST_SHA256 {
        return Err("evaluation manifest digest does not match the approved corpus");
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "evaluation manifest is not UTF-8")?;
    let mut lines = text.lines();
    if lines.next() != Some("# corpus-version: goal-01-v1")
        || lines.next() != Some("# format: sha256  repository-relative-path")
    {
        return Err("evaluation manifest format is invalid");
    }
    let mut entries = BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let Some((digest, path)) = line.split_once("  ") else {
            return Err("evaluation manifest format is invalid");
        };
        if !is_sha256(digest)
            || !is_safe_corpus_path(path)
            || entries.insert(path.to_owned(), digest.to_owned()).is_some()
        {
            return Err("evaluation manifest format is invalid");
        }
    }
    if entries.len() != EXPECTED_MANIFEST_ENTRIES {
        return Err("evaluation manifest entry count is invalid");
    }
    for (relative, expected) in &entries {
        let path = repo.join(relative);
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| "evaluation corpus file is unavailable")?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("evaluation corpus file is not a regular file");
        }
        let bytes = fs::read(path).map_err(|_| "evaluation corpus file is unavailable")?;
        if sha256(&bytes) != *expected {
            return Err("evaluation corpus file digest does not match the manifest");
        }
    }
    Ok(Manifest { entries })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte <= b'f'))
}

fn is_safe_corpus_path(value: &str) -> bool {
    if !value.starts_with("evaluation/goal-01/") || value.contains('\\') {
        return false;
    }
    let path = Path::new(value);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn build_request(
    repo: &Path,
    manifest: &Manifest,
    artifact: Artifact,
) -> Result<ReviewRequest, &'static str> {
    match artifact.source {
        ArtifactSource::Document => {
            let content = read_verified_utf8(repo, manifest, artifact.path)?;
            ReviewRequest::new(
                artifact.lens,
                ReviewScope::Document,
                ReviewSource::document_at(content, artifact.path),
                SourceSnapshot::default(),
            )
            .map_err(|_| "evaluation document request is invalid")
        }
        ArtifactSource::AgentSkill => {
            let prefix = format!("{}/", artifact.path);
            let mut files = Vec::new();
            for relative in manifest
                .entries
                .keys()
                .filter(|path| path.starts_with(&prefix))
            {
                let content = read_verified_utf8(repo, manifest, relative)?;
                let package_path = relative
                    .strip_prefix(&prefix)
                    .ok_or("evaluation Agent Skill path is invalid")?;
                files.push(
                    SkillPackageFile::text(package_path, content, "complete Agent Skill snapshot")
                        .map_err(|_| "evaluation Agent Skill file is invalid")?,
                );
            }
            let package = SkillPackage::new(files, Vec::new())
                .map_err(|_| "evaluation Agent Skill package is invalid")?;
            ReviewRequest::agent_skill(package, SourceSnapshot::default())
                .map_err(|_| "evaluation Agent Skill request is invalid")
        }
    }
}

fn read_verified_utf8(
    repo: &Path,
    manifest: &Manifest,
    relative: &str,
) -> Result<String, &'static str> {
    let expected = manifest
        .entries
        .get(relative)
        .ok_or("evaluation artifact is not covered by the manifest")?;
    let bytes = fs::read(repo.join(relative)).map_err(|_| "evaluation artifact is unavailable")?;
    if sha256(&bytes) != *expected {
        return Err("evaluation artifact changed after manifest verification");
    }
    String::from_utf8(bytes).map_err(|_| "evaluation artifact is not UTF-8")
}

fn write_completed_record(
    artifact_dir: &Path,
    artifact: Artifact,
    record: &ReviewExecutionRecord,
) -> Result<(), &'static str> {
    let inspection = inspection_json(record)?;
    write_json_new(
        &artifact_dir.join("request.json"),
        &json!({
            "schema": "markturbo-goal-06-owner-local-request-v1",
            "artifact_id": artifact.id,
            "corpus_version": CORPUS_VERSION,
            "manifest_sha256": APPROVED_MANIFEST_SHA256,
            "configuration": fixed_configuration(),
            "request": record.request(),
            "provider_user_payload": record.user_payload(),
            "inspection": inspection,
        }),
    )?;
    let result = record.transport_result();
    write_json_new(
        &artifact_dir.join("result.json"),
        &json!({
            "schema": "markturbo-goal-06-owner-local-result-v1",
            "artifact_id": artifact.id,
            "corpus_version": CORPUS_VERSION,
            "manifest_sha256": APPROVED_MANIFEST_SHA256,
            "metadata": result.metadata,
            "raw_model_response": record.raw_model_response(),
            "validated_result": result.result,
        }),
    )
}

fn inspection_json(record: &ReviewExecutionRecord) -> Result<Value, &'static str> {
    let source_bytes = record.request().outbound_bytes();
    Ok(json!({
        "source_byte_size": source_bytes.len(),
        "canonical_byte_size": source_bytes.len(),
        "canonical_sha256": sha256(&source_bytes),
    }))
}

fn write_judgment_template(
    records_dir: &Path,
    artifact: Artifact,
    record: &ReviewExecutionRecord,
) -> Result<(), &'static str> {
    let question_count = record
        .transport_result()
        .result
        .output
        .as_ref()
        .map_or(0, |output| output.clarification_questions.len());
    write_json_new(
        &records_dir
            .join("owner-judgments")
            .join(format!("{}.json", artifact.id)),
        &json!({
            "artifact_id": artifact.id,
            "decoded_completely": true,
            "surfaced_item_ids": [],
            "unsupported_claim_ids": [],
            "unsupported_claim_count": 0,
            "false_source_anchor_count": 0,
            "boilerplate_question_count": 0,
            "question_count": question_count,
            "materially_misleading": false,
            "usefulness": "not_useful",
            "model_reported_id": record.transport_result().metadata.response_model(),
        }),
    )
}

fn fixed_configuration() -> Value {
    json!({
        "provider_wire_format": "openai-responses",
        "endpoint": ENDPOINT,
        "model_requested": MODEL,
        "reasoning_effort": "medium",
        "sampling": "provider-defaults-omitted",
        "prompt_version": "review-v1",
        "tools": false,
        "browsing": false,
        "memory": false,
        "agent_actions": false,
    })
}

fn write_json_new(path: &Path, value: &Value) -> Result<(), &'static str> {
    let parent = path
        .parent()
        .ok_or("owner-local record parent is unavailable")?;
    fs::create_dir_all(parent).map_err(|_| "could not create owner-local record directory")?;
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| "could not serialize owner-local record")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "could not create owner-local record")?;
    file.write_all(&bytes)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|_| "could not write owner-local record")
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_app::credentials::CredentialSource;

    #[test]
    fn fixed_artifact_set_has_every_required_lens() {
        assert_eq!(ARTIFACTS.len(), 12);
        assert_eq!(
            ARTIFACTS
                .iter()
                .filter(|artifact| matches!(artifact.source, ArtifactSource::AgentSkill))
                .count(),
            2
        );
        assert!(
            ARTIFACTS
                .iter()
                .all(|artifact| artifact.path.starts_with("evaluation/goal-01/"))
        );
    }

    #[test]
    fn corpus_paths_reject_traversal_and_noncanonical_separators() {
        assert!(is_safe_corpus_path(
            "evaluation/goal-01/task-prompts/example.md"
        ));
        assert!(!is_safe_corpus_path("evaluation/goal-01/../outside.md"));
        assert!(!is_safe_corpus_path("evaluation\\goal-01\\example.md"));
        assert!(!is_safe_corpus_path("C:/evaluation/goal-01/example.md"));
    }

    #[test]
    fn corpus_send_requires_explicit_acknowledgement() {
        let arguments = [
            std::ffi::OsString::from("--records-dir"),
            std::ffi::OsString::from("C:/owner-local-records"),
        ];
        assert!(matches!(
            parse_arguments_from(arguments),
            Err(SEND_AUTHORIZATION_REQUIRED)
        ));
    }

    #[test]
    fn authorization_text_binds_the_exact_fixed_endpoint_and_scope() {
        for required in [
            "goal-01-v1",
            "http://127.0.0.1:4141/v1/",
            "loopback HTTP",
            "unencrypted",
            "no proxy",
            "gpt-5.6-terra",
            "reasoning effort medium",
            "provider-default sampling",
            "no tools, browsing, memory, or agent actions",
        ] {
            assert!(SEND_AUTHORIZATION_REQUIRED.contains(required));
        }
        assert!(SEND_ACKNOWLEDGEMENT.contains("http-127-0-0-1-4141-v1"));
    }

    #[test]
    fn acknowledgement_allows_fixed_artifact_selection_in_any_flag_order() {
        let arguments = [
            std::ffi::OsString::from(SEND_ACKNOWLEDGEMENT),
            std::ffi::OsString::from("--artifact"),
            std::ffi::OsString::from("AS-01"),
            std::ffi::OsString::from("--records-dir"),
            std::ffi::OsString::from("C:/owner-local-records"),
        ];
        let (records_dir, artifact) = parse_arguments_from(arguments).unwrap();
        assert_eq!(records_dir, PathBuf::from("C:/owner-local-records"));
        assert_eq!(artifact.unwrap().id, "AS-01");
    }

    #[test]
    fn previous_unscoped_acknowledgement_does_not_authorize_a_send() {
        let arguments = [
            std::ffi::OsString::from("--records-dir"),
            std::ffi::OsString::from("C:/owner-local-records"),
            std::ffi::OsString::from("--confirm-send-goal-01-v1"),
        ];
        assert!(matches!(
            parse_arguments_from(arguments),
            Err("unrecognized evaluation argument")
        ));
    }

    #[test]
    fn credential_and_endpoint_arguments_are_not_accepted() {
        let arguments = [
            std::ffi::OsString::from("--records-dir"),
            std::ffi::OsString::from("C:/owner-local-records"),
            std::ffi::OsString::from(SEND_ACKNOWLEDGEMENT),
            std::ffi::OsString::from("--endpoint"),
            std::ffi::OsString::from("http://other.example/v1/"),
        ];
        assert!(matches!(
            parse_arguments_from(arguments),
            Err("unrecognized evaluation argument")
        ));
    }

    #[test]
    fn environment_only_store_cannot_prefer_a_persistent_credential() {
        let vault = CredentialVault::with_store(Arc::new(EnvironmentOnlyStore));
        let credential = vault
            .resolve(
                "fixed-evaluation-target",
                Some("environment-only".to_owned()),
                true,
            )
            .unwrap()
            .unwrap();
        assert!(!vault.secure_store_supported());
        assert_eq!(credential.source(), CredentialSource::Environment);
    }
}
