//! Interface strings.
//!
//! One enum of keys and one exhaustive table per language, rather than scattered
//! literals. Missing translations are compile errors.
//!
//! Deliberately not a general i18n framework. There is no pluralization, no
//! gender, no date formatting — this app's interface is a few dozen labels, and
//! the cost of a framework would exceed the strings it manages.

use mt_core::model::{
    EndpointIdentity, EndpointIdentityError, EndpointLocation, ModelOperation,
    ModelRequestDisclosure, OutboundScopeKind, ProxyDisclosure, TransportEncryption,
};
use mt_core::settings::Language;

/// Every string the interface shows.
///
/// Adding a variant is a compile error in each language's `match`, which is the
/// point: a language that silently lost a string would be discovered by a user,
/// not by the build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    // Title bar and commands
    Workspace,
    Close,
    Quit,
    Refresh,
    SearchFiles,
    ShowingFirstResults,
    ResolvingInstructions,
    Revising,
    PreviewEmpty,
    PreviewLoading,
    DiagramCouldNotRender,
    SaveConflictDetail,
    Opened,
    Reloaded,
    OpenFailed,
    OpenFolderFailed,
    ReloadFailed,
    SaveFailed,
    SaveAsFailed,
    OverwriteFailed,
    SettingsSaved,
    SettingsSaveFailed,
    ReviewWithContext,
    File,
    Assumption,
    System,
    Light,
    Dark,
    SaveSettings,
    KeyStoredForSession,
    KeyStoredSecurely,
    ClipboardTextUnavailable,
    Translate,
    Review,
    ReviewLensPrompt,
    ReviewLensAgentInstructions,
    ReviewLensAgentSkill,
    ReviewWaiting,
    ReviewStale,
    ReviewMalformed,
    ReviewFailed,
    ReviewReady,
    ReviewPartial,
    ReviewStatedGoal,
    ReviewContext,
    ReviewConstraints,
    ReviewNonGoals,
    ReviewDeliverable,
    ReviewSuccessEvidence,
    ReviewAssumptions,
    ReviewDecisions,
    ReviewPriorityCritical,
    ReviewPriorityHigh,
    ReviewPriorityMedium,
    ReviewPriorityLow,
    ReviewSourceStatement,
    ReviewInference,

    // Approved Revision workflow
    SendToModel,
    ModelRequestWaiting,
    ModelRequestDocumentClosed,
    ModelRequestDocumentChanged,
    Settings,
    Save,
    SaveAsPicker,
    SaveAsSnapshotChanged,
    Cancel,

    // Side panels
    SectionSkills,

    // Search
    Searching,
    NoMatches,

    // Tabs
    Untitled,

    // Empty states
    Recent,

    // Document view
    ModeSource,
    ModeNative,
    ModeWeb,
    WebPreviewFailed,

    // Inspector fields
    Saved,
    Open,

    // Settings pages
    Theme,
    Language_,
    Translation,
    Provider,
    Model,
    BaseUrl,
    TargetLanguage,
    ApiKey,
    Credential,
    ProviderBestAvailable,

    // Credential commands and status
    StoreCredentialSecurely,
    UseCredentialForSession,
    ChooseProviderForCredential,
    UnsupportedProvider,
    CredentialEndpointInvalid,
    CredentialDeleted,
    CredentialReadFailed,
    CredentialRequired,

    // Endpoint validation
    EndpointInvalidUrl,
    EndpointUnsupportedScheme,
    EndpointMissingHost,
    EndpointUserInfoNotAllowed,
    EndpointQueryNotAllowed,
    EndpointFragmentNotAllowed,
    EndpointInsecureRemoteTransport,

    // Optional local usage counters
    UsageCounters,
    UsageDisclosure,
    UsageRetention,
    UsageOff,
    UsageOn,
    UsageInspect,
    UsageExport,
    UsageClear,
    UsageExportFileName,
    UsageExportFormat,
    UsageConsentSaved,
    UsageConsentSaveFailed,
    UsageUnavailable,
    UsageExported,
    UsageExportFailed,
    UsageCleared,
    UsageUtcDay,
    UsageFirstLaunch,
    UsageArtifactOpened,
    UsageArtifactPasted,
    UsageReviewStarted,
    UsageReviewCompleted,
    UsageClarificationAnswered,
    UsageClarificationDeferred,
    UsageClarificationRejected,
    UsageRevisionDecision,
    UsageRevisionApplied,
    UsageArtifactSaved,
    UsageArtifactCopied,
    UsageProviderFailure,
    UsageCredentialFailure,
    UsageStaleResultBlocked,
    UsageRecoveryRestored,
    UsageAcceptedHunks,
    UsageRejectedHunks,
    UsageScopeDocument,
    UsageScopeSelection,
    UsageLensSpecification,
    UsageLensPlan,
    UsageDurationUnderOne,
    UsageDurationOneToFive,
    UsageDurationFiveToFifteen,
    UsageDurationFifteenPlus,
}

/// The string for `key` in the language the user picked.
pub fn t(language: Language, key: Key) -> &'static str {
    match language {
        Language::English => english(key),
        Language::Chinese => chinese(key),
    }
}

pub(crate) fn diagram_failure_label(language: Language, kind: &str) -> String {
    format!("{kind} {}.", t(language, Key::DiagramCouldNotRender))
}

pub fn usage_summary(language: Language, counts: &mt_core::usage::UsageAggregate) -> String {
    use mt_core::usage::UsageEventKind;
    let total = |kind| {
        counts.days().values().fold(0_u64, |sum, day| {
            sum.saturating_add(day.events.get(&kind).copied().unwrap_or_default())
        })
    };
    let days = counts.days().len();
    let reviews = total(UsageEventKind::ReviewCompleted);
    let decisions = total(UsageEventKind::RevisionDecision);
    let events = counts
        .days()
        .values()
        .flat_map(|day| day.events.values())
        .fold(0_u64, |sum, count| sum.saturating_add(*count));
    match language {
        Language::English => format!(
            "{days} retained UTC days · {reviews} Reviews completed · {decisions} revision decisions · {events} events"
        ),
        Language::Chinese => format!(
            "保留 {days} 个 UTC 日 · 完成 {reviews} 次审查 · {decisions} 次修订决定 · {events} 个事件"
        ),
    }
}

pub fn usage_details(language: Language, counts: &mt_core::usage::UsageAggregate) -> String {
    use mt_core::{
        review::ArtifactLens,
        usage::{ReviewDurationBucket, UsageEventKind, UsageReviewScope},
    };
    let mut lines = Vec::new();
    for (day, counts) in counts.days() {
        lines.push(format!("{} {day}", t(language, Key::UsageUtcDay)));
        for (kind, count) in &counts.events {
            let key = match kind {
                UsageEventKind::FirstLaunch => Key::UsageFirstLaunch,
                UsageEventKind::ArtifactOpened => Key::UsageArtifactOpened,
                UsageEventKind::ArtifactPasted => Key::UsageArtifactPasted,
                UsageEventKind::ReviewStarted => Key::UsageReviewStarted,
                UsageEventKind::ReviewCompleted => Key::UsageReviewCompleted,
                UsageEventKind::ClarificationAnswered => Key::UsageClarificationAnswered,
                UsageEventKind::ClarificationDeferred => Key::UsageClarificationDeferred,
                UsageEventKind::ClarificationRejected => Key::UsageClarificationRejected,
                UsageEventKind::RevisionDecision => Key::UsageRevisionDecision,
                UsageEventKind::RevisionApplied => Key::UsageRevisionApplied,
                UsageEventKind::ArtifactSaved => Key::UsageArtifactSaved,
                UsageEventKind::ArtifactCopied => Key::UsageArtifactCopied,
                UsageEventKind::ProviderFailure => Key::UsageProviderFailure,
                UsageEventKind::CredentialFailure => Key::UsageCredentialFailure,
                UsageEventKind::StaleResultBlocked => Key::UsageStaleResultBlocked,
                UsageEventKind::RecoveryRestored => Key::UsageRecoveryRestored,
            };
            lines.push(format!("{}: {count}", t(language, key)));
        }
        for (scope, lenses) in &counts.review_started {
            let scope = match scope {
                UsageReviewScope::Document => Key::UsageScopeDocument,
                UsageReviewScope::Selection => Key::UsageScopeSelection,
            };
            for (lens, count) in lenses {
                let lens = match lens {
                    ArtifactLens::Prompt => Key::ReviewLensPrompt,
                    ArtifactLens::Specification => Key::UsageLensSpecification,
                    ArtifactLens::Plan => Key::UsageLensPlan,
                    ArtifactLens::AgentInstructions => Key::ReviewLensAgentInstructions,
                    ArtifactLens::AgentSkill => Key::ReviewLensAgentSkill,
                };
                lines.push(format!(
                    "{} / {}: {count}",
                    t(language, scope),
                    t(language, lens)
                ));
            }
        }
        for (duration, count) in &counts.review_durations {
            let key = match duration {
                ReviewDurationBucket::UnderOneMinute => Key::UsageDurationUnderOne,
                ReviewDurationBucket::OneToFiveMinutes => Key::UsageDurationOneToFive,
                ReviewDurationBucket::FiveToFifteenMinutes => Key::UsageDurationFiveToFifteen,
                ReviewDurationBucket::FifteenMinutesOrMore => Key::UsageDurationFifteenPlus,
            };
            lines.push(format!("{}: {count}", t(language, key)));
        }
        lines.push(format!(
            "{}: {} · {}: {}",
            t(language, Key::UsageAcceptedHunks),
            counts.accepted_hunks,
            t(language, Key::UsageRejectedHunks),
            counts.rejected_hunks
        ));
    }
    lines.join("\n")
}

pub(crate) fn quoted_path(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| quoted_path(path))
}

pub fn replace_file_title(language: Language, path: &std::path::Path) -> String {
    let name = file_name(path);
    match language {
        Language::English => format!("Replace \u{201c}{name}\u{201d}?"),
        Language::Chinese => format!("替换\u{201c}{name}\u{201d}？"),
    }
}

pub fn replace_file_description(language: Language, path: &std::path::Path) -> String {
    let path = quoted_path(path);
    match language {
        Language::English => {
            format!("This will replace \u{201c}{path}\u{201d} with the current editor text.")
        }
        Language::Chinese => format!("当前编辑器文本将替换\u{201c}{path}\u{201d}。"),
    }
}

pub fn open_recent_target_label(language: Language, path: &std::path::Path) -> String {
    recent_target_label_in(language, path, true)
}

pub fn remove_recent_target_label(language: Language, path: &std::path::Path) -> String {
    recent_target_label_in(language, path, false)
}

pub fn save_as_snapshot_changed_message(language: Language) -> &'static str {
    t(language, Key::SaveAsSnapshotChanged)
}

pub fn save_as_path_already_open_message(language: Language, path: &std::path::Path) -> String {
    let path = quoted_path(path);
    match language {
        Language::English => {
            format!("\u{201c}{path}\u{201d} is already open. Choose another Save As path.")
        }
        Language::Chinese => format!("\u{201c}{path}\u{201d}已打开。请选择其他另存为路径。"),
    }
}

fn recent_target_label_in(language: Language, path: &std::path::Path, open: bool) -> String {
    let path = quoted_path(path);
    match (language, open) {
        (Language::English, true) => format!("Open \u{201c}{path}\u{201d}"),
        (Language::English, false) => format!("Remove \u{201c}{path}\u{201d} from Recent"),
        (Language::Chinese, true) => format!("打开\u{201c}{path}\u{201d}"),
        (Language::Chinese, false) => format!("从最近打开中移除\u{201c}{path}\u{201d}"),
    }
}

pub fn model_endpoint_status(language: Language, endpoint: &EndpointIdentity) -> String {
    let base_url = endpoint.base_url();
    match (
        language,
        endpoint.location(),
        endpoint.transport().is_encrypted(),
        endpoint.transport().uses_proxy(),
    ) {
        (Language::English, EndpointLocation::Local, false, false) => {
            format!("{base_url} is local HTTP. Transport is unencrypted and the proxy is disabled.")
        }
        (Language::Chinese, EndpointLocation::Local, false, false) => {
            format!("{base_url} 是本地 HTTP。传输未加密，且已禁用代理。")
        }
        (Language::English, EndpointLocation::Local, true, false) => {
            format!("{base_url} is local HTTPS. Transport is encrypted and the proxy is disabled.")
        }
        (Language::Chinese, EndpointLocation::Local, true, false) => {
            format!("{base_url} 是本地 HTTPS。传输已加密，且已禁用代理。")
        }
        (Language::English, EndpointLocation::Remote, true, true) => format!(
            "{base_url} is remote HTTPS. Transport is encrypted and may use the configured proxy."
        ),
        (Language::Chinese, EndpointLocation::Remote, true, true) => {
            format!("{base_url} 是远程 HTTPS。传输已加密，并可能使用已配置的代理。")
        }
        (Language::English, _, encrypted, proxy) => {
            format!("{base_url} is valid. Encrypted: {encrypted}. Configured proxy: {proxy}.")
        }
        (Language::Chinese, _, encrypted, proxy) => {
            format!("{base_url} 有效。加密：{encrypted}。配置代理：{proxy}。")
        }
    }
}

pub fn effective_context_choice_label(language: Language, selected: bool) -> &'static str {
    match (language, selected) {
        (Language::English, false) => "Include context source (unchecked)",
        (Language::English, true) => "Include context source (selected)",
        (Language::Chinese, false) => "包含 Context 来源（未选择）",
        (Language::Chinese, true) => "包含 Context 来源（已选择）",
    }
}

pub fn model_request_disclosure(language: Language, disclosure: &ModelRequestDisclosure) -> String {
    let endpoint = disclosure.endpoint();
    let (operation, location, encryption, proxy, suffix) = match language {
        Language::English => (
            match disclosure.operation() {
                ModelOperation::Review => "Review",
                ModelOperation::Revision => "Revision",
                ModelOperation::Translation => "Translation",
            },
            match endpoint.location() {
                EndpointLocation::Local => "local",
                EndpointLocation::Remote => "remote",
            },
            match endpoint.transport().encryption() {
                TransportEncryption::Encrypted => "encrypted with certificate validation",
                TransportEncryption::Unencrypted => "unencrypted",
            },
            match endpoint.transport().proxy() {
                ProxyDisclosure::MayUseConfiguredProxy => "may use the configured system proxy",
                ProxyDisclosure::Disabled => "proxy disabled",
            },
            "Protocol framing crosses this boundary.\nThe disclosed source content also crosses it.",
        ),
        Language::Chinese => (
            match disclosure.operation() {
                ModelOperation::Review => "审查",
                ModelOperation::Revision => "修订",
                ModelOperation::Translation => "翻译",
            },
            match endpoint.location() {
                EndpointLocation::Local => "本地",
                EndpointLocation::Remote => "远程",
            },
            match endpoint.transport().encryption() {
                TransportEncryption::Encrypted => "已加密并验证证书",
                TransportEncryption::Unencrypted => "未加密",
            },
            match endpoint.transport().proxy() {
                ProxyDisclosure::MayUseConfiguredProxy => "可能使用已配置的系统代理",
                ProxyDisclosure::Disabled => "已禁用代理",
            },
            "协议封装会跨越此边界。\n上述源内容也会跨越此边界发送。",
        ),
    };
    let scope = match (language, disclosure.scope().kind()) {
        (Language::English, OutboundScopeKind::Selection) => format!(
            "Scope: selection, {} UTF-8 content bytes",
            disclosure.scope().primary_content_byte_size()
        ),
        (Language::English, OutboundScopeKind::Block) => format!(
            "Scope: current block, {} UTF-8 content bytes",
            disclosure.scope().primary_content_byte_size()
        ),
        (Language::English, OutboundScopeKind::Document) => format!(
            "Scope: document, {} UTF-8 content bytes",
            disclosure.scope().primary_content_byte_size()
        ),
        (Language::Chinese, OutboundScopeKind::Selection) => format!(
            "范围：选区，{} UTF-8 内容字节",
            disclosure.scope().primary_content_byte_size()
        ),
        (Language::Chinese, OutboundScopeKind::Block) => format!(
            "范围：当前块，{} UTF-8 内容字节",
            disclosure.scope().primary_content_byte_size()
        ),
        (Language::Chinese, OutboundScopeKind::Document) => format!(
            "范围：文档，{} UTF-8 内容字节",
            disclosure.scope().primary_content_byte_size()
        ),
        (Language::English, OutboundScopeKind::DocumentWithEffectiveAgentContext) => {
            let sources = disclosure
                .scope()
                .effective_context_sources()
                .iter()
                .map(|source| format!("- {source}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "Scope: document plus Effective Agent Context, {} UTF-8 document bytes\nContext sources:\n{sources}",
                disclosure.scope().primary_content_byte_size()
            )
        }
        (Language::Chinese, OutboundScopeKind::DocumentWithEffectiveAgentContext) => {
            let sources = disclosure
                .scope()
                .effective_context_sources()
                .iter()
                .map(|source| format!("- {source}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "范围：文档及 Effective Agent Context，{} UTF-8 文档字节\nContext 来源：\n{sources}",
                disclosure.scope().primary_content_byte_size()
            )
        }
        (language, OutboundScopeKind::AgentSkillPackage) => {
            let inventory = disclosure
                .scope()
                .agent_skill_inventory()
                .expect("an Agent Skill scope owns an inventory");
            let files = inventory
                .files()
                .iter()
                .map(|file| {
                    format!(
                        "- {} ({} bytes): {}",
                        file.normalized_relative_path(),
                        file.byte_size(),
                        file.inclusion_reason()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            match language {
                Language::English => format!(
                    "Scope: Agent Skill package, {} bytes\nFiles:\n{files}\nCoverage: {}",
                    inventory.total_byte_size(),
                    if inventory.is_partial() {
                        "partial"
                    } else {
                        "complete"
                    }
                ),
                Language::Chinese => format!(
                    "范围：Agent Skill 包，共 {} 字节\n文件：\n{files}\n覆盖范围：{}",
                    inventory.total_byte_size(),
                    if inventory.is_partial() {
                        "部分"
                    } else {
                        "完整"
                    }
                ),
            }
        }
    };
    let scope = if disclosure.scope().kind() == OutboundScopeKind::AgentSkillPackage
        && !disclosure.scope().effective_context_sources().is_empty()
    {
        let sources = disclosure
            .scope()
            .effective_context_sources()
            .iter()
            .map(|source| format!("- {source}"))
            .collect::<Vec<_>>()
            .join("\n");
        match language {
            Language::English => format!("{scope}\nEffective Agent Context sources:\n{sources}"),
            Language::Chinese => format!("{scope}\nEffective Agent Context 来源：\n{sources}"),
        }
    } else {
        scope
    };
    let revision_details = match (language, disclosure.operation()) {
        (Language::English, ModelOperation::Revision) => match disclosure.revision_binding() {
            Some(binding) => {
                let content = disclosure.revision_details().map_or_else(
                    || "\nRevision content sizes are unavailable.".to_owned(),
                    |details| {
                        format!(
                            "\nReview context: {} UTF-8 JSON bytes.\nAnswers: {} states, {} UTF-8 JSON bytes.",
                            details.review_context_bytes(),
                            details.answer_count(),
                            details.answers_bytes(),
                        )
                    },
                );
                format!(
                    "\nSource scope is frozen for this Revision.\nSource SHA-256: {}\nSource revision: {}\nSource generation: {}\nArtifact lens digest: {}\nReview context digest: {}\nAnswers digest: {}{content}",
                    disclosure_digest_label(Some(binding.source_sha256())),
                    binding.source_revision(),
                    binding.source_generation(),
                    disclosure_digest_label(Some(binding.artifact_lens_digest())),
                    disclosure_digest_label(Some(binding.review_context_digest())),
                    disclosure_digest_label(Some(binding.answers_digest())),
                )
            }
            None => "\nRevision binding is unavailable.".to_owned(),
        },
        (Language::Chinese, ModelOperation::Revision) => match disclosure.revision_binding() {
            Some(binding) => {
                let content = disclosure.revision_details().map_or_else(
                    || "\n修订内容大小不可用。".to_owned(),
                    |details| {
                        format!(
                            "\n审查上下文：{} UTF-8 JSON 字节。\n答案：{} 个状态，共 {} UTF-8 JSON 字节。",
                            details.review_context_bytes(),
                            details.answer_count(),
                            details.answers_bytes(),
                        )
                    },
                );
                format!(
                    "\n此修订的源范围已冻结。\n源 SHA-256：{}\n源修订：{}\n源代次：{}\n工件 lens 摘要：{}\n审查上下文摘要：{}\n答案摘要：{}{content}",
                    disclosure_digest_label(Some(binding.source_sha256())),
                    binding.source_revision(),
                    binding.source_generation(),
                    disclosure_digest_label(Some(binding.artifact_lens_digest())),
                    disclosure_digest_label(Some(binding.review_context_digest())),
                    disclosure_digest_label(Some(binding.answers_digest())),
                )
            }
            None => "\n修订绑定不可用。".to_owned(),
        },
        (
            Language::English | Language::Chinese,
            ModelOperation::Review | ModelOperation::Translation,
        ) => String::new(),
    };

    match language {
        Language::English => format!(
            "Operation: {operation}\nEndpoint: {location}\nWire format: {}\nIdentity: {}\nTransport: {encryption}; {proxy}\n{scope}{revision_details}\n\n{suffix}",
            endpoint.provider().label(),
            endpoint.normalized_identity(),
        ),
        Language::Chinese => format!(
            "操作：{operation}\n端点：{location}\n协议格式：{}\n端点标识：{}\n传输：{encryption}；{proxy}\n{scope}{revision_details}\n\n{suffix}",
            endpoint.provider().label(),
            endpoint.normalized_identity(),
        ),
    }
}

fn disclosure_digest_label(digest: Option<&[u8; 32]>) -> String {
    digest
        .map(|digest| {
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        })
        .unwrap_or_else(|| "unavailable".to_owned())
}

pub fn model_endpoint_error(language: Language, error: &EndpointIdentityError) -> String {
    let key = match error {
        EndpointIdentityError::InvalidUrl(_) => Key::EndpointInvalidUrl,
        EndpointIdentityError::UnsupportedScheme => Key::EndpointUnsupportedScheme,
        EndpointIdentityError::MissingHost => Key::EndpointMissingHost,
        EndpointIdentityError::UserInfoNotAllowed => Key::EndpointUserInfoNotAllowed,
        EndpointIdentityError::QueryNotAllowed => Key::EndpointQueryNotAllowed,
        EndpointIdentityError::FragmentNotAllowed => Key::EndpointFragmentNotAllowed,
        EndpointIdentityError::InsecureRemoteTransport => Key::EndpointInsecureRemoteTransport,
    };
    t(language, key).into()
}

pub fn environment_credential_description(
    language: Language,
    variable: &str,
    endpoint: &EndpointIdentity,
) -> String {
    let base_url = endpoint.base_url();
    match language {
        Language::English => format!(
            "Allow {variable} only for {base_url}. Changing the provider or endpoint makes this authorization inactive."
        ),
        Language::Chinese => {
            format!("仅允许 {base_url} 使用 {variable}。更改服务商或端点后，此授权将失效。")
        }
    }
}

pub fn delete_model_credential_description(
    language: Language,
    endpoint: &EndpointIdentity,
) -> String {
    let provider = endpoint.provider().label();
    let base_url = endpoint.base_url();
    match language {
        Language::English => format!(
            "Delete the session and securely stored credential for {provider} at {base_url}? Credentials for other endpoint identities are unchanged."
        ),
        Language::Chinese => format!(
            "删除 {provider} 在 {base_url} 的会话凭据和安全存储凭据？其他端点身份不受影响。"
        ),
    }
}

pub fn migrate_legacy_credential_description(
    language: Language,
    endpoint: &EndpointIdentity,
) -> String {
    let provider = endpoint.provider().label();
    let base_url = endpoint.base_url();
    match language {
        Language::English => format!(
            "Move the legacy plaintext credential to secure storage for {provider} at {base_url}. The plaintext setting is removed only after the secure write is verified."
        ),
        Language::Chinese => format!(
            "将旧的明文凭据迁移到 {provider} 在 {base_url} 的安全存储。仅在安全写入验证成功后删除明文设置。"
        ),
    }
}

fn english(key: Key) -> &'static str {
    match key {
        Key::Workspace => "Workspace",
        Key::Close => "Close",
        Key::Quit => "Quit",
        Key::Refresh => "Refresh",
        Key::SearchFiles => "Search files",
        Key::ShowingFirstResults => "Showing the first results only.",
        Key::ResolvingInstructions => "Resolving instructions…",
        Key::Revising => "Revising…",
        Key::PreviewEmpty => "Nothing to preview yet.",
        Key::PreviewLoading => "Preparing preview…",
        Key::DiagramCouldNotRender => "could not render",
        Key::SaveConflictDetail => {
            "Save refused: the file changed on disk. Compare versions, keep yours, use the disk version, or choose Save As…."
        }
        Key::Opened => "Opened",
        Key::Reloaded => "Reloaded",
        Key::OpenFailed => "Open failed",
        Key::OpenFolderFailed => "Open folder failed",
        Key::ReloadFailed => "Reload failed",
        Key::SaveFailed => "Save failed",
        Key::SaveAsFailed => "Save As failed",
        Key::OverwriteFailed => "Overwrite failed",
        Key::SettingsSaved => "Settings saved.",
        Key::SettingsSaveFailed => "Could not save settings.",
        Key::ReviewWithContext => "Review with selected context",
        Key::File => "File",
        Key::Assumption => "Assumption",
        Key::System => "System",
        Key::Light => "Light",
        Key::Dark => "Dark",
        Key::SaveSettings => "Save",
        Key::KeyStoredForSession => "Key stored for this session.",
        Key::KeyStoredSecurely => "Key stored securely.",
        Key::ClipboardTextUnavailable => "Clipboard does not contain text or is unavailable.",
        Key::Translate => "Translate",
        Key::Review => "Review",
        Key::ReviewLensPrompt => "Prompt",
        Key::ReviewLensAgentInstructions => "Agent Instructions",
        Key::ReviewLensAgentSkill => "Agent Skill",
        Key::ReviewWaiting => "Waiting for review request approval…",
        Key::ReviewStale => "This review describes an older source revision and is stale.",
        Key::ReviewMalformed => {
            "The model returned malformed Review data; nothing was shown as trusted output."
        }
        Key::ReviewFailed => "Review failed. The source document was not changed.",
        Key::ReviewReady => "Review ready",
        Key::ReviewPartial => "Partial review",
        Key::ReviewStatedGoal => "Stated goal",
        Key::ReviewContext => "Relevant context and inputs",
        Key::ReviewConstraints => "Constraints and non-goals",
        Key::ReviewNonGoals => "Non-goals",
        Key::ReviewDeliverable => "Expected deliverable",
        Key::ReviewSuccessEvidence => "Success evidence",
        Key::ReviewAssumptions => "Inferred assumptions",
        Key::ReviewDecisions => "Unresolved or contradictory decisions",
        Key::ReviewPriorityCritical => "Critical",
        Key::ReviewPriorityHigh => "High",
        Key::ReviewPriorityMedium => "Medium",
        Key::ReviewPriorityLow => "Low",
        Key::ReviewSourceStatement => "Source statement",
        Key::ReviewInference => "Inference",

        Key::SendToModel => "Send",
        Key::ModelRequestWaiting => "Waiting for model request approval…",
        Key::ModelRequestDocumentClosed => "Model request cancelled because the document closed",
        Key::ModelRequestDocumentChanged => {
            "The document changed while approval was pending. Review the updated scope and try again."
        }
        Key::Settings => "Settings",
        Key::Save => "Save",
        Key::SaveAsPicker => "Save As\u{2026}",
        Key::SaveAsSnapshotChanged => {
            "The document changed while Save As was open. The pending close was cancelled."
        }
        Key::Cancel => "Cancel",

        Key::SectionSkills => "Skills",

        Key::Searching => "Searching…",
        Key::NoMatches => "No matches.",

        Key::Untitled => "Untitled",

        Key::Recent => "Recent",

        Key::ModeSource => "Edit",
        Key::ModeNative => "Preview",
        Key::ModeWeb => "Compatibility Preview",
        Key::WebPreviewFailed => "The Web preview could not be opened.",

        Key::Saved => "Saved",
        Key::Open => "Open",

        Key::Theme => "Theme",
        Key::Language_ => "Language",
        Key::Translation => "Translation",
        Key::Provider => "Provider",
        Key::Model => "Model",
        Key::BaseUrl => "Base URL",
        Key::TargetLanguage => "Target language",
        Key::ApiKey => "API key",
        Key::Credential => "Credential",
        Key::ProviderBestAvailable => "Best available",

        Key::StoreCredentialSecurely => "Store securely",
        Key::UseCredentialForSession => "Use for session",
        Key::ChooseProviderForCredential => "Choose a provider before configuring credentials.",
        Key::UnsupportedProvider => "The configured provider is not supported.",
        Key::CredentialEndpointInvalid => "Fix the endpoint before configuring credentials.",
        Key::CredentialDeleted => "Credential deleted for this endpoint.",
        Key::CredentialReadFailed => "The secure credential could not be read.",
        Key::CredentialRequired => "Enter a credential first.",

        Key::EndpointInvalidUrl => "The endpoint URL is invalid.",
        Key::EndpointUnsupportedScheme => "The endpoint must use HTTP or HTTPS.",
        Key::EndpointMissingHost => "The endpoint URL is missing a host.",
        Key::EndpointUserInfoNotAllowed => "The endpoint URL cannot contain user information.",
        Key::EndpointQueryNotAllowed => "The endpoint URL cannot contain query parameters.",
        Key::EndpointFragmentNotAllowed => "The endpoint URL cannot contain a fragment.",
        Key::EndpointInsecureRemoteTransport => "Remote endpoints must use HTTPS.",

        Key::UsageCounters => "Local usage counters",
        Key::UsageDisclosure => {
            "Optional, local only: daily counts of opted-in first launch, artifact open/paste/save, Review starts (scope and lens), completed Reviews (duration bucket), submitted clarification answers, revision decisions (accepted/rejected hunk totals), applied revisions, provider/credential failures, stale-result blocks and recovery restores. No content, names, paths, credentials, identities or precise timestamps. Nothing is uploaded; export only when you choose."
        }
        Key::UsageRetention => {
            "Retain counts for 180 days; prune older days on load/save. Day keys are UTC days since 1970-01-01. Off stops recording; Clear deletes local counts, not your exported copies."
        }
        Key::UsageOff => "Off",
        Key::UsageOn => "On",
        Key::UsageInspect => "Inspect counts",
        Key::UsageExport => "Export counts…",
        Key::UsageClear => "Clear counts",
        Key::UsageExportFileName => "markturbo-usage.json",
        Key::UsageExportFormat => "JSON",
        Key::UsageConsentSaved => "Usage-counter preference saved.",
        Key::UsageConsentSaveFailed => {
            "Could not save the preference. Recording is off for this session; try again before restarting."
        }
        Key::UsageUnavailable => {
            "Local usage counts could not be read or written. Editing and Review remain available."
        }
        Key::UsageExported => "Aggregate counts exported. Nothing was uploaded.",
        Key::UsageExportFailed => {
            "Counts could not be exported. Choose a writable path that does not already exist."
        }
        Key::UsageCleared => {
            "Local counts deleted. Recording preference and exported copies are unchanged."
        }
        Key::UsageUtcDay => "UTC day since 1970-01-01:",
        Key::UsageFirstLaunch => "First opted-in launch",
        Key::UsageArtifactOpened => "Artifacts opened",
        Key::UsageArtifactPasted => "Artifacts pasted",
        Key::UsageReviewStarted => "Reviews started",
        Key::UsageReviewCompleted => "Reviews completed",
        Key::UsageClarificationAnswered => "Clarification answers submitted",
        Key::UsageClarificationDeferred => "Clarifications deferred",
        Key::UsageClarificationRejected => "Clarifications rejected",
        Key::UsageRevisionDecision => "Revision decisions",
        Key::UsageRevisionApplied => "Revisions applied",
        Key::UsageArtifactSaved => "Artifacts saved",
        Key::UsageArtifactCopied => "Artifacts copied",
        Key::UsageProviderFailure => "Provider failures",
        Key::UsageCredentialFailure => "Credential failures",
        Key::UsageStaleResultBlocked => "Stale results blocked",
        Key::UsageRecoveryRestored => "Recovery restores",
        Key::UsageAcceptedHunks => "Accepted hunks",
        Key::UsageRejectedHunks => "Rejected hunks",
        Key::UsageScopeDocument => "Whole-artifact Review",
        Key::UsageScopeSelection => "Selection Review",
        Key::UsageLensSpecification => "Specification",
        Key::UsageLensPlan => "Plan",
        Key::UsageDurationUnderOne => "Review duration under 1 minute",
        Key::UsageDurationOneToFive => "Review duration 1–5 minutes",
        Key::UsageDurationFiveToFifteen => "Review duration 5–15 minutes",
        Key::UsageDurationFifteenPlus => "Review duration 15 minutes or more",
    }
}

/// Chinese strings.
///
/// Technical terms and file names stay in English — `SKILL.md` is a filename,
/// not a word. The exhaustive match requires a translation for every key.
fn chinese(key: Key) -> &'static str {
    match key {
        Key::Workspace => "工作区",
        Key::Close => "关闭",
        Key::Quit => "退出",
        Key::Refresh => "刷新",
        Key::SearchFiles => "搜索文件",
        Key::ShowingFirstResults => "仅显示前几条结果。",
        Key::ResolvingInstructions => "正在解析指令…",
        Key::Revising => "正在修订…",
        Key::PreviewEmpty => "暂无内容可预览。",
        Key::PreviewLoading => "正在准备预览…",
        Key::DiagramCouldNotRender => "无法渲染",
        Key::SaveConflictDetail => {
            "保存被拒绝：磁盘上的文件已更改。请比较版本、保留你的版本、采用磁盘版本，或选择“另存为…”。"
        }
        Key::Opened => "已打开",
        Key::Reloaded => "已重新加载",
        Key::OpenFailed => "打开失败",
        Key::OpenFolderFailed => "打开文件夹失败",
        Key::ReloadFailed => "重新加载失败",
        Key::SaveFailed => "保存失败",
        Key::SaveAsFailed => "另存为失败",
        Key::OverwriteFailed => "覆盖失败",
        Key::SettingsSaved => "设置已保存。",
        Key::SettingsSaveFailed => "无法保存设置。",
        Key::ReviewWithContext => "使用所选上下文审查",
        Key::File => "文件",
        Key::Assumption => "假设",
        Key::System => "跟随系统",
        Key::Light => "浅色",
        Key::Dark => "深色",
        Key::SaveSettings => "保存",
        Key::KeyStoredForSession => "密钥已保存到当前会话。",
        Key::KeyStoredSecurely => "密钥已安全保存。",
        Key::ClipboardTextUnavailable => "剪贴板中没有文本，或剪贴板当前不可用。",
        Key::Translate => "翻译",
        Key::Review => "审查",
        Key::ReviewLensPrompt => "Prompt",
        Key::ReviewLensAgentInstructions => "Agent Instructions",
        Key::ReviewLensAgentSkill => "Agent Skill",
        Key::ReviewWaiting => "正在等待审查请求授权…",
        Key::ReviewStale => "此审查描述的是较旧的源版本，已过期。",
        Key::ReviewMalformed => "模型返回的审查数据格式无效，未显示为可信结果。",
        Key::ReviewFailed => "审查失败，源文档未被修改。",
        Key::ReviewReady => "审查已就绪",
        Key::ReviewPartial => "部分审查",
        Key::ReviewStatedGoal => "明确目标",
        Key::ReviewContext => "相关上下文和输入",
        Key::ReviewConstraints => "约束与非目标",
        Key::ReviewNonGoals => "非目标",
        Key::ReviewDeliverable => "预期交付物",
        Key::ReviewSuccessEvidence => "成功证据",
        Key::ReviewAssumptions => "推断的假设",
        Key::ReviewDecisions => "未解决或相互矛盾的决策",
        Key::ReviewPriorityCritical => "关键",
        Key::ReviewPriorityHigh => "高",
        Key::ReviewPriorityMedium => "中",
        Key::ReviewPriorityLow => "低",
        Key::ReviewSourceStatement => "源文本陈述",
        Key::ReviewInference => "推断",

        Key::SendToModel => "发送",
        Key::ModelRequestWaiting => "正在等待模型请求授权…",
        Key::ModelRequestDocumentClosed => "文档已关闭，模型请求已取消",
        Key::ModelRequestDocumentChanged => "等待授权期间文档已更改。请检查更新后的范围并重试。",
        Key::Settings => "设置",
        Key::Save => "保存",
        Key::SaveAsPicker => "另存为\u{2026}",
        Key::SaveAsSnapshotChanged => "另存为对话框打开期间文档已更改，待处理的关闭操作已取消。",
        Key::Cancel => "取消",

        Key::SectionSkills => "Skills",

        Key::Searching => "搜索中…",
        Key::NoMatches => "没有匹配项。",

        Key::Untitled => "未命名",

        Key::Recent => "最近打开",

        Key::ModeSource => "编辑",
        Key::ModeNative => "预览",
        Key::ModeWeb => "兼容预览",
        Key::WebPreviewFailed => "无法打开 Web 预览。",

        Key::Saved => "已保存",
        Key::Open => "打开",

        Key::Theme => "主题",
        Key::Language_ => "界面语言",
        Key::Translation => "翻译",
        Key::Provider => "服务商",
        Key::Model => "模型",
        Key::BaseUrl => "接口地址",
        Key::TargetLanguage => "目标语言",
        Key::ApiKey => "API key",
        Key::Credential => "凭据",
        Key::ProviderBestAvailable => "自动选择",

        Key::StoreCredentialSecurely => "安全存储",
        Key::UseCredentialForSession => "仅本次会话使用",
        Key::ChooseProviderForCredential => "请先选择服务商，再配置凭据。",
        Key::UnsupportedProvider => "当前配置的服务商不受支持。",
        Key::CredentialEndpointInvalid => "请先修正端点，再配置凭据。",
        Key::CredentialDeleted => "已删除此端点的凭据。",
        Key::CredentialReadFailed => "无法读取安全存储的凭据。",
        Key::CredentialRequired => "请先输入凭据。",

        Key::EndpointInvalidUrl => "端点 URL 无效。",
        Key::EndpointUnsupportedScheme => "端点必须使用 HTTP 或 HTTPS。",
        Key::EndpointMissingHost => "端点 URL 缺少主机。",
        Key::EndpointUserInfoNotAllowed => "端点 URL 不能包含用户信息。",
        Key::EndpointQueryNotAllowed => "端点 URL 不能包含查询参数。",
        Key::EndpointFragmentNotAllowed => "端点 URL 不能包含片段。",
        Key::EndpointInsecureRemoteTransport => "远程端点必须使用 HTTPS。",

        Key::UsageCounters => "本地使用计数",
        Key::UsageDisclosure => {
            "可选且仅限本地：按日记录启用后的首次启动、工件打开/粘贴/保存、审查开始（范围及视角）、审查完成（耗时区间）、已提交的澄清回答、修订决定（接受/拒绝的修改块总数）、已应用的修订、服务商/凭据失败、过期结果拦截和恢复操作次数。不记录内容、名称、路径、凭据、身份或精确时间。不上传任何数据；仅由您主动导出。"
        }
        Key::UsageRetention => {
            "计数保留 180 天，读取/保存时清理更早的日期。日期键为自 1970-01-01 起的 UTC 日数。关闭即停止记录；清除仅删除本地计数，不删除您导出的副本。"
        }
        Key::UsageOff => "关闭",
        Key::UsageOn => "开启",
        Key::UsageInspect => "查看计数",
        Key::UsageExport => "导出计数…",
        Key::UsageClear => "清除计数",
        Key::UsageExportFileName => "markturbo-usage.json",
        Key::UsageExportFormat => "JSON",
        Key::UsageConsentSaved => "使用计数偏好已保存。",
        Key::UsageConsentSaveFailed => "无法保存偏好。本次会话已停止记录；请在重启前重试。",
        Key::UsageUnavailable => "无法读取或写入本地使用计数。编辑和审查仍然可用。",
        Key::UsageExported => "已导出汇总计数，未上传任何数据。",
        Key::UsageExportFailed => "无法导出计数。请选择可写且尚不存在的路径。",
        Key::UsageCleared => "已删除本地计数。记录偏好和已导出的副本不变。",
        Key::UsageUtcDay => "自 1970-01-01 起的 UTC 日数：",
        Key::UsageFirstLaunch => "启用后的首次启动",
        Key::UsageArtifactOpened => "工件打开次数",
        Key::UsageArtifactPasted => "工件粘贴次数",
        Key::UsageReviewStarted => "审查开始次数",
        Key::UsageReviewCompleted => "审查完成次数",
        Key::UsageClarificationAnswered => "已提交的澄清回答数",
        Key::UsageClarificationDeferred => "暂缓的澄清数",
        Key::UsageClarificationRejected => "拒绝的澄清数",
        Key::UsageRevisionDecision => "修订决定次数",
        Key::UsageRevisionApplied => "修订应用次数",
        Key::UsageArtifactSaved => "工件保存次数",
        Key::UsageArtifactCopied => "工件复制次数",
        Key::UsageProviderFailure => "服务商失败次数",
        Key::UsageCredentialFailure => "凭据失败次数",
        Key::UsageStaleResultBlocked => "过期结果拦截次数",
        Key::UsageRecoveryRestored => "恢复操作次数",
        Key::UsageAcceptedHunks => "接受的修改块",
        Key::UsageRejectedHunks => "拒绝的修改块",
        Key::UsageScopeDocument => "完整工件审查",
        Key::UsageScopeSelection => "选区审查",
        Key::UsageLensSpecification => "规格",
        Key::UsageLensPlan => "计划",
        Key::UsageDurationUnderOne => "审查耗时不足 1 分钟",
        Key::UsageDurationOneToFive => "审查耗时 1–5 分钟",
        Key::UsageDurationFiveToFifteen => "审查耗时 5–15 分钟",
        Key::UsageDurationFifteenPlus => "审查耗时至少 15 分钟",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key the enum defines, so the coverage tests cannot silently miss
    /// one that was added later.
    const EVERY_KEY: &[Key] = &[
        Key::ClipboardTextUnavailable,
        Key::Translate,
        Key::Review,
        Key::ReviewLensPrompt,
        Key::ReviewLensAgentInstructions,
        Key::ReviewLensAgentSkill,
        Key::ReviewWaiting,
        Key::ReviewStale,
        Key::ReviewMalformed,
        Key::ReviewFailed,
        Key::ReviewReady,
        Key::ReviewPartial,
        Key::ReviewStatedGoal,
        Key::ReviewContext,
        Key::ReviewConstraints,
        Key::ReviewNonGoals,
        Key::ReviewDeliverable,
        Key::ReviewSuccessEvidence,
        Key::ReviewAssumptions,
        Key::ReviewDecisions,
        Key::ReviewPriorityCritical,
        Key::ReviewPriorityHigh,
        Key::ReviewPriorityMedium,
        Key::ReviewPriorityLow,
        Key::ReviewSourceStatement,
        Key::ReviewInference,
        Key::SendToModel,
        Key::ModelRequestWaiting,
        Key::ModelRequestDocumentClosed,
        Key::ModelRequestDocumentChanged,
        Key::Settings,
        Key::Save,
        Key::SaveAsPicker,
        Key::SaveAsSnapshotChanged,
        Key::Cancel,
        Key::SectionSkills,
        Key::Searching,
        Key::NoMatches,
        Key::Untitled,
        Key::Recent,
        Key::ModeSource,
        Key::ModeNative,
        Key::ModeWeb,
        Key::WebPreviewFailed,
        Key::Saved,
        Key::Open,
        Key::Theme,
        Key::Language_,
        Key::Translation,
        Key::Provider,
        Key::Model,
        Key::BaseUrl,
        Key::TargetLanguage,
        Key::ApiKey,
        Key::Credential,
        Key::ProviderBestAvailable,
        Key::StoreCredentialSecurely,
        Key::UseCredentialForSession,
        Key::ChooseProviderForCredential,
        Key::UnsupportedProvider,
        Key::CredentialEndpointInvalid,
        Key::CredentialDeleted,
        Key::CredentialReadFailed,
        Key::CredentialRequired,
        Key::EndpointInvalidUrl,
        Key::EndpointUnsupportedScheme,
        Key::EndpointMissingHost,
        Key::EndpointUserInfoNotAllowed,
        Key::EndpointQueryNotAllowed,
        Key::EndpointFragmentNotAllowed,
        Key::EndpointInsecureRemoteTransport,
        Key::UsageCounters,
        Key::UsageDisclosure,
        Key::UsageRetention,
        Key::UsageOff,
        Key::UsageOn,
        Key::UsageInspect,
        Key::UsageExport,
        Key::UsageClear,
        Key::UsageExportFileName,
        Key::UsageExportFormat,
        Key::UsageConsentSaved,
        Key::UsageConsentSaveFailed,
        Key::UsageUnavailable,
        Key::UsageExported,
        Key::UsageExportFailed,
        Key::UsageCleared,
        Key::UsageUtcDay,
        Key::UsageFirstLaunch,
        Key::UsageArtifactOpened,
        Key::UsageArtifactPasted,
        Key::UsageReviewStarted,
        Key::UsageReviewCompleted,
        Key::UsageClarificationAnswered,
        Key::UsageClarificationDeferred,
        Key::UsageClarificationRejected,
        Key::UsageRevisionDecision,
        Key::UsageRevisionApplied,
        Key::UsageArtifactSaved,
        Key::UsageArtifactCopied,
        Key::UsageProviderFailure,
        Key::UsageCredentialFailure,
        Key::UsageStaleResultBlocked,
        Key::UsageRecoveryRestored,
        Key::UsageAcceptedHunks,
        Key::UsageRejectedHunks,
        Key::UsageScopeDocument,
        Key::UsageScopeSelection,
        Key::UsageLensSpecification,
        Key::UsageLensPlan,
        Key::UsageDurationUnderOne,
        Key::UsageDurationOneToFive,
        Key::UsageDurationFiveToFifteen,
        Key::UsageDurationFifteenPlus,
    ];

    #[test]
    fn every_key_has_a_string_in_every_language() {
        for &key in EVERY_KEY {
            for language in Language::ALL {
                let value = t(language, key);
                assert!(
                    !value.is_empty(),
                    "{key:?} is empty in {}",
                    language.label()
                );
            }
        }
    }

    #[test]
    fn chinese_covers_every_key() {
        for &key in EVERY_KEY {
            assert!(
                !chinese(key).is_empty(),
                "{key:?} has no Chinese translation"
            );
        }
    }

    #[test]
    fn chinese_preserves_shared_english_labels() {
        // A shared technical label remains readable, not an enum variant name.
        assert_eq!(t(Language::Chinese, Key::ModeWeb), "兼容预览");
        assert!(!t(Language::Chinese, Key::ModeWeb).contains("Key::"));
    }

    #[test]
    fn languages_are_listed_under_their_own_names() {
        // A picker that says "Chinese" in English is unreadable to the person
        // who needs it.
        assert_eq!(Language::Chinese.label(), "简体中文");
        assert_eq!(Language::English.label(), "English");
    }

    #[test]
    fn language_keys_round_trip_and_are_bcp47() {
        for language in Language::ALL {
            assert_eq!(Language::from_key(language.key()), language);
            assert!(language.key().contains('-'), "{}", language.key());
        }
        // Case-insensitive, since a hand-edited settings file may say `zh-CN`.
        assert_eq!(Language::from_key("zh-CN"), Language::Chinese);
        // Unknown falls back rather than panicking.
        assert_eq!(Language::from_key("kl-GL"), Language::default());
    }

    #[test]
    fn picker_commands_use_the_platform_ellipsis_convention() {
        for key in [Key::SaveAsPicker] {
            for language in Language::ALL {
                let label = t(language, key);
                assert!(label.ends_with('\u{2026}'), "{key:?}: {label}");
                assert!(!label.ends_with("..."), "{key:?}: {label}");
            }
        }
    }

    #[test]
    fn destructive_and_recent_labels_name_the_exact_path() {
        let path = std::path::Path::new("C:/work/notes.md");
        for language in Language::ALL {
            assert!(replace_file_title(language, path).contains("notes.md"));
            assert!(replace_file_description(language, path).contains("C:/work/notes.md"));
            assert!(open_recent_target_label(language, path).contains("C:/work/notes.md"));
            assert!(remove_recent_target_label(language, path).contains("C:/work/notes.md"));
        }
    }

    #[test]
    fn model_request_disclosure_is_complete_in_each_interface_language() {
        use mt_core::model::{
            AgentSkillRequest, AgentSkillRequestEntry, OutboundScope, RevisionDisclosureDetails,
            RevisionRequestBinding,
        };

        let endpoint = EndpointIdentity::parse(
            mt_core::model::Provider::OpenAiResponses,
            Some("http://127.0.0.1:8080/custom/v1/"),
        )
        .unwrap();
        let disclosure = ModelRequestDisclosure::new(
            ModelOperation::Translation,
            endpoint.clone(),
            OutboundScope::block(37),
        );

        let english = model_request_disclosure(Language::English, &disclosure);
        for expected in [
            "Operation: Translation",
            "Endpoint: local",
            "Wire format: OpenAI Responses",
            "Identity: http://127.0.0.1:8080/custom/v1/",
            "Transport: unencrypted; proxy disabled",
            "Scope: current block, 37 UTF-8 content bytes",
            "Protocol framing crosses this boundary.\nThe disclosed source content also crosses it.",
        ] {
            assert!(
                english.contains(expected),
                "missing {expected:?} in {english:?}"
            );
        }

        let chinese = model_request_disclosure(Language::Chinese, &disclosure);
        for expected in [
            "操作：翻译",
            "端点：本地",
            "协议格式：OpenAI Responses",
            "端点标识：http://127.0.0.1:8080/custom/v1/",
            "传输：未加密；已禁用代理",
            "范围：当前块，37 UTF-8 内容字节",
            "协议封装会跨越此边界。\n上述源内容也会跨越此边界发送。",
        ] {
            assert!(
                chinese.contains(expected),
                "missing {expected:?} in {chinese:?}"
            );
        }

        let revision = ModelRequestDisclosure::revision_with_details(
            endpoint.clone(),
            OutboundScope::document(37),
            RevisionRequestBinding::new([0x03; 32], 7, 11, [0x04; 32], [0x01; 32], [0x02; 32]),
            RevisionDisclosureDetails::new(123, 45, 2),
        );
        let revision_english = model_request_disclosure(Language::English, &revision);
        for expected in [
            "Operation: Revision",
            "Scope: document, 37 UTF-8 content bytes",
            "Source scope is frozen for this Revision.",
            "Source SHA-256: 030303",
            "Source revision: 7",
            "Source generation: 11",
            "Artifact lens digest: 040404",
            "Review context digest: 010101",
            "Answers digest: 020202",
            "Review context: 123 UTF-8 JSON bytes.",
            "Answers: 2 states, 45 UTF-8 JSON bytes.",
        ] {
            assert!(
                revision_english.contains(expected),
                "missing {expected:?} in {revision_english:?}"
            );
        }

        let revision_chinese = model_request_disclosure(Language::Chinese, &revision);
        for expected in [
            "操作：修订",
            "范围：文档，37 UTF-8 内容字节",
            "此修订的源范围已冻结。",
            "源 SHA-256：030303",
            "源修订：7",
            "源代次：11",
            "工件 lens 摘要：040404",
            "审查上下文摘要：010101",
            "答案摘要：020202",
            "审查上下文：123 UTF-8 JSON 字节。",
            "答案：2 个状态，共 45 UTF-8 JSON 字节。",
        ] {
            assert!(
                revision_chinese.contains(expected),
                "missing {expected:?} in {revision_chinese:?}"
            );
        }

        let context = ModelRequestDisclosure::new(
            ModelOperation::Review,
            endpoint.clone(),
            OutboundScope::document_with_effective_agent_context(
                12,
                ["workspace AGENTS.md", "project CONTEXT.md"],
            )
            .unwrap(),
        );
        for language in Language::ALL {
            let message = model_request_disclosure(language, &context);
            assert!(message.contains("workspace AGENTS.md"));
            assert!(message.contains("project CONTEXT.md"));
        }

        let package = AgentSkillRequest::new(
            vec![
                AgentSkillRequestEntry::new("SKILL.md", "entrypoint", b"root".to_vec()).unwrap(),
                AgentSkillRequestEntry::new(
                    "references/guide.md",
                    "referenced support",
                    b"guide".to_vec(),
                )
                .unwrap(),
            ],
            vec![
                mt_core::model::AgentSkillOmission::new(
                    "references/private.md",
                    "excluded by policy",
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let package = package.disclosure(ModelOperation::Review, endpoint);
        for language in Language::ALL {
            let message = model_request_disclosure(language, &package);
            for expected in [
                "SKILL.md (4 bytes): entrypoint",
                "references/guide.md (5 bytes): referenced support",
            ] {
                assert!(
                    message.contains(expected),
                    "missing {expected:?} in {message:?}"
                );
            }
        }
        assert!(model_request_disclosure(Language::English, &package).contains("partial"));
        assert!(model_request_disclosure(Language::Chinese, &package).contains("部分"));
    }

    #[test]
    fn local_https_status_discloses_encryption_and_disabled_proxy_in_both_languages() {
        let endpoint = EndpointIdentity::parse(
            mt_core::model::Provider::OpenAiChat,
            Some("https://127.0.0.1:8443/v1/"),
        )
        .unwrap();

        let english = model_endpoint_status(Language::English, &endpoint);
        assert!(english.contains("Transport is encrypted"), "{english}");
        assert!(english.contains("proxy is disabled"), "{english}");

        let chinese = model_endpoint_status(Language::Chinese, &endpoint);
        assert!(chinese.contains("传输已加密"), "{chinese}");
        assert!(chinese.contains("已禁用代理"), "{chinese}");
    }

    #[test]
    fn english_is_the_default() {
        // Not because it is the better language, but because it is the one this
        // app's own strings are authored in, so it is the only one guaranteed
        // complete.
        assert_eq!(Language::default(), Language::English);
    }

    #[test]
    fn credential_settings_strings_are_translated_and_content_free() {
        for language in Language::ALL {
            for key in [
                Key::Credential,
                Key::StoreCredentialSecurely,
                Key::UseCredentialForSession,
                Key::CredentialDeleted,
            ] {
                let value = t(language, key);
                assert!(!value.is_empty(), "{key:?} in {language:?}");
                assert!(!value.contains("sentinel-secret"));
            }
        }

        assert_ne!(
            t(Language::English, Key::StoreCredentialSecurely),
            t(Language::Chinese, Key::StoreCredentialSecurely)
        );
    }
}
