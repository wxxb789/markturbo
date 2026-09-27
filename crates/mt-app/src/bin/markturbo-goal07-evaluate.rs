//! Build an offline, metadata-only machine receipt for one Goal 07 Revision capture.
//!
//! This binary intentionally has no provider, credential, endpoint, or network
//! boundary. It consumes owner-local capture data, reuses the production
//! Review and Revision validators, and writes only digests, sizes, ranges, and
//! other machine facts to a create-new receipt.

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt as _;

use mt_app::review::{
    RevisionAnswer, RevisionAnswers, RevisionQuestionCoverageStatus, decode_review_output_capture,
    decode_revision_capture,
};
use mt_doc::review::{ArtifactLens, ReviewRequest, ReviewSource, SkillFilePayload};
use mt_doc::revision::{ChangeId, RevisionProposal};
use serde::Deserialize;
use serde::Deserializer as _;
use serde::de::{self, DeserializeOwned, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const INPUT_SCHEMA: &str = "markturbo-goal-07-revision-capture-v2";
const RECEIPT_SCHEMA: &str = "markturbo-goal-07-machine-receipt-v2";
const RUNNER_SCHEMA: &str = "markturbo-local-diff-receipt-v2";
const CORPUS_VERSION: &str = "goal-01-v1";
const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
const MAX_DECISION_BYTES: usize = 256 * 1024;

#[derive(Debug)]
struct Arguments {
    inputs: Vec<PathBuf>,
    receipt: PathBuf,
    runner_sha256: String,
    expected_runner_sha256: Option<String>,
    decisions: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureInput {
    schema: String,
    corpus_version: String,
    manifest_sha256: String,
    corpus_artifact: CorpusArtifact,
    request: Value,
    review_output: Value,
    answers: Vec<AnswerWire>,
    raw_revision_response: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusArtifact {
    artifact_id: String,
    path: String,
    lens: String,
    sha256: String,
    byte_count: u64,
    files: Vec<CorpusArtifactFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusArtifactFile {
    path: String,
    sha256: String,
    byte_count: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerWire {
    state: String,
    #[serde(default)]
    answer: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionFile {
    schema: String,
    artifact_id: String,
    proposal_sha256: String,
    editable_source_sha256: String,
    editable_source_byte_count: u64,
    source_binding_sha256: String,
    source_revision: u64,
    source_generation: u64,
    artifact_lens_sha256: String,
    review_context_sha256: String,
    answers_sha256: String,
    decisions: Vec<DecisionWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionWire {
    change_id: u32,
    accepted: bool,
    intent_change_ids: Vec<String>,
}

fn main() -> ExitCode {
    if env::args_os()
        .skip(1)
        .any(|argument| argument == "--help" || argument == "-h")
    {
        print_help();
        return ExitCode::SUCCESS;
    }

    match run() {
        Ok(receipt_sha256) => {
            println!("GOAL07_MACHINE_RECEIPT_SHA256={receipt_sha256}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("Goal 07 machine evaluation did not complete: {message}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "usage: markturbo-goal07-evaluate --input <capture.json> --receipt <new-receipt.json> --runner-sha256 <sha256> [--expect-runner-sha256 <sha256>] [--decisions <owner-decisions.json> ...]\n\n\
         Reads one private, owner-local Goal 07 Revision capture completely offline.\n\
         The runner hash must be measured externally and supplied explicitly; this\n\
         portable binary does not self-hash. The create-new receipt contains only\n\
         machine metadata and digests, never source, answers, rationale, paths, or\n\
         owner intent judgments. Repeatable decisions files are matched by\n\
         artifact_id; a missing file leaves that case uncomposed. Each file\n\
         must carry the proposal/source/snapshot binding emitted with the same\n\
         capture, and approved output records only opaque digests."
    );
}

fn run() -> Result<String, String> {
    let arguments = parse_arguments(env::args_os().skip(1))?;
    validate_digest(&arguments.runner_sha256, "runner SHA-256", false)?;
    if let Some(expected) = &arguments.expected_runner_sha256 {
        validate_digest(expected, "expected runner SHA-256", false)?;
        if expected != &arguments.runner_sha256 {
            return Err("runner SHA-256 does not match the expected external anchor".to_owned());
        }
    }
    let captures = arguments
        .inputs
        .iter()
        .map(|path| {
            read_private_file(path, MAX_CAPTURE_BYTES, "capture input")
                .and_then(|bytes| parse_capture(&bytes))
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_capture_set(&captures)?;
    let decisions = read_decisions_for_captures(&arguments.decisions, &captures)?;
    let receipt = build_receipt(&captures, &arguments.runner_sha256, &decisions)?;
    let receipt_bytes = serde_json::to_vec_pretty(&receipt)
        .map_err(|_| "could not serialize machine receipt".to_owned())?;
    write_create_new(&arguments.receipt, &receipt_bytes)?;
    let mut file_bytes = receipt_bytes;
    file_bytes.push(b'\n');
    Ok(sha256_hex(&file_bytes))
}

fn parse_arguments(arguments: impl IntoIterator<Item = OsString>) -> Result<Arguments, String> {
    let mut input = Vec::new();
    let mut receipt = None;
    let mut runner_sha256 = None;
    let mut expected_runner_sha256 = None;
    let mut decisions = Vec::new();
    let mut values = arguments.into_iter();

    while let Some(flag) = values.next() {
        let flag_text = flag
            .to_str()
            .ok_or_else(|| "command-line argument is not valid UTF-8".to_owned())?;
        match flag_text {
            "--input" => push_path(&mut input, values.next(), "input")?,
            "--receipt" | "--output" => set_path(&mut receipt, values.next(), "receipt")?,
            "--runner-sha256" => {
                if runner_sha256.is_some() {
                    return Err("runner SHA-256 was specified more than once".to_owned());
                }
                runner_sha256 = Some(
                    values
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .ok_or_else(|| "runner SHA-256 is required".to_owned())?,
                );
            }
            "--expect-runner-sha256" => {
                if expected_runner_sha256.is_some() {
                    return Err("expected runner SHA-256 was specified more than once".to_owned());
                }
                expected_runner_sha256 = Some(
                    values
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .ok_or_else(|| "expected runner SHA-256 is required".to_owned())?,
                );
            }
            "--decisions" => push_path(&mut decisions, values.next(), "decisions")?,
            "--help" | "-h" => return Err("help requested".to_owned()),
            _ => return Err("unrecognized evaluation argument".to_owned()),
        }
    }

    Ok(Arguments {
        inputs: if input.is_empty() {
            return Err("capture input is required".to_owned());
        } else {
            input
        },
        receipt: receipt.ok_or_else(|| "new receipt path is required".to_owned())?,
        runner_sha256: runner_sha256.ok_or_else(|| "runner SHA-256 is required".to_owned())?,
        expected_runner_sha256,
        decisions,
    })
}

fn set_path(
    slot: &mut Option<PathBuf>,
    value: Option<OsString>,
    label: &str,
) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{label} path was specified more than once"));
    }
    let value = value.ok_or_else(|| format!("{label} path is required"))?;
    if value.is_empty() {
        return Err(format!("{label} path is required"));
    }
    *slot = Some(PathBuf::from(value));
    Ok(())
}

fn push_path(slot: &mut Vec<PathBuf>, value: Option<OsString>, label: &str) -> Result<(), String> {
    let value = value.ok_or_else(|| format!("{label} path is required"))?;
    if value.is_empty() {
        return Err(format!("{label} path is required"));
    }
    slot.push(PathBuf::from(value));
    Ok(())
}

struct RejectDuplicateKeys;

impl<'de> DeserializeSeed<'de> for RejectDuplicateKeys {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for RejectDuplicateKeys {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_string<E>(self, _value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element_seed(RejectDuplicateKeys)?.is_some() {}
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = std::collections::BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            map.next_value_seed(RejectDuplicateKeys)?;
        }
        Ok(())
    }
}

fn parse_json<T: DeserializeOwned>(bytes: &[u8], error: &str) -> Result<T, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    deserializer
        .deserialize_any(RejectDuplicateKeys)
        .map_err(|_| error.to_owned())?;
    deserializer.end().map_err(|_| error.to_owned())?;
    serde_json::from_slice(bytes).map_err(|_| error.to_owned())
}

fn parse_capture(bytes: &[u8]) -> Result<ParsedCapture, String> {
    let input: CaptureInput =
        parse_json(bytes, "capture input is not a valid Goal 07 JSON object")?;
    if input.schema != INPUT_SCHEMA {
        return Err("capture input schema is unsupported".to_owned());
    }
    if input.corpus_version != CORPUS_VERSION {
        return Err("capture corpus version is unsupported".to_owned());
    }
    validate_digest(&input.manifest_sha256, "manifest SHA-256", false)?;
    validate_stable_id(&input.corpus_artifact.artifact_id, "artifact identifier")?;
    validate_digest(
        &input.corpus_artifact.sha256,
        "corpus artifact SHA-256",
        false,
    )?;

    let request_json = serde_json::to_string(&input.request)
        .map_err(|_| "capture Review request is not serializable".to_owned())?;
    let request = ReviewRequest::decode_json(&request_json)
        .map_err(|_| "capture Review request failed production validation".to_owned())?;
    if input.corpus_artifact.lens != corpus_lens(request.lens) {
        return Err("capture corpus artifact lens does not match the Review request".to_owned());
    }

    let review_json = serde_json::to_string(&input.review_output)
        .map_err(|_| "capture Review output is not serializable".to_owned())?;
    let review_output = decode_review_output_capture(&review_json, &request)
        .map_err(|_| "capture Review output failed production validation".to_owned())?;

    if input.raw_revision_response.len() > mt_app::review::REVISION_MAX_DECODED_RESPONSE_BYTES {
        return Err("capture Revision response exceeds the production bound".to_owned());
    }
    let answers = parse_answers(input.answers)?;
    let revision = decode_revision_capture(
        &request,
        &review_output,
        &answers,
        &input.raw_revision_response,
    )
    .map_err(|_| "capture Revision response failed production validation".to_owned())?;
    let (request_artifact_sha256, request_artifact_byte_count) = request_artifact_binding(
        &request,
        &input.corpus_artifact,
        revision.proposal().source(),
    )?;

    Ok(ParsedCapture {
        corpus_version: input.corpus_version,
        manifest_sha256: input.manifest_sha256,
        corpus_artifact: input.corpus_artifact,
        request,
        answers,
        raw_revision_response: input.raw_revision_response,
        revision,
        request_artifact_sha256,
        request_artifact_byte_count,
    })
}

struct ParsedCapture {
    corpus_version: String,
    manifest_sha256: String,
    corpus_artifact: CorpusArtifact,
    request: ReviewRequest,
    answers: RevisionAnswers,
    raw_revision_response: String,
    revision: mt_app::review::ValidatedRevisionCapture,
    request_artifact_sha256: String,
    request_artifact_byte_count: u64,
}

fn validate_corpus_relative_path(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.contains('\\')
        || value.starts_with('/')
        || !value.starts_with("evaluation/goal-01/")
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("{field} is not a normalized corpus-relative path"));
    }
    Ok(())
}

fn decode_sha256(value: &str, field: &str) -> Result<[u8; 32], String> {
    validate_digest(value, field, false)?;
    let mut bytes = [0_u8; 32];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        bytes[index] = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
    }
    Ok(bytes)
}

fn hex_value(value: u8) -> Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err("digest contains a non-hexadecimal byte".to_owned()),
    }
}

fn corpus_artifact_digest(artifact: &CorpusArtifact) -> Result<String, String> {
    if artifact.files.is_empty() {
        return Err("corpus artifact file metadata is empty".to_owned());
    }
    validate_corpus_relative_path(&artifact.path, "corpus artifact path")?;
    let is_single_file = artifact.files.len() == 1 && artifact.files[0].path == artifact.path;
    let prefix = format!("{}/", artifact.path);
    for file in &artifact.files {
        validate_corpus_relative_path(&file.path, "corpus artifact file path")?;
        if !is_single_file && !file.path.starts_with(&prefix) {
            return Err("corpus artifact file escapes its declared root".to_owned());
        }
    }
    let digest = corpus_artifact_files_digest(&artifact.files)?;
    let byte_count = corpus_artifact_metadata_byte_count(&artifact.files)?;
    if byte_count != artifact.byte_count {
        return Err("corpus artifact byte count does not match its file metadata".to_owned());
    }
    if digest != artifact.sha256 {
        return Err("corpus artifact digest does not match its file metadata".to_owned());
    }
    Ok(digest)
}

fn corpus_artifact_files_digest(files: &[CorpusArtifactFile]) -> Result<String, String> {
    let mut previous: Option<&str> = None;
    let mut digest = Sha256::new();
    for file in files {
        validate_corpus_relative_path(&file.path, "corpus artifact file path")?;
        if let Some(previous) = previous
            && file.path.as_str() <= previous
        {
            return Err("corpus artifact files must be sorted and duplicate-free".to_owned());
        }
        previous = Some(&file.path);
        let sha = decode_sha256(&file.sha256, "corpus artifact file SHA-256")?;
        let path_bytes = file.path.as_bytes();
        digest.update((path_bytes.len() as u64).to_be_bytes());
        digest.update(path_bytes);
        digest.update(file.byte_count.to_be_bytes());
        digest.update(sha);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn corpus_artifact_metadata_byte_count(files: &[CorpusArtifactFile]) -> Result<u64, String> {
    let mut byte_count = 0_u64;
    for file in files {
        byte_count = byte_count
            .checked_add(file.byte_count)
            .ok_or_else(|| "corpus artifact byte count overflow".to_owned())?;
    }
    Ok(byte_count)
}

fn skill_file_digest(file: &mt_doc::review::SkillPackageFile) -> (String, u64) {
    match &file.payload {
        SkillFilePayload::Utf8 { content } => {
            (sha256_hex(content.as_bytes()), content.len() as u64)
        }
        SkillFilePayload::RawBinary { bytes, .. } => (sha256_hex(bytes), bytes.len() as u64),
        SkillFilePayload::Binary { sha256 } => (sha256.clone(), file.byte_size),
    }
}

fn request_artifact_binding(
    request: &ReviewRequest,
    artifact: &CorpusArtifact,
    editable_source: &str,
) -> Result<(String, u64), String> {
    let artifact_digest = corpus_artifact_digest(artifact)?;
    match &request.source {
        ReviewSource::Document { .. } => {
            if artifact.files.len() != 1 {
                return Err("document capture must bind exactly one corpus file".to_owned());
            }
            let file = &artifact.files[0];
            if sha256_hex(editable_source.as_bytes()) != file.sha256
                || editable_source.len() as u64 != file.byte_count
            {
                return Err("editable document source does not match the corpus file".to_owned());
            }
        }
        ReviewSource::AgentSkillPackage { package } => {
            let prefix = format!("{}/", artifact.path);
            let mut matched = 0_usize;
            let mut has_entrypoint = false;
            for file in &artifact.files {
                let package_path = file.path.strip_prefix(&prefix).ok_or_else(|| {
                    "Agent Skill corpus path is not rooted at its package path".to_owned()
                })?;
                let package_file = package
                    .files()
                    .iter()
                    .find(|candidate| candidate.path == package_path)
                    .ok_or_else(|| "Agent Skill request is missing a corpus file".to_owned())?;
                let (actual_sha, actual_count) = skill_file_digest(package_file);
                if actual_sha != file.sha256 || actual_count != file.byte_count {
                    return Err(
                        "Agent Skill request file does not match the corpus file".to_owned()
                    );
                }
                matched += 1;
                if package_path == "SKILL.md" {
                    has_entrypoint = true;
                }
            }
            if matched != package.files().len() || !has_entrypoint {
                return Err(
                    "Agent Skill request must cover every corpus file and SKILL.md".to_owned(),
                );
            }
            let entrypoint = package
                .files()
                .iter()
                .find(|file| file.path == "SKILL.md")
                .ok_or_else(|| "Agent Skill request is missing SKILL.md".to_owned())?;
            let (entrypoint_sha, entrypoint_count) = skill_file_digest(entrypoint);
            if entrypoint_sha != sha256_hex(editable_source.as_bytes())
                || entrypoint_count != editable_source.len() as u64
            {
                return Err("Agent Skill editable source does not match SKILL.md".to_owned());
            }
        }
    }
    Ok((artifact_digest, artifact.byte_count))
}

fn parse_answers(values: Vec<AnswerWire>) -> Result<RevisionAnswers, String> {
    let answers = values
        .into_iter()
        .map(|answer| match answer.state.as_str() {
            "unanswered" if answer.answer.is_none() => Ok(RevisionAnswer::unanswered()),
            "intentionally_unspecified" if answer.answer.is_none() => {
                Ok(RevisionAnswer::intentionally_unspecified())
            }
            "answered" => answer
                .answer
                .filter(|value| !value.trim().is_empty())
                .map(RevisionAnswer::answered)
                .ok_or_else(|| "answered Revision state is missing text".to_owned()),
            _ => Err("Revision answer state is invalid".to_owned()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    RevisionAnswers::new(answers)
        .map_err(|_| "Revision answers failed production validation".to_owned())
}

struct ParsedDecisions {
    raw_sha256: String,
    decision_set_sha256: String,
    decisions: Vec<(ChangeId, bool)>,
    intent_change_ids: BTreeMap<ChangeId, Vec<String>>,
}

fn read_decisions_for_captures(
    paths: &[PathBuf],
    captures: &[ParsedCapture],
) -> Result<BTreeMap<String, ParsedDecisions>, String> {
    let mut result = BTreeMap::new();
    for path in paths {
        let bytes = read_private_file(path, MAX_DECISION_BYTES, "owner decisions")?;
        let decision_file: DecisionFile =
            parse_json(&bytes, "owner decision file is not valid machine JSON")?;
        let artifact_id = decision_file.artifact_id.clone();
        let capture = captures
            .iter()
            .find(|capture| capture.corpus_artifact.artifact_id == artifact_id)
            .ok_or_else(|| {
                "owner decision artifact is not present in the capture set".to_owned()
            })?;
        if result.contains_key(&artifact_id) {
            return Err("owner decision files contain a duplicate artifact identifier".to_owned());
        }
        let decisions = validate_decision_file(&bytes, decision_file, capture)?;
        result.insert(artifact_id, decisions);
    }
    Ok(result)
}

fn validate_decision_file(
    bytes: &[u8],
    decision_file: DecisionFile,
    capture: &ParsedCapture,
) -> Result<ParsedDecisions, String> {
    if decision_file.schema != "markturbo-goal-07-owner-composition-v2" {
        return Err("owner decision file schema is unsupported".to_owned());
    }
    if decision_file.artifact_id != capture.corpus_artifact.artifact_id {
        return Err("owner decision artifact does not match the capture".to_owned());
    }
    validate_digest(
        &decision_file.proposal_sha256,
        "owner decision proposal SHA-256",
        false,
    )?;
    validate_digest(
        &decision_file.editable_source_sha256,
        "owner decision editable source SHA-256",
        false,
    )?;
    validate_digest(
        &decision_file.source_binding_sha256,
        "owner decision source binding SHA-256",
        false,
    )?;
    validate_digest(
        &decision_file.artifact_lens_sha256,
        "owner decision artifact lens SHA-256",
        false,
    )?;
    validate_digest(
        &decision_file.review_context_sha256,
        "owner decision Review context SHA-256",
        false,
    )?;
    validate_digest(
        &decision_file.answers_sha256,
        "owner decision answers SHA-256",
        false,
    )?;
    let proposal_sha256 = proposal_digest(
        capture.revision.proposal(),
        capture.revision.question_coverage(),
    )?;
    if decision_file.proposal_sha256 != proposal_sha256 {
        return Err("owner decisions are bound to a different proposal".to_owned());
    }
    let proposal_source = capture.revision.proposal().source();
    let binding = capture.revision.binding();
    let editable_source_sha256 = sha256_hex(proposal_source.as_bytes());
    let source_binding_sha256 = hex_digest(binding.source_sha256());
    if decision_file.editable_source_sha256 != editable_source_sha256
        || decision_file.editable_source_byte_count != proposal_source.len() as u64
        || decision_file.source_binding_sha256 != source_binding_sha256
    {
        return Err("owner decisions are bound to a different editable source".to_owned());
    }
    if decision_file.source_revision != binding.source_revision()
        || decision_file.source_generation != binding.source_generation()
        || decision_file.artifact_lens_sha256 != hex_digest(binding.artifact_lens_digest())
        || decision_file.review_context_sha256 != hex_digest(binding.review_context_digest())
        || decision_file.answers_sha256 != hex_digest(binding.answers_digest())
    {
        return Err("owner decisions are bound to a different Revision request".to_owned());
    }

    let expected_ids = capture
        .revision
        .proposal()
        .hunks()
        .iter()
        .map(|hunk| hunk.change_id())
        .collect::<std::collections::BTreeSet<_>>();
    let mut seen = std::collections::BTreeSet::new();
    let mut decisions = Vec::with_capacity(decision_file.decisions.len());
    let mut intent_change_ids = BTreeMap::new();
    for decision in decision_file.decisions {
        let id = ChangeId(decision.change_id);
        if !expected_ids.contains(&id) || !seen.insert(id) {
            return Err("owner decisions do not cover the validated change groups".to_owned());
        }
        let mut previous_intent_id: Option<&str> = None;
        for intent_id in &decision.intent_change_ids {
            validate_stable_id(intent_id, "owner intent ChangeId")?;
            if let Some(previous) = previous_intent_id
                && intent_id.as_str() <= previous
            {
                return Err("owner intent_change_ids must be sorted and duplicate-free".to_owned());
            }
            previous_intent_id = Some(intent_id);
        }
        intent_change_ids.insert(id, decision.intent_change_ids);
        decisions.push((id, decision.accepted));
    }
    if seen != expected_ids {
        return Err("owner decisions do not cover every validated change group".to_owned());
    }
    decisions.sort_by_key(|(id, _)| *id);
    Ok(ParsedDecisions {
        raw_sha256: sha256_hex(bytes),
        decision_set_sha256: decision_set_digest(&decisions)?,
        decisions,
        intent_change_ids,
    })
}

fn validate_capture_set(captures: &[ParsedCapture]) -> Result<(), String> {
    let first = captures
        .first()
        .ok_or_else(|| "at least one capture input is required".to_owned())?;
    let mut artifact_ids = std::collections::BTreeSet::new();
    for capture in captures {
        if capture.corpus_version != first.corpus_version
            || capture.manifest_sha256 != first.manifest_sha256
        {
            return Err("capture inputs do not share one corpus identity".to_owned());
        }
        if !artifact_ids.insert(capture.corpus_artifact.artifact_id.as_str()) {
            return Err("capture inputs contain a duplicate artifact identifier".to_owned());
        }
    }
    Ok(())
}

fn build_receipt(
    captures: &[ParsedCapture],
    runner_sha256: &str,
    decisions: &BTreeMap<String, ParsedDecisions>,
) -> Result<Value, String> {
    let first = captures
        .first()
        .ok_or_else(|| "at least one capture input is required".to_owned())?;
    let cases = captures
        .iter()
        .map(|capture| {
            let case_decisions = decisions.get(&capture.corpus_artifact.artifact_id);
            Ok((
                capture.corpus_artifact.artifact_id.clone(),
                build_case(capture, case_decisions)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(json!({
        "schema": RECEIPT_SCHEMA,
        "corpus_version": first.corpus_version,
        "manifest_sha256": first.manifest_sha256,
        "runner_schema": RUNNER_SCHEMA,
        "runner_executable_sha256": runner_sha256,
        "cases": cases,
    }))
}

fn build_case(
    capture: &ParsedCapture,
    decisions: Option<&ParsedDecisions>,
) -> Result<Value, String> {
    let proposal = capture.revision.proposal();
    let source = proposal.source();
    let editable_source_sha256 = sha256_hex(source.as_bytes());
    let editable_source_byte_count = source.len();
    let review_scope_bytes = serde_json::to_vec(&capture.request)
        .map_err(|_| "capture Review request is not serializable".to_owned())?;
    let review_scope_sha256 = sha256_hex(&review_scope_bytes);
    let review_scope_byte_count = review_scope_bytes.len();
    let review_context_sha256 = hex_digest(capture.revision.binding().review_context_digest());
    let answers_sha256 = hex_digest(capture.revision.binding().answers_digest());
    let proposal_sha256 = proposal_digest(proposal, capture.revision.question_coverage())?;
    let binding = capture.revision.binding();

    let (changes, displayed_diff_sha256) = build_diff_metadata(
        &capture.corpus_artifact.artifact_id,
        proposal,
        decisions.map(|value| &value.intent_change_ids),
    )?;
    let question_coverage = build_question_coverage(capture.revision.question_coverage());
    let (
        answered_material_question_ids,
        represented_question_ids,
        intentionally_omitted_question_ids,
    ) = question_id_lists(capture);
    let reject_all = json!({
        "source_sha256": editable_source_sha256,
        "result_sha256": sha256_hex(proposal.reject_all().as_bytes()),
        "source_byte_count": editable_source_byte_count,
        "result_byte_count": proposal.reject_all().len(),
    });
    let approved_output =
        build_approved_output(proposal, capture.revision.question_coverage(), decisions)?;

    Ok(json!({
        "artifact_id": capture.corpus_artifact.artifact_id,
        "corpus_artifact_lens": capture.corpus_artifact.lens,
        "corpus_artifact_sha256": capture.corpus_artifact.sha256,
        "corpus_artifact_byte_count": capture.corpus_artifact.byte_count,
        "request_artifact_sha256": capture.request_artifact_sha256,
        "request_artifact_byte_count": capture.request_artifact_byte_count,
        "review_scope_sha256": review_scope_sha256,
        "review_scope_byte_count": review_scope_byte_count,
        "editable_source_sha256": editable_source_sha256,
        "editable_source_byte_count": editable_source_byte_count,
        "source_binding_sha256": hex_digest(binding.source_sha256()),
        "review_context_sha256": review_context_sha256,
        "answers_sha256": answers_sha256,
        "raw_revision_response_sha256": sha256_hex(capture.raw_revision_response.as_bytes()),
        "proposal_sha256": proposal_sha256,
        "source_revision": binding.source_revision(),
        "source_generation": binding.source_generation(),
        "artifact_lens_sha256": hex_digest(binding.artifact_lens_digest()),
        "displayed_diff_sha256": displayed_diff_sha256,
        "changes": changes,
        "answered_material_question_ids": answered_material_question_ids,
        "represented_question_ids": represented_question_ids,
        "intentionally_omitted_question_ids": intentionally_omitted_question_ids,
        "question_coverage": question_coverage,
        "reject_all": reject_all,
        "approved_output": approved_output,
    }))
}

fn build_diff_metadata(
    artifact_id: &str,
    proposal: &RevisionProposal,
    intent_change_ids: Option<&BTreeMap<ChangeId, Vec<String>>>,
) -> Result<(Vec<Value>, String), String> {
    let mut groups: BTreeMap<ChangeId, Vec<Value>> = BTreeMap::new();
    let mut hunk_digests = Vec::with_capacity(proposal.hunks().len());
    let mut hunk_ordinals: BTreeMap<ChangeId, usize> = BTreeMap::new();

    for hunk in proposal.hunks() {
        let ordinal = hunk_ordinals.entry(hunk.change_id()).or_insert(0);
        let change_id = format!("{}-CH-{:02}", artifact_id, hunk.change_id().0 + 1);
        let hunk_id = format!("{}-H-{:02}", change_id, *ordinal + 1);
        *ordinal += 1;
        let replacement_sha256 = sha256_hex(hunk.replacement().as_bytes());
        let rationale_sha256 = sha256_hex(hunk.rationale().as_bytes());
        let source_range = hunk.source();
        let display_digest = digest_json(&json!({
            "hunk_id": hunk_id,
            "source_start": source_range.start,
            "source_end": source_range.end,
            "replacement_sha256": replacement_sha256,
            "replacement_byte_count": hunk.replacement().len(),
            "rationale_sha256": rationale_sha256,
            "rationale_byte_count": hunk.rationale().len(),
            "runner_schema": RUNNER_SCHEMA,
            "utf8_boundaries_verified": true,
        }))?;
        let hunk_value = json!({
            "hunk_id": hunk_id,
            "source_start": source_range.start,
            "source_end": source_range.end,
            "replacement_sha256": replacement_sha256,
            "replacement_byte_count": hunk.replacement().len(),
            "rationale_sha256": rationale_sha256,
            "rationale_byte_count": hunk.rationale().len(),
            "displayed_diff_sha256": display_digest,
            "runner_schema": RUNNER_SCHEMA,
            "local_diff_verified": true,
            "utf8_boundaries_verified": true,
        });
        hunk_digests.push(display_digest);
        groups.entry(hunk.change_id()).or_default().push(hunk_value);
    }

    let changes = groups
        .into_iter()
        .map(|(change_id, hunks)| {
            json!({
                "change_id": format!("{}-CH-{:02}", artifact_id, change_id.0 + 1),
                "intent_change_ids": intent_change_ids
                    .and_then(|mapping| mapping.get(&change_id))
                    .cloned()
                    .unwrap_or_default(),
                "hunks": hunks,
            })
        })
        .collect::<Vec<_>>();
    let displayed_diff_sha256 = digest_json(&json!({
        "runner_schema": RUNNER_SCHEMA,
        "changes": changes,
        "hunk_digests": hunk_digests,
    }))?;
    Ok((changes, displayed_diff_sha256))
}

fn build_question_coverage(coverage: &[mt_app::review::RevisionQuestionCoverage]) -> Vec<Value> {
    coverage
        .iter()
        .map(|item| {
            let status = match item.status() {
                RevisionQuestionCoverageStatus::Represented { change_ids } => json!({
                    "kind": "represented",
                    "change_ids": change_ids.iter().map(|id| id.0).collect::<Vec<_>>(),
                }),
                RevisionQuestionCoverageStatus::IntentionallyOmitted { reason } => json!({
                    "kind": "intentionally_omitted",
                    "reason_sha256": sha256_hex(reason.as_bytes()),
                    "reason_byte_count": reason.len(),
                }),
                RevisionQuestionCoverageStatus::NotAddressed => json!({
                    "kind": "not_addressed",
                }),
            };
            json!({
                "question_index": item.question_index(),
                "question_id": item.question_id(),
                "status": status,
            })
        })
        .collect()
}

fn question_id_lists(capture: &ParsedCapture) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut answered = Vec::new();
    let mut represented = Vec::new();
    let mut intentionally_omitted = Vec::new();
    for coverage in capture.revision.question_coverage() {
        match coverage.status() {
            RevisionQuestionCoverageStatus::Represented { .. } => {
                represented.push(coverage.question_id().to_owned());
            }
            RevisionQuestionCoverageStatus::IntentionallyOmitted { .. } => {
                intentionally_omitted.push(coverage.question_id().to_owned());
            }
            RevisionQuestionCoverageStatus::NotAddressed => {}
        }
        if matches!(
            capture.answers.as_slice().get(coverage.question_index()),
            Some(RevisionAnswer::Answered(_))
        ) {
            answered.push(coverage.question_id().to_owned());
        }
    }
    answered.sort();
    represented.sort();
    intentionally_omitted.sort();
    (answered, represented, intentionally_omitted)
}

fn corpus_lens(lens: ArtifactLens) -> &'static str {
    match lens {
        ArtifactLens::Prompt => "Task prompt",
        ArtifactLens::Specification | ArtifactLens::Plan => "Specification / plan",
        ArtifactLens::AgentInstructions => "Agent instructions",
        ArtifactLens::AgentSkill => "Agent Skill",
    }
}

fn decision_set_digest(decisions: &[(ChangeId, bool)]) -> Result<String, String> {
    digest_json(&json!({
        "decisions": decisions
            .iter()
            .map(|(change_id, accepted)| {
                json!({"change_id": change_id.0, "accepted": accepted})
            })
            .collect::<Vec<_>>(),
    }))
}

fn proposal_digest(
    proposal: &RevisionProposal,
    coverage: &[mt_app::review::RevisionQuestionCoverage],
) -> Result<String, String> {
    let hunks = proposal
        .hunks()
        .iter()
        .map(|hunk| {
            let range = hunk.source();
            json!({
                "change_id": hunk.change_id().0,
                "source_start": range.start,
                "source_end": range.end,
                "expected_source_sha256": sha256_hex(hunk.expected_source().as_bytes()),
                "replacement_sha256": sha256_hex(hunk.replacement().as_bytes()),
                "rationale_sha256": sha256_hex(hunk.rationale().as_bytes()),
            })
        })
        .collect::<Vec<_>>();
    let snapshot = proposal.snapshot();
    let coverage = build_question_coverage(coverage);
    digest_json(&json!({
        "binding_schema": "markturbo-goal-07-proposal-binding-v2",
        "source_sha256": sha256_hex(proposal.source().as_bytes()),
        "source_byte_count": proposal.source().len(),
        "source_revision": snapshot.revision,
        "source_generation": snapshot.source_generation,
        "hunks": hunks,
        "coverage": coverage,
    }))
}

fn build_approved_output(
    proposal: &RevisionProposal,
    coverage: &[mt_app::review::RevisionQuestionCoverage],
    decisions: Option<&ParsedDecisions>,
) -> Result<Value, String> {
    let Some(decisions) = decisions else {
        return Ok(json!({
            "status": "not_composed",
            "decision_file_sha256": Value::Null,
            "decision_set_sha256": Value::Null,
            "proposal_sha256": Value::Null,
            "decision_count": 0,
            "result_sha256": Value::Null,
            "result_byte_count": Value::Null,
        }));
    };
    let result = proposal
        .compose(&decisions.decisions)
        .map_err(|_| "owner decisions could not compose the validated proposal".to_owned())?;
    Ok(json!({
        "status": "composed",
        "decision_file_sha256": decisions.raw_sha256,
        "decision_set_sha256": decisions.decision_set_sha256,
        "proposal_sha256": proposal_digest(proposal, coverage)?,
        "decision_count": decisions.decisions.len(),
        "result_sha256": sha256_hex(result.as_bytes()),
        "result_byte_count": result.len(),
    }))
}

fn read_private_file(path: &Path, max_bytes: usize, label: &str) -> Result<Vec<u8>, String> {
    let safe_path = reject_unsafe_path(path, label)?;
    let file = OpenOptions::new()
        .read(true)
        .open(&safe_path)
        .map_err(|_| format!("could not read {label}"))?;
    let opened_metadata = file
        .metadata()
        .map_err(|_| format!("could not inspect {label}"))?;
    if !opened_metadata.is_file() || opened_metadata.len() > max_bytes as u64 {
        return Err(format!("{label} exceeds its bound"));
    }
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    (&file)
        .take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| format!("could not read {label}"))?;
    if bytes.len() > max_bytes {
        return Err(format!("{label} exceeds its bound"));
    }
    let final_metadata = file
        .metadata()
        .map_err(|_| format!("could not inspect {label}"))?;
    if !final_metadata.is_file()
        || final_metadata.len() > max_bytes as u64
        || bytes.len() as u64 > max_bytes as u64
    {
        return Err(format!("{label} exceeds its bound"));
    }
    Ok(bytes)
}

fn write_create_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("receipt path is required".to_owned());
    }
    let safe_path = reject_unsafe_path(path, "receipt")?;
    let parent = safe_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| "receipt path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|_| "could not create receipt directory".to_owned())?;
    let safe_path = reject_unsafe_path(&safe_path, "receipt")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(safe_path)
        .map_err(|_| "receipt path already exists or is unavailable".to_owned())?;
    file.write_all(bytes)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|_| "could not write receipt".to_owned())
}

fn reject_unsafe_path(path: &Path, label: &str) -> Result<PathBuf, String> {
    let current_dir = env::current_dir().map_err(|_| format!("{label} path is unavailable"))?;
    let repository_root = repository_root()?;
    reject_unsafe_path_from(path, label, &current_dir, &repository_root)
}

fn repository_root() -> Result<PathBuf, String> {
    repository_root_from_manifest(Path::new(env!("CARGO_MANIFEST_DIR")))
}

// CARGO_MANIFEST_DIR is a compile-time source anchor. A moved binary therefore
// fails closed when its source checkout and immutable corpus are unavailable.
fn repository_root_from_manifest(manifest_dir: &Path) -> Result<PathBuf, String> {
    let crates_dir = manifest_dir
        .parent()
        .ok_or_else(|| "workspace manifest anchor is unavailable".to_owned())?;
    let root = crates_dir
        .parent()
        .ok_or_else(|| "workspace root manifest anchor is unavailable".to_owned())?;
    let root = canonicalize_existing_directory(root, "workspace manifest anchor")?;
    let manifest = root.join("Cargo.toml");
    let manifest_metadata = fs::symlink_metadata(&manifest)
        .map_err(|_| "workspace root manifest is unavailable".to_owned())?;
    if metadata_is_reparse_or_symlink(&manifest_metadata) || !manifest_metadata.is_file() {
        return Err("workspace root manifest is unavailable".to_owned());
    }
    canonicalize_existing_directory(
        &root.join("evaluation").join("goal-01"),
        "immutable evaluation corpus",
    )?;
    Ok(root)
}

fn reject_unsafe_path_from(
    path: &Path,
    label: &str,
    current_dir: &Path,
    repository_root: &Path,
) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        current_dir.join(path)
    };
    reject_reparse_ancestors(&absolute, label)?;
    let normalized = normalize_path(&absolute);
    let resolved = canonicalize_nearest_existing_parent(&normalized, label)?;
    let immutable = canonicalize_existing_directory(
        &repository_root.join("evaluation").join("goal-01"),
        "immutable evaluation corpus",
    )?;
    if path_starts_with(&resolved, &immutable) {
        return Err(format!(
            "{label} must remain outside the immutable evaluation corpus"
        ));
    }
    Ok(resolved)
}

fn metadata_is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn reject_reparse_ancestors(path: &Path, label: &str) -> Result<(), String> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata_is_reparse_or_symlink(&metadata) => {
                return Err(format!("{label} must not use a symlink or reparse path"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(format!("{label} path is unavailable")),
        }
    }
    Ok(())
}

fn canonicalize_existing_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| format!("{label} is unavailable"))?;
    if metadata_is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
        return Err(format!("{label} is unavailable"));
    }
    fs::canonicalize(path).map_err(|_| format!("{label} is unavailable"))
}

fn canonicalize_nearest_existing_parent(path: &Path, label: &str) -> Result<PathBuf, String> {
    let mut candidate = path.to_owned();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) => {
                if metadata_is_reparse_or_symlink(&metadata) {
                    return Err(format!("{label} must not use a symlink or reparse path"));
                }
                let mut resolved = fs::canonicalize(&candidate)
                    .map_err(|_| format!("{label} path is unavailable"))?;
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = candidate
                    .file_name()
                    .ok_or_else(|| format!("{label} path is unavailable"))?;
                missing.push(name.to_owned());
                if !candidate.pop() {
                    return Err(format!("{label} path is unavailable"));
                }
            }
            Err(_) => return Err(format!("{label} path is unavailable")),
        }
    }
}

fn path_starts_with(path: &Path, prefix: &Path) -> bool {
    #[cfg(windows)]
    {
        let mut path_components = path.components();
        prefix.components().all(|prefix_component| {
            path_components.next().is_some_and(|path_component| {
                path_component
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&prefix_component.as_os_str().to_string_lossy())
            })
        })
    }
    #[cfg(not(windows))]
    {
        path.starts_with(prefix)
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn digest_json(value: &Value) -> Result<String, String> {
    let canonical = canonical_value(value);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|_| "could not serialize digest input".to_owned())?;
    Ok(sha256_hex(&bytes))
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), canonical_value(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        other => other.clone(),
    }
}

fn validate_stable_id(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(format!("{field} is invalid"));
    }
    Ok(())
}

fn validate_digest(value: &str, field: &str, allow_zero: bool) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || (!allow_zero && value == "0".repeat(64))
    {
        return Err(format!("{field} is invalid"));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hex_digest(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for &byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_app::model::RevisionRequestBinding;
    use mt_app::review::{
        ArtifactLens, ByteRange, Finding, ReviewModelOutput, ReviewScope, SourceAnchor,
    };
    use mt_doc::review::{
        ClarificationPriority, ClarificationQuestion, ReviewSections, SourceSnapshot,
        StructuredText,
    };

    fn review_output(scope: ReviewScope) -> ReviewModelOutput {
        ReviewModelOutput {
            schema_version: "review-v1".to_owned(),
            scope,
            understood_intent: ReviewSections {
                stated_goal: StructuredText::from("goal"),
                relevant_context: Vec::new(),
                constraints: Vec::new(),
                non_goals: Vec::new(),
                expected_deliverable: StructuredText::from("deliverable"),
                success_evidence: Vec::new(),
                inferred_assumptions: Vec::new(),
                unresolved_decisions: Vec::new(),
            },
            findings: if matches!(scope, ReviewScope::Document) {
                vec![Finding::inference("inert", SourceAnchor::DocumentWide)]
            } else {
                Vec::new()
            },
            clarification_questions: Vec::<ClarificationQuestion>::new(),
        }
    }

    fn revision_response(groups: Value) -> String {
        json!({
            "schema_version": "revision-v1",
            "groups": groups,
            "question_coverage": [],
        })
        .to_string()
    }

    fn revision_question_id(question: &str, question_index: usize, priority: u8) -> String {
        let mut digest = Sha256::new();
        digest.update(b"markturbo-revision-question-v1\0");
        digest.update((question_index as u64).to_be_bytes());
        digest.update(question.as_bytes());
        digest.update([priority]);
        digest.update([0]);
        format!("{:x}", digest.finalize())
    }

    fn revision_response_with_question(groups: Value, status: Value) -> String {
        json!({
            "schema_version": "revision-v1",
            "groups": groups,
            "question_coverage": [{
                "question_index": 0,
                "question_id": revision_question_id("question", 0, 1),
                "status": status,
            }],
        })
        .to_string()
    }

    fn document_capture_with_parts(
        source: &str,
        response: String,
        output: ReviewModelOutput,
        answers: RevisionAnswers,
    ) -> ParsedCapture {
        let request =
            ReviewRequest::document(ArtifactLens::Prompt, source, SourceSnapshot::default())
                .unwrap();
        let revision = decode_revision_capture(&request, &output, &answers, &response).unwrap();
        let file_path = "evaluation/goal-01/task-prompts/01-upgrade-gpui-without-regression.md";
        let file = CorpusArtifactFile {
            path: file_path.to_owned(),
            sha256: sha256_hex(source.as_bytes()),
            byte_count: source.len() as u64,
        };
        let mut corpus_artifact = CorpusArtifact {
            artifact_id: "TP-01".to_owned(),
            path: file_path.to_owned(),
            lens: "Task prompt".to_owned(),
            sha256: "0".repeat(64),
            byte_count: source.len() as u64,
            files: vec![file],
        };
        corpus_artifact.sha256 = corpus_artifact_files_digest(&corpus_artifact.files).unwrap();
        let (request_artifact_sha256, request_artifact_byte_count) =
            request_artifact_binding(&request, &corpus_artifact, revision.proposal().source())
                .unwrap();
        ParsedCapture {
            corpus_version: CORPUS_VERSION.to_owned(),
            manifest_sha256: "1".repeat(64),
            corpus_artifact,
            request,
            answers,
            raw_revision_response: response,
            revision,
            request_artifact_sha256,
            request_artifact_byte_count,
        }
    }

    fn document_capture(source: &str, response: String) -> ParsedCapture {
        document_capture_with_parts(
            source,
            response,
            review_output(ReviewScope::Document),
            RevisionAnswers::empty(),
        )
    }

    fn document_capture_with_question(source: &str, response: String) -> ParsedCapture {
        let mut output = review_output(ReviewScope::Document);
        output.clarification_questions = vec![ClarificationQuestion::new(
            "question",
            ClarificationPriority::High,
        )];
        document_capture_with_parts(
            source,
            response,
            output,
            RevisionAnswers::new(vec![RevisionAnswer::answered("answer")]).unwrap(),
        )
    }

    fn selection_capture() -> ParsedCapture {
        let source = "prefix\nselected\nsuffix";
        let range = ByteRange::new(7, 15).unwrap();
        let request = ReviewRequest::selection(
            ArtifactLens::Prompt,
            source,
            range,
            SourceSnapshot::default(),
        )
        .unwrap();
        let output = review_output(ReviewScope::selection(range));
        let response = revision_response(json!([
            {
                "rationale": "clarify the selected text",
                "edits": [{
                    "range": {"start": 0, "end": 8},
                    "expected_source": "selected",
                    "replacement": "chosen"
                }]
            }
        ]));
        let answers = RevisionAnswers::empty();
        let revision = decode_revision_capture(&request, &output, &answers, &response).unwrap();
        let file_path = "evaluation/goal-01/task-prompts/selection.md";
        let file = CorpusArtifactFile {
            path: file_path.to_owned(),
            sha256: sha256_hex(source.as_bytes()),
            byte_count: source.len() as u64,
        };
        let mut corpus_artifact = CorpusArtifact {
            artifact_id: "TP-SEL-01".to_owned(),
            path: file_path.to_owned(),
            lens: "Task prompt".to_owned(),
            sha256: "0".repeat(64),
            byte_count: source.len() as u64,
            files: vec![file],
        };
        corpus_artifact.sha256 = corpus_artifact_files_digest(&corpus_artifact.files).unwrap();
        let (request_artifact_sha256, request_artifact_byte_count) =
            request_artifact_binding(&request, &corpus_artifact, revision.proposal().source())
                .unwrap();
        ParsedCapture {
            corpus_version: CORPUS_VERSION.to_owned(),
            manifest_sha256: "1".repeat(64),
            corpus_artifact,
            request,
            answers,
            raw_revision_response: response,
            revision,
            request_artifact_sha256,
            request_artifact_byte_count,
        }
    }

    #[test]
    fn malformed_capture_fails_before_revision_decode() {
        assert!(parse_capture(br#"{"schema":"wrong"}"#).is_err());
    }

    #[test]
    fn validated_review_output_capture_round_trips_through_the_production_seam() {
        let request =
            ReviewRequest::document(ArtifactLens::Prompt, "source", SourceSnapshot::default())
                .unwrap();
        let output = review_output(ReviewScope::Document);
        let serialized = serde_json::to_string(&output).unwrap();
        let decoded = decode_review_output_capture(&serialized, &request).unwrap();
        assert_eq!(decoded, output);
    }

    #[test]
    fn empty_proposal_and_reject_all_are_byte_identical() {
        let capture = document_capture("alpha\r\n世界😀\r\n", revision_response(json!([])));
        let receipt = build_receipt(
            std::slice::from_ref(&capture),
            &"3".repeat(64),
            &BTreeMap::new(),
        )
        .unwrap();
        let case = &receipt["cases"]["TP-01"];
        assert_eq!(case["changes"], json!([]));
        assert_eq!(
            case["reject_all"]["source_sha256"],
            case["reject_all"]["result_sha256"]
        );
        assert_eq!(
            case["reject_all"]["source_byte_count"],
            case["reject_all"]["result_byte_count"]
        );
    }

    #[test]
    fn deletion_and_grouped_multi_hunk_are_recorded_without_content() {
        let source = "one\r\ntwo\r\nthree";
        let response = revision_response(json!([
            {
                "rationale": "remove two and three",
                "edits": [
                    {"range": {"start": 5, "end": 8}, "expected_source": "two", "replacement": ""},
                    {"range": {"start": 10, "end": 15}, "expected_source": "three", "replacement": "3"}
                ]
            }
        ]));
        let capture = document_capture(source, response);
        let receipt = build_receipt(
            std::slice::from_ref(&capture),
            &"3".repeat(64),
            &BTreeMap::new(),
        )
        .unwrap();
        let case = &receipt["cases"]["TP-01"];
        assert_eq!(case["changes"][0]["hunks"].as_array().unwrap().len(), 2);
        assert_eq!(case["changes"][0]["intent_change_ids"], json!([]));
        assert_eq!(
            case["changes"][0]["hunks"][0]["replacement_byte_count"],
            json!(0)
        );
        assert!(case["changes"][0]["hunks"][0].get("replacement").is_none());
    }

    #[test]
    fn optional_decision_file_only_emits_composed_hashes() {
        let response = revision_response(json!([
            {"rationale": "rename", "edits": [{"range": {"start": 0, "end": 3}, "expected_source": "old", "replacement": "new"}]}
        ]));
        let capture = document_capture("old", response);
        let decisions = ParsedDecisions {
            raw_sha256: "4".repeat(64),
            decision_set_sha256: "5".repeat(64),
            decisions: vec![(ChangeId(0), true)],
            intent_change_ids: BTreeMap::from([(ChangeId(0), Vec::new())]),
        };
        let decisions = BTreeMap::from([("TP-01".to_owned(), decisions)]);
        let receipt = build_receipt(&[capture], &"3".repeat(64), &decisions).unwrap();
        let approved = &receipt["cases"]["TP-01"]["approved_output"];
        assert_eq!(approved["status"], json!("composed"));
        assert_eq!(approved["decision_file_sha256"], json!("4".repeat(64)));
        assert_ne!(approved["result_sha256"], Value::Null);
        assert!(approved.get("intent_judgment").is_none());
    }

    #[test]
    fn receipt_identity_fields_are_copyable_into_owner_decision() {
        let capture = document_capture(
            "old",
            revision_response(json!([
                {"rationale": "rename", "edits": [{"range": {"start": 0, "end": 3}, "expected_source": "old", "replacement": "new"}]}
            ])),
        );
        let receipt = build_receipt(
            std::slice::from_ref(&capture),
            &"3".repeat(64),
            &BTreeMap::new(),
        )
        .unwrap();
        let case = &receipt["cases"]["TP-01"];
        assert_eq!(case["editable_source_sha256"], json!(sha256_hex(b"old")));
        assert_eq!(
            case["source_binding_sha256"],
            json!(hex_digest(capture.revision.binding().source_sha256()))
        );
        assert_eq!(
            case["review_context_sha256"],
            json!(hex_digest(
                capture.revision.binding().review_context_digest()
            ))
        );

        let decision = decision_value(&capture, "TP-01");
        assert_eq!(
            decision["editable_source_sha256"],
            case["editable_source_sha256"]
        );
        assert_eq!(
            decision["source_binding_sha256"],
            case["source_binding_sha256"]
        );
        let path = write_temp_decision(&decision, "receipt-scaffold");
        assert!(
            read_decisions_for_captures(
                std::slice::from_ref(&path),
                std::slice::from_ref(&capture)
            )
            .is_ok()
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn selection_receipt_identity_can_author_a_valid_decision() {
        let capture = selection_capture();
        let receipt = build_receipt(
            std::slice::from_ref(&capture),
            &"3".repeat(64),
            &BTreeMap::new(),
        )
        .unwrap();
        let case = &receipt["cases"]["TP-SEL-01"];
        assert_eq!(
            case["editable_source_sha256"],
            json!(sha256_hex(capture.revision.proposal().source().as_bytes()))
        );
        assert_eq!(
            case["source_binding_sha256"],
            json!(hex_digest(capture.revision.binding().source_sha256()))
        );
        let decision = decision_value(&capture, "TP-SEL-01");
        let path = write_temp_decision(&decision, "selection-receipt-scaffold");
        assert!(
            read_decisions_for_captures(
                std::slice::from_ref(&path),
                std::slice::from_ref(&capture)
            )
            .is_ok()
        );
        let _ = fs::remove_file(path);
    }

    fn decision_value(capture: &ParsedCapture, artifact_id: &str) -> Value {
        let receipt = build_receipt(
            std::slice::from_ref(capture),
            &"3".repeat(64),
            &BTreeMap::new(),
        )
        .unwrap();
        let case = &receipt["cases"][artifact_id];
        json!({
            "schema": "markturbo-goal-07-owner-composition-v2",
            "artifact_id": artifact_id,
            "proposal_sha256": case["proposal_sha256"].clone(),
            "editable_source_sha256": case["editable_source_sha256"].clone(),
            "editable_source_byte_count": case["editable_source_byte_count"].clone(),
            "source_binding_sha256": case["source_binding_sha256"].clone(),
            "source_revision": case["source_revision"].clone(),
            "source_generation": case["source_generation"].clone(),
            "artifact_lens_sha256": case["artifact_lens_sha256"].clone(),
            "review_context_sha256": case["review_context_sha256"].clone(),
            "answers_sha256": case["answers_sha256"].clone(),
            "decisions": [{
                "change_id": 0,
                "accepted": true,
                "intent_change_ids": [format!("{artifact_id}-IV-01")]
            }],
        })
    }

    fn write_temp_decision(value: &Value, label: &str) -> PathBuf {
        // macOS can spell its temporary root through the /var symlink.
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "markturbo-g07-{label}-{}-{:x}.json",
            std::process::id(),
            Sha256::digest(label.as_bytes())
        ));
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }

    #[test]
    fn multiple_decision_files_bind_to_matching_captures() {
        let first = document_capture(
            "old",
            revision_response(json!([
                {"rationale": "rename", "edits": [{"range": {"start": 0, "end": 3}, "expected_source": "old", "replacement": "new"}]}
            ])),
        );
        let mut second = document_capture(
            "two",
            revision_response(json!([
                {"rationale": "rename", "edits": [{"range": {"start": 0, "end": 3}, "expected_source": "two", "replacement": "new"}]}
            ])),
        );
        second.corpus_artifact.artifact_id = "TP-02".to_owned();
        let first_decision_path =
            write_temp_decision(&decision_value(&first, "TP-01"), "first-decision");
        let second_decision_path =
            write_temp_decision(&decision_value(&second, "TP-02"), "second-decision");
        let captures = vec![first, second];
        let decisions = read_decisions_for_captures(
            &[first_decision_path.clone(), second_decision_path.clone()],
            &captures,
        )
        .unwrap();
        let receipt = build_receipt(&captures, &"3".repeat(64), &decisions).unwrap();
        assert_eq!(
            receipt["cases"]["TP-01"]["approved_output"]["status"],
            json!("composed")
        );
        assert_eq!(
            receipt["cases"]["TP-02"]["approved_output"]["status"],
            json!("composed")
        );
        assert_eq!(
            receipt["cases"]["TP-01"]["changes"][0]["intent_change_ids"],
            json!(["TP-01-IV-01"])
        );
        assert_eq!(
            receipt["cases"]["TP-02"]["changes"][0]["intent_change_ids"],
            json!(["TP-02-IV-01"])
        );
        let _ = fs::remove_file(first_decision_path);
        let _ = fs::remove_file(second_decision_path);
    }

    #[test]
    fn missing_duplicate_and_unknown_decision_files_fail_closed() {
        let first = document_capture(
            "old",
            revision_response(json!([
                {"rationale": "rename", "edits": [{"range": {"start": 0, "end": 3}, "expected_source": "old", "replacement": "new"}]}
            ])),
        );
        let mut second = document_capture(
            "two",
            revision_response(json!([
                {"rationale": "rename", "edits": [{"range": {"start": 0, "end": 3}, "expected_source": "two", "replacement": "new"}]}
            ])),
        );
        second.corpus_artifact.artifact_id = "TP-02".to_owned();
        let first_path = write_temp_decision(&decision_value(&first, "TP-01"), "missing-decision");
        let captures = vec![first, second];
        let missing =
            read_decisions_for_captures(std::slice::from_ref(&first_path), &captures).unwrap();
        let receipt = build_receipt(&captures, &"3".repeat(64), &missing).unwrap();
        assert_eq!(
            receipt["cases"]["TP-02"]["approved_output"]["status"],
            json!("not_composed")
        );
        assert!(
            read_decisions_for_captures(&[first_path.clone(), first_path.clone()], &captures)
                .is_err()
        );
        let mut missing_intent = decision_value(&captures[0], "TP-01");
        missing_intent["decisions"][0]
            .as_object_mut()
            .unwrap()
            .remove("intent_change_ids");
        let missing_intent_path = write_temp_decision(&missing_intent, "missing-intent");
        assert!(
            read_decisions_for_captures(std::slice::from_ref(&missing_intent_path), &captures)
                .is_err()
        );
        let mut duplicate_intent = decision_value(&captures[0], "TP-01");
        duplicate_intent["decisions"][0]["intent_change_ids"] =
            json!(["TP-01-IV-01", "TP-01-IV-01"]);
        let duplicate_intent_path = write_temp_decision(&duplicate_intent, "duplicate-intent");
        assert!(
            read_decisions_for_captures(std::slice::from_ref(&duplicate_intent_path), &captures)
                .is_err()
        );
        let mut invalid_intent = decision_value(&captures[0], "TP-01");
        invalid_intent["decisions"][0]["intent_change_ids"] = json!([42]);
        let invalid_intent_path = write_temp_decision(&invalid_intent, "invalid-intent");
        assert!(
            read_decisions_for_captures(std::slice::from_ref(&invalid_intent_path), &captures)
                .is_err()
        );
        let unknown_value = decision_value(&captures[0], "TP-99");
        let unknown_path = write_temp_decision(&unknown_value, "unknown-decision");
        assert!(
            read_decisions_for_captures(std::slice::from_ref(&unknown_path), &captures).is_err()
        );
        let _ = fs::remove_file(first_path);
        let _ = fs::remove_file(missing_intent_path);
        let _ = fs::remove_file(duplicate_intent_path);
        let _ = fs::remove_file(invalid_intent_path);
        let _ = fs::remove_file(unknown_path);
    }

    #[test]
    fn owner_decisions_reject_replay_across_question_coverage_metadata() {
        let groups = json!([
            {
                "rationale": "rename",
                "edits": [{
                    "range": {"start": 0, "end": 3},
                    "expected_source": "old",
                    "replacement": "new"
                }]
            }
        ]);
        let represented = document_capture_with_question(
            "old",
            revision_response_with_question(
                groups.clone(),
                json!({"kind": "represented", "change_ids": [0]}),
            ),
        );
        let omitted = document_capture_with_question(
            "old",
            revision_response_with_question(
                groups,
                json!({
                    "kind": "intentionally_omitted",
                    "reason": "The answered question is intentionally not written."
                }),
            ),
        );
        assert_eq!(
            represented.revision.binding(),
            omitted.revision.binding(),
            "coverage is proposal metadata, not request binding"
        );
        assert_ne!(
            proposal_digest(
                represented.revision.proposal(),
                represented.revision.question_coverage()
            )
            .unwrap(),
            proposal_digest(
                omitted.revision.proposal(),
                omitted.revision.question_coverage()
            )
            .unwrap()
        );

        let path = write_temp_decision(&decision_value(&represented, "TP-01"), "coverage-replay");
        let result = read_decisions_for_captures(std::slice::from_ref(&path), &[omitted]);
        assert!(
            result.is_err(),
            "owner decision must not cross proposal coverage"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn owner_decisions_reject_source_and_request_binding_replay() {
        let capture = document_capture(
            "old",
            revision_response(json!([
                {"rationale": "rename", "edits": [{"range": {"start": 0, "end": 3}, "expected_source": "old", "replacement": "new"}]}
            ])),
        );
        let fields = [
            ("editable_source_sha256", json!("1".repeat(64))),
            (
                "editable_source_byte_count",
                json!(capture.revision.proposal().source().len() + 1),
            ),
            ("source_binding_sha256", json!("5".repeat(64))),
            ("source_revision", json!(1)),
            ("source_generation", json!(1)),
            ("artifact_lens_sha256", json!("2".repeat(64))),
            ("review_context_sha256", json!("3".repeat(64))),
            ("answers_sha256", json!("4".repeat(64))),
        ];
        for (index, (field, value)) in fields.into_iter().enumerate() {
            let mut decision = decision_value(&capture, "TP-01");
            decision[field] = value;
            let path = write_temp_decision(&decision, &format!("binding-replay-{index}"));
            assert!(
                read_decisions_for_captures(
                    std::slice::from_ref(&path),
                    std::slice::from_ref(&capture)
                )
                .is_err(),
                "owner decision field {field} must remain bound"
            );
            let _ = fs::remove_file(path);
        }
    }

    #[test]
    fn immutable_corpus_guard_uses_manifest_anchor_not_current_directory() {
        let root = repository_root().unwrap();
        let fake_current_dir =
            std::env::temp_dir().join(format!("markturbo-g07-cwd-{}", std::process::id()));
        fs::create_dir_all(&fake_current_dir).unwrap();
        let corpus_path = root
            .join("evaluation")
            .join("goal-01")
            .join("MANIFEST.sha256");
        let result =
            reject_unsafe_path_from(&corpus_path, "capture input", &fake_current_dir, &root);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("immutable evaluation corpus"));
        let _ = fs::remove_dir(&fake_current_dir);
    }

    #[cfg(windows)]
    #[test]
    fn immutable_corpus_guard_rejects_alternate_case_path() {
        let root = repository_root().unwrap();
        let alternate_case_path = root
            .join("EVALUATION")
            .join("GOAL-01")
            .join("MANIFEST.SHA256");
        let result = reject_unsafe_path_from(
            &alternate_case_path,
            "capture input",
            &std::env::temp_dir(),
            &root,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("immutable evaluation corpus"));
    }

    #[test]
    fn moved_manifest_anchor_fails_closed() {
        let moved_manifest = std::env::temp_dir()
            .join(format!("markturbo-g07-moved-{}", std::process::id()))
            .join("crates")
            .join("mt-app");
        assert!(repository_root_from_manifest(&moved_manifest).is_err());
    }

    #[test]
    fn private_file_reader_enforces_bound_from_open_handle() {
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "markturbo-g07-private-bound-{}",
            std::process::id()
        ));
        fs::write(&path, vec![b'x'; MAX_DECISION_BYTES + 1]).unwrap();
        let result = read_private_file(&path, MAX_DECISION_BYTES, "owner decisions");
        assert!(result.unwrap_err().contains("exceeds its bound"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_skill_uses_only_active_skill_entrypoint_as_editable_source() {
        let package = mt_doc::review::SkillPackage::new(
            vec![
                mt_doc::review::SkillPackageFile::text("SKILL.md", "active\n", "entry").unwrap(),
                mt_doc::review::SkillPackageFile::text(
                    "references/readme.md",
                    "support\n",
                    "support",
                )
                .unwrap(),
            ],
            Vec::new(),
        )
        .unwrap();
        let request = ReviewRequest::agent_skill(package, SourceSnapshot::default()).unwrap();
        let expected_binding_digest: [u8; 32] = Sha256::digest(request.outbound_bytes()).into();
        let output = review_output(ReviewScope::AgentSkillPackage);
        let answers = RevisionAnswers::empty();
        let response = revision_response(json!([
            {"rationale": "clarify", "edits": [{"range": {"start": 0, "end": 6}, "expected_source": "active", "replacement": "ACTIVE"}]}
        ]));
        let revision = decode_revision_capture(&request, &output, &answers, &response).unwrap();
        assert_eq!(revision.proposal().source(), "active\n");
        assert_eq!(revision.binding().source_sha256(), &expected_binding_digest);
        let capture = ParsedCapture {
            corpus_version: CORPUS_VERSION.to_owned(),
            manifest_sha256: "1".repeat(64),
            corpus_artifact: {
                let files = vec![
                    CorpusArtifactFile {
                        path: "evaluation/goal-01/snapshots/skills/gpui/SKILL.md".to_owned(),
                        sha256: sha256_hex(b"active\n"),
                        byte_count: 7,
                    },
                    CorpusArtifactFile {
                        path: "evaluation/goal-01/snapshots/skills/gpui/references/readme.md"
                            .to_owned(),
                        sha256: sha256_hex(b"support\n"),
                        byte_count: 8,
                    },
                ];
                let mut artifact = CorpusArtifact {
                    artifact_id: "AS-01".to_owned(),
                    path: "evaluation/goal-01/snapshots/skills/gpui".to_owned(),
                    lens: "Agent Skill".to_owned(),
                    sha256: "0".repeat(64),
                    byte_count: 15,
                    files,
                };
                artifact.sha256 = corpus_artifact_files_digest(&artifact.files).unwrap();
                artifact
            },
            request,
            answers,
            raw_revision_response: response,
            revision,
            request_artifact_sha256: String::new(),
            request_artifact_byte_count: 0,
        };
        let (request_artifact_sha256, request_artifact_byte_count) = request_artifact_binding(
            &capture.request,
            &capture.corpus_artifact,
            capture.revision.proposal().source(),
        )
        .unwrap();
        let mut capture = capture;
        capture.request_artifact_sha256 = request_artifact_sha256;
        capture.request_artifact_byte_count = request_artifact_byte_count;
        let receipt = build_receipt(
            std::slice::from_ref(&capture),
            &"3".repeat(64),
            &BTreeMap::new(),
        )
        .unwrap();
        let case = &receipt["cases"]["AS-01"];
        assert_eq!(
            case["editable_source_sha256"],
            json!(sha256_hex(b"active\n"))
        );
        assert_eq!(
            case["source_binding_sha256"],
            json!(hex_digest(capture.revision.binding().source_sha256()))
        );
        assert_eq!(case["editable_source_byte_count"], json!(7));
        let decision = decision_value(&capture, "AS-01");
        let decision_path = write_temp_decision(&decision, "agent-skill-receipt-scaffold");
        assert!(
            read_decisions_for_captures(
                std::slice::from_ref(&decision_path),
                std::slice::from_ref(&capture)
            )
            .is_ok()
        );
        let _ = fs::remove_file(decision_path);
        let serialized = serde_json::to_string(&receipt).unwrap();
        assert!(!serialized.contains("active"));
        assert!(!serialized.contains("support"));
    }

    #[test]
    fn receipt_has_no_content_or_filesystem_claims() {
        let capture = document_capture("private source", revision_response(json!([])));
        let receipt = build_receipt(&[capture], &"3".repeat(64), &BTreeMap::new()).unwrap();
        let serialized = serde_json::to_string(&receipt).unwrap();
        for forbidden in [
            "private source",
            "\"rationale\":",
            "\"answer\":",
            "\"source_text\":",
            "\"absolute_path\":",
            "\"dirty\":",
            "\"undo\":",
            "\"trust\":",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "receipt leaked {forbidden}"
            );
        }
    }

    #[test]
    fn binding_digest_helpers_are_production_values() {
        let capture = document_capture("source", revision_response(json!([])));
        let binding: &RevisionRequestBinding = capture.revision.binding();
        assert_ne!(binding.review_context_digest(), &[0; 32]);
        assert_ne!(binding.answers_digest(), &[0; 32]);
    }
}
