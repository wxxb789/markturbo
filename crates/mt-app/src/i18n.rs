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
    OpenFolder,
    OpenFile,
    OpenFolderPicker,
    OpenFilePicker,
    NewDocument,
    Paste,
    Workspace,
    Clear,
    Close,
    Quit,
    AgentContext,
    EffectiveAgentContext,
    Refresh,
    SearchFiles,
    ShowingFirstResults,
    ResolvingInstructions,
    Reviewing,
    Revising,
    SendRevision,
    PrepareRevision,
    ApplyApproved,
    Approved,
    Rejected,
    SaveConflict,
    SaveConflictDetail,
    Opened,
    SettingsSaved,
    TrustScripts,
    ScriptsTrusted,
    ScriptsRestricted,
    MoreLayouts,
    FewerLayouts,
    ReviewWithContext,
    RestoreAll,
    DiscardAll,
    RestoredDocuments,
    RestoreKeepsSource,
    SaveAll,
    QuitTitle,
    QuitDescription,
    QuitSaveFailed,
    OpenSample,
    File,
    Assumption,
    Instructions,
    System,
    Light,
    Dark,
    Use,
    SaveSettings,
    KeyStoredForSession,
    KeyStoredSecurely,
    ClipboardTextUnavailable,
    OpenBundledSample,
    BundledSampleUnavailable,
    Translate,
    Review,
    ReviewDocument,
    ReviewSelection,
    ReviewEmptySelection,
    ReviewLens,
    ReviewLensPrompt,
    ReviewLensSpecification,
    ReviewLensAgentInstructions,
    ReviewLensAgentSkill,
    ReviewLensCorrection,
    ReviewReadOnly,
    ReviewSelectionContextOmitted,
    ReviewWaiting,
    ReviewCancelled,
    ReviewDocumentClosed,
    ReviewDocumentChanged,
    ReviewStale,
    ReviewMalformed,
    ReviewOversized,
    ReviewSkillPackageUnavailable,
    ReviewSkillPackageEntrypointMissing,
    ReviewSkillPackageSelectionUnsupported,
    ReviewSkillPackageReadFailed,
    ReviewSkillPackageChanged,
    ReviewSkillPackageNonUtf8Path,
    ReviewSkillPackageOversized,
    ReviewNoProvider,
    ReviewMissingCredential,
    ReviewUnavailable,
    ReviewTimeout,
    ReviewFailed,
    ReviewReady,
    ReviewPartial,
    ReviewNoResult,
    ReviewWhatIUnderstand,
    ReviewRequestedModel,
    ReviewResponseModel,
    ReviewStatedGoal,
    ReviewContext,
    ReviewConstraints,
    ReviewNonGoals,
    ReviewDeliverable,
    ReviewSuccessEvidence,
    ReviewAssumptions,
    ReviewDecisions,
    ReviewFindings,
    ReviewNoFindings,
    ReviewQuestions,
    ReviewNoQuestions,
    ReviewPriority,
    ReviewPriorityCritical,
    ReviewPriorityHigh,
    ReviewPriorityMedium,
    ReviewPriorityLow,
    ReviewSourceStatement,
    ReviewInference,
    ReviewDocumentWide,
    ReviewAnchor,
    ReviewRun,
    ReviewDismiss,

    // Approved Revision workflow
    RevisionAnswers,
    RevisionRecoveredAnswers,
    RevisionCopyRecoveredAnswers,
    RevisionDiscardRecoveredAnswers,
    RevisionRecoveredAnswersCopied,
    RevisionAnswerUnanswered,
    RevisionAnswerIntentionallyUnspecified,
    RevisionAnswerAnswered,
    RevisionRun,
    RevisionDiscardAnswers,
    RevisionRetry,
    RevisionDismiss,
    RevisionConsentTitle,
    RevisionWaitingForConsent,
    RevisionRunning,
    RevisionPreparationFailed,
    RevisionAuthorizationFailed,
    RevisionProviderFailed,
    RevisionApplyFailed,
    RevisionSourceChanged,
    RevisionScopeChangedDuringConsent,
    RevisionDocumentClosed,
    RevisionConsentCancelledAnswersRetained,
    RevisionCancelledAnswersRetained,
    RevisionAnswersRetained,
    RevisionAnswersDiscarded,
    RevisionRecoveryRetirementFailed,
    RevisionStale,
    RevisionStaleInspection,
    RevisionReady,
    RevisionReadyStale,
    RevisionApplied,
    RevisionAppliedSavePending,
    RevisionNoApprovedChanges,
    RevisionCopied,
    RevisionResult,
    RevisionChange,
    RevisionChangeAccepted,
    RevisionChangeRejected,
    RevisionRationale,
    RevisionAccept,
    RevisionReject,
    RevisionAcceptAll,
    RevisionRejectAll,
    RevisionCopy,
    RevisionApply,
    RevisionFinalPreview,
    RevisionFinalPreviewSource,
    RevisionCoverage,
    RevisionCoverageValidated,
    RevisionCoverageQuestion,
    RevisionCoverageRepresented,
    RevisionCoverageIntentionallyOmitted,
    RevisionCoverageNotAddressed,
    SendToModel,
    ModelRequestConsentTitle,
    ModelRequestWaiting,
    ModelRequestCancelled,
    ModelRequestDocumentClosed,
    ModelRequestDocumentChanged,
    Settings,
    Save,
    SaveAsPicker,
    SaveAsSnapshotChanged,
    Replace,
    Cancel,

    // Side panels
    PanelFiles,
    PanelSearch,
    PanelHarness,
    PanelOutline,
    SectionSkills,
    SectionInstructions,
    GroupBy,
    Rescan,
    ViewLayout,
    Details,
    ToggleLeftPanel,
    ToggleRightPanel,
    SidePanelWidth,
    DetailsPanelWidth,
    CopyPath,
    CopyRelativePath,

    // Search
    ScopeDocument,
    ScopeOpenTabs,
    ScopeFolder,
    ScopeHarness,
    Searching,
    TypeToSearch,
    NoMatches,

    // Tabs
    Untitled,
    RecoveredSnapshot,
    UnsavedChanges,
    NavigateBack,
    NavigateForward,

    // Empty states
    OpenFolderToBegin,
    OpenFolderToDiscover,
    OpenDocumentForOutline,
    NoHeadings,
    OpenAMarkdownFile,
    Scanning,
    WelcomeTitle,
    WelcomeSubtitle,
    Recent,
    RecentMissing,
    RecentUnavailable,
    DontShowWelcomeAgain,

    // Document view
    ModeSource,
    ModeNative,
    ModeWeb,
    ModeSplitNative,
    ModeSplitWeb,
    WebPreviewFailed,
    WebPreviewRetry,
    TrustThisDocument,
    Trusted,
    HtmlNeedsTrust,
    FileChangedOnDisk,
    ReloadFromDisk,
    Overwrite,

    // Status bar
    Watching,
    AutoRefresh,
    AutoRefreshOn,

    // Inspector fields
    Origin,
    Location,
    DiscoveredIn,
    AlsoLinkedFrom,
    Files,
    Validation,
    Kind,
    Status,
    ChangedOnDisk,
    Saved,
    Open,
    OpenSkillMd,

    // Settings pages
    Appearance,
    Theme,
    Mode,
    LightTheme,
    DarkTheme,
    Language_,
    Translation,
    ModelConfiguration,
    Provider,
    Model,
    BaseUrl,
    EndpointStatus,
    TargetLanguage,
    Skills,
    Discovery,
    IncludeGlobalSkills,
    ShowInternalSkills,
    SyncScrolling,
    ApiKey,
    Credentials,
    Credential,
    EnvironmentCredential,
    LegacyCredential,
    Editor,
    SplitView,
    ProviderBestAvailable,

    // Credential commands and status
    StoreCredentialSecurely,
    UseCredentialForSession,
    TestCredential,
    DeleteCredential,
    MigrateLegacyCredential,
    DeleteCredentialTitle,
    MigrateLegacyCredentialTitle,
    ChooseProviderForCredential,
    UnsupportedProvider,
    CredentialEndpointInvalid,
    SecureStoreUnavailable,
    CredentialStoredPresent,
    CredentialStoredAbsent,
    CredentialStoredSecurely,
    CredentialSessionActive,
    CredentialTestRequested,
    CredentialDeleted,
    LegacyCredentialMigrated,
    CredentialStoring,
    CredentialDeleting,
    CredentialMigrating,
    CredentialReadFailed,
    CredentialWriteFailed,
    CredentialVerifyFailed,
    CredentialDeleteFailed,
    CredentialInvalidEncoding,
    CredentialRequired,
    CredentialMigrationSaveFailed,
    EnvironmentCredentialAuthorizationSaveFailed,
    SettingsPathUnavailable,
    SaveEndpointBeforeCredential,

    // Endpoint validation
    EndpointInvalidUrl,
    EndpointUnsupportedScheme,
    EndpointMissingHost,
    EndpointUserInfoNotAllowed,
    EndpointQueryNotAllowed,
    EndpointFragmentNotAllowed,
    EndpointInsecureRemoteTransport,

    // Settings descriptions
    //
    // Their own block: a description is the sentence that says what a toggle
    // costs, and leaving fourteen of them hard-coded English while the label
    // above them translated was the split this replaces.
    ModeHelp,
    LightThemeHelp,
    DarkThemeHelp,
    LanguageHelp,
    ProviderHelp,
    ApiKeyHelp,
    CredentialHelp,
    LegacyCredentialHelp,
    BaseUrlHelp,
    ModelHelp,
    TargetLanguageHelp,
    SyncScrollingHelp,
    AutoRefreshHelp,
    IncludeGlobalSkillsHelp,
    ShowInternalSkillsHelp,
    GroupByHelp,

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
        Key::OpenFolder => "Open Folder",
        Key::OpenFile => "Open File",
        Key::OpenFolderPicker => "Open Folder\u{2026}",
        Key::OpenFilePicker => "Open File\u{2026}",
        Key::NewDocument => "New",
        Key::Paste => "Paste",
        Key::Workspace => "Workspace",
        Key::Clear => "Clear",
        Key::Close => "Close",
        Key::Quit => "Quit",
        Key::AgentContext => "Agent Context",
        Key::EffectiveAgentContext => "Effective Agent Context",
        Key::Refresh => "Refresh",
        Key::SearchFiles => "Search files",
        Key::ShowingFirstResults => "Showing the first results only.",
        Key::ResolvingInstructions => "Resolving instructions…",
        Key::Reviewing => "Reviewing…",
        Key::Revising => "Revising…",
        Key::SendRevision => "Send revision",
        Key::PrepareRevision => "Prepare revision",
        Key::ApplyApproved => "Apply approved",
        Key::Approved => "Approved",
        Key::Rejected => "Rejected",
        Key::SaveConflict => "Save conflict",
        Key::SaveConflictDetail => {
            "Save refused: the file changed on disk. Reload, overwrite, or save as a new file."
        }
        Key::Opened => "Opened",
        Key::SettingsSaved => "Settings saved.",
        Key::TrustScripts => "Trust scripts",
        Key::ScriptsTrusted => "Trusted: scripts may run",
        Key::ScriptsRestricted => "Restricted: scripts disabled",
        Key::MoreLayouts => "More layouts",
        Key::FewerLayouts => "Fewer layouts",
        Key::ReviewWithContext => "Review with selected context",
        Key::RestoreAll => "Restore all",
        Key::DiscardAll => "Discard all",
        Key::RestoredDocuments => "Unsaved documents recovered from a previous session",
        Key::RestoreKeepsSource => "Restore keeps the original file untouched until you save.",
        Key::SaveAll => "Save all",
        Key::QuitTitle => "Quit MarkTurbo?",
        Key::QuitDescription => "Some documents have unsaved changes.",
        Key::QuitSaveFailed => "Quit cancelled: a document could not be saved.",
        Key::OpenSample => "Open sample",
        Key::File => "File",
        Key::Assumption => "Assumption",
        Key::Instructions => "Instruction files",
        Key::System => "System",
        Key::Light => "Light",
        Key::Dark => "Dark",
        Key::Use => "Use",
        Key::SaveSettings => "Save",
        Key::KeyStoredForSession => "Key stored for this session.",
        Key::KeyStoredSecurely => "Key stored securely.",
        Key::ClipboardTextUnavailable => "Clipboard does not contain text or is unavailable.",
        Key::OpenBundledSample => "Open Bundled Sample",
        Key::BundledSampleUnavailable => "The bundled sample could not be opened.",
        Key::Translate => "Translate",
        Key::Review => "Review",
        Key::ReviewDocument => "Review document",
        Key::ReviewSelection => "Review selection",
        Key::ReviewEmptySelection => "Select some text before starting a selection Review.",
        Key::ReviewLens => "Artifact lens",
        Key::ReviewLensPrompt => "Prompt",
        Key::ReviewLensSpecification => "Specification / Plan",
        Key::ReviewLensAgentInstructions => "Agent Instructions",
        Key::ReviewLensAgentSkill => "Agent Skill",
        Key::ReviewLensCorrection => "Correct lens",
        Key::ReviewReadOnly => "Review is read-only. Your source text will not be changed.",
        Key::ReviewSelectionContextOmitted => {
            "Selection review omits surrounding document context. It does not describe the complete artifact."
        }
        Key::ReviewWaiting => "Waiting for review request approval…",
        Key::ReviewCancelled => "Review cancelled",
        Key::ReviewDocumentClosed => "Review cancelled because the document closed",
        Key::ReviewDocumentChanged => {
            "The document changed while approval was pending. Review the updated scope and try again."
        }
        Key::ReviewStale => "This review describes an older source revision and is stale.",
        Key::ReviewMalformed => {
            "The model returned malformed Review data; nothing was shown as trusted output."
        }
        Key::ReviewOversized => "The Review payload exceeds the configured request-size limit.",
        Key::ReviewSkillPackageUnavailable => "The Agent Skill package is unavailable for Review.",
        Key::ReviewSkillPackageEntrypointMissing => "The Agent Skill entrypoint is missing.",
        Key::ReviewSkillPackageSelectionUnsupported => {
            "Agent Skill Review covers the whole package. Review the document or choose another lens."
        }
        Key::ReviewSkillPackageReadFailed => "The Agent Skill package could not be read.",
        Key::ReviewSkillPackageChanged => {
            "The Agent Skill package changed while Review was being prepared. Try again."
        }
        Key::ReviewSkillPackageNonUtf8Path => {
            "The Agent Skill package contains a path that cannot be sent."
        }
        Key::ReviewSkillPackageOversized => {
            "The Agent Skill package exceeds the Review source size limit."
        }
        Key::ReviewNoProvider => "Review is unavailable because no model provider is configured.",
        Key::ReviewMissingCredential => {
            "Review is unavailable because the configured model provider has no credential."
        }
        Key::ReviewUnavailable => {
            "Review is unavailable. Check the model configuration and try again."
        }
        Key::ReviewTimeout => "Review timed out. The source document was not changed.",
        Key::ReviewFailed => "Review failed. The source document was not changed.",
        Key::ReviewReady => "Review ready",
        Key::ReviewPartial => "Partial review",
        Key::ReviewNoResult => "Run Review to inspect this artifact.",
        Key::ReviewWhatIUnderstand => "What I understand",
        Key::ReviewRequestedModel => "Requested model",
        Key::ReviewResponseModel => "Response model",
        Key::ReviewStatedGoal => "Stated goal",
        Key::ReviewContext => "Relevant context and inputs",
        Key::ReviewConstraints => "Constraints and non-goals",
        Key::ReviewNonGoals => "Non-goals",
        Key::ReviewDeliverable => "Expected deliverable",
        Key::ReviewSuccessEvidence => "Success evidence",
        Key::ReviewAssumptions => "Inferred assumptions",
        Key::ReviewDecisions => "Unresolved or contradictory decisions",
        Key::ReviewFindings => "Findings",
        Key::ReviewNoFindings => "No source-grounded findings.",
        Key::ReviewQuestions => "Clarification questions",
        Key::ReviewNoQuestions => "No consequential clarification questions.",
        Key::ReviewPriority => "Priority",
        Key::ReviewPriorityCritical => "Critical",
        Key::ReviewPriorityHigh => "High",
        Key::ReviewPriorityMedium => "Medium",
        Key::ReviewPriorityLow => "Low",
        Key::ReviewSourceStatement => "Source statement",
        Key::ReviewInference => "Inference",
        Key::ReviewDocumentWide => "Document-wide",
        Key::ReviewAnchor => "Source anchor",
        Key::ReviewRun => "Run Review",
        Key::ReviewDismiss => "Dismiss Review",

        Key::RevisionAnswers => "Revision answers",
        Key::RevisionRecoveredAnswers => "Recovered Revision answers",
        Key::RevisionCopyRecoveredAnswers => "Copy recovered answers",
        Key::RevisionDiscardRecoveredAnswers => "Discard recovered answers",
        Key::RevisionRecoveredAnswersCopied => "Recovered Revision answers copied",
        Key::RevisionAnswerUnanswered => "Unanswered",
        Key::RevisionAnswerIntentionallyUnspecified => "Intentionally unspecified",
        Key::RevisionAnswerAnswered => "Answered",
        Key::RevisionRun => "Run Revision",
        Key::RevisionDiscardAnswers => "Discard Revision answers",
        Key::RevisionRetry => "Retry",
        Key::RevisionDismiss => "Dismiss",
        Key::RevisionConsentTitle => "Revision consent",
        Key::RevisionWaitingForConsent => "Revision waiting for consent",
        Key::RevisionRunning => "Revision running",
        Key::RevisionPreparationFailed => "Revision preparation failed",
        Key::RevisionAuthorizationFailed => "Revision authorization failed",
        Key::RevisionProviderFailed => "Revision provider failed",
        Key::RevisionApplyFailed => "Revision could not be applied",
        Key::RevisionSourceChanged => {
            "The reviewed source changed. Rerun Review before requesting a Revision."
        }
        Key::RevisionScopeChangedDuringConsent => {
            "The reviewed source or Agent Skill package changed while consent was open."
        }
        Key::RevisionDocumentClosed => "The document closed before the Revision completed.",
        Key::RevisionConsentCancelledAnswersRetained => {
            "Revision consent was cancelled; answers were retained."
        }
        Key::RevisionCancelledAnswersRetained => "Revision cancelled; answers were retained.",
        Key::RevisionAnswersRetained => "Revision answers are retained.",
        Key::RevisionAnswersDiscarded => "Revision answers discarded",
        Key::RevisionRecoveryRetirementFailed => {
            "Could not clear the recovery checkpoint. Recovered Revision answers were retained."
        }
        Key::RevisionStale => "Revision is stale and cannot be applied. Rerun Review first.",
        Key::RevisionStaleInspection => "Revision result is stale and inspectable only",
        Key::RevisionReady => "Revision ready for inspection",
        Key::RevisionReadyStale => "Revision ready for inspection, but stale",
        Key::RevisionApplied => "Approved Revision applied",
        Key::RevisionAppliedSavePending => "Approved Revision applied; save is still pending",
        Key::RevisionNoApprovedChanges => "No approved changes to apply",
        Key::RevisionCopied => "Copied approved Revision preview",
        Key::RevisionResult => "Revision result",
        Key::RevisionChange => "Revision change",
        Key::RevisionChangeAccepted => "accepted",
        Key::RevisionChangeRejected => "rejected",
        Key::RevisionRationale => "Rationale",
        Key::RevisionAccept => "Accept",
        Key::RevisionReject => "Reject",
        Key::RevisionAcceptAll => "Accept all",
        Key::RevisionRejectAll => "Reject all",
        Key::RevisionCopy => "Copy",
        Key::RevisionApply => "Apply",
        Key::RevisionFinalPreview => "Final preview",
        Key::RevisionFinalPreviewSource => "Final approved Revision source",
        Key::RevisionCoverage => "Question coverage",
        Key::RevisionCoverageValidated => "validated question coverage entries",
        Key::RevisionCoverageQuestion => "Question",
        Key::RevisionCoverageRepresented => "represented by",
        Key::RevisionCoverageIntentionallyOmitted => "intentionally omitted",
        Key::RevisionCoverageNotAddressed => "not addressed",
        Key::SendToModel => "Send",
        Key::ModelRequestConsentTitle => "Send content to model?",
        Key::ModelRequestWaiting => "Waiting for model request approval…",
        Key::ModelRequestCancelled => "Model request cancelled",
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
        Key::Replace => "Replace",
        Key::Cancel => "Cancel",

        Key::PanelFiles => "Files",
        Key::PanelSearch => "Search",
        Key::PanelHarness => "Agent Context",
        Key::PanelOutline => "Outline",
        Key::SectionSkills => "Skills",
        Key::SectionInstructions => "Instructions",
        Key::GroupBy => "Group by",
        Key::Rescan => "Rescan",
        Key::ViewLayout => "View",
        Key::Details => "Details",
        Key::ToggleLeftPanel => "Toggle the side panel",
        Key::ToggleRightPanel => "Toggle the details panel",
        Key::SidePanelWidth => "Side panel width",
        Key::DetailsPanelWidth => "Details panel width",
        Key::CopyPath => "Copy path",
        Key::CopyRelativePath => "Copy relative path",

        Key::ScopeDocument => "This file",
        Key::ScopeOpenTabs => "Open tabs",
        Key::ScopeFolder => "Folder",
        Key::ScopeHarness => "Harness",
        Key::Searching => "Searching…",
        Key::TypeToSearch => "Type to search.",
        Key::NoMatches => "No matches.",

        Key::Untitled => "Untitled",
        Key::RecoveredSnapshot => "Recovered snapshot",
        Key::UnsavedChanges => "Unsaved changes",
        Key::NavigateBack => "Back",
        Key::NavigateForward => "Forward",

        Key::OpenFolderToBegin => "Open a folder to begin.",
        Key::OpenFolderToDiscover => "Open a folder to discover skills and instruction files.",
        Key::OpenDocumentForOutline => "Open a document to see its outline.",
        Key::NoHeadings => "This document has no headings.",
        Key::OpenAMarkdownFile => "Open a Markdown file to begin.",
        Key::Scanning => "Scanning…",
        Key::WelcomeTitle => "Start a Markdown document",
        Key::WelcomeSubtitle => "Create a new document or open one from your computer.",
        Key::Recent => "Recent",
        Key::RecentMissing => "Missing",
        Key::RecentUnavailable => "Unavailable",
        Key::DontShowWelcomeAgain => "Don't show this again",

        Key::ModeSource => "Edit",
        Key::ModeNative => "Preview",
        Key::ModeWeb => "Compatibility Preview",
        Key::ModeSplitNative => "Split · Preview",
        Key::ModeSplitWeb => "Split · Compatibility",
        Key::WebPreviewFailed => "The Web preview could not be opened.",
        Key::WebPreviewRetry => "Retry Web preview",
        Key::TrustThisDocument => "Trust this document",
        Key::Trusted => "Trusted ✓",
        Key::HtmlNeedsTrust => {
            "This HTML file is shown in a sandbox, so images and stylesheets it loads from disk \
             are blocked. Trust this document to load them."
        }
        Key::FileChangedOnDisk => "This file changed on disk since it was opened.",
        Key::ReloadFromDisk => "Reload from disk",
        Key::Overwrite => "Overwrite",

        Key::Watching => "Watching",
        Key::AutoRefresh => "Auto-refresh on external change",
        Key::AutoRefreshOn => "Auto-refresh is on",

        Key::Origin => "Origin",
        Key::Location => "Location",
        Key::DiscoveredIn => "Discovered in",
        Key::AlsoLinkedFrom => "Also linked from",
        Key::Files => "Files",
        Key::Validation => "Validation",
        Key::Kind => "Kind",
        Key::Status => "Status",
        Key::ChangedOnDisk => "Changed on disk",
        Key::Saved => "Saved",
        Key::Open => "Open",
        Key::OpenSkillMd => "Open SKILL.md",

        Key::Appearance => "Appearance",
        Key::Theme => "Theme",
        Key::Mode => "Mode",
        Key::LightTheme => "Light theme",
        Key::DarkTheme => "Dark theme",
        Key::Language_ => "Language",
        Key::Translation => "Translation",
        Key::ModelConfiguration => "Model configuration",
        Key::Provider => "Provider",
        Key::Model => "Model",
        Key::BaseUrl => "Base URL",
        Key::EndpointStatus => "Endpoint status",
        Key::TargetLanguage => "Target language",
        Key::Skills => "Skills",
        Key::Discovery => "Discovery",
        Key::IncludeGlobalSkills => "Include global skills",
        Key::ShowInternalSkills => "Show internal skills",
        Key::SyncScrolling => "Sync scrolling in Split",
        Key::ApiKey => "API key",
        Key::Credentials => "Credentials",
        Key::Credential => "Credential",
        Key::EnvironmentCredential => "Environment credential",
        Key::LegacyCredential => "Legacy plaintext credential",
        Key::Editor => "Editor",
        Key::SplitView => "Split view",
        Key::ProviderBestAvailable => "Best available",

        Key::StoreCredentialSecurely => "Store securely",
        Key::UseCredentialForSession => "Use for session",
        Key::TestCredential => "Test",
        Key::DeleteCredential => "Delete",
        Key::MigrateLegacyCredential => "Migrate securely",
        Key::DeleteCredentialTitle => "Delete this credential?",
        Key::MigrateLegacyCredentialTitle => "Migrate the plaintext credential?",
        Key::ChooseProviderForCredential => "Choose a provider before configuring credentials.",
        Key::UnsupportedProvider => "The configured provider is not supported.",
        Key::CredentialEndpointInvalid => "Fix the endpoint before configuring credentials.",
        Key::SecureStoreUnavailable => {
            "Secure storage is unavailable. Use a session or environment credential."
        }
        Key::CredentialStoredPresent => "A credential is stored securely for this endpoint.",
        Key::CredentialStoredAbsent => {
            "No credential is stored securely for this endpoint. A session or environment credential may still be active."
        }
        Key::CredentialStoredSecurely => "Credential stored securely.",
        Key::CredentialSessionActive => "Credential is active for this session only.",
        Key::CredentialTestRequested => "Credential test requested.",
        Key::CredentialDeleted => "Credential deleted for this endpoint.",
        Key::LegacyCredentialMigrated => "Plaintext credential migrated securely.",
        Key::CredentialStoring => "Storing credential securely…",
        Key::CredentialDeleting => "Deleting credential…",
        Key::CredentialMigrating => "Migrating credential…",
        Key::CredentialReadFailed => "The secure credential could not be read.",
        Key::CredentialWriteFailed => "The secure credential could not be written.",
        Key::CredentialVerifyFailed => "The secure credential write could not be verified.",
        Key::CredentialDeleteFailed => "The secure credential could not be deleted.",
        Key::CredentialInvalidEncoding => "The secure credential is not valid UTF-8.",
        Key::CredentialRequired => "Enter a credential first.",
        Key::CredentialMigrationSaveFailed => {
            "The credential was stored securely, but the plaintext setting could not be removed."
        }
        Key::EnvironmentCredentialAuthorizationSaveFailed => {
            "The environment credential authorization could not be saved. The previous authorization remains unchanged."
        }
        Key::SettingsPathUnavailable => "The settings path is unavailable.",
        Key::SaveEndpointBeforeCredential => "Save the endpoint before managing its credential.",

        Key::EndpointInvalidUrl => "The endpoint URL is invalid.",
        Key::EndpointUnsupportedScheme => "The endpoint must use HTTP or HTTPS.",
        Key::EndpointMissingHost => "The endpoint URL is missing a host.",
        Key::EndpointUserInfoNotAllowed => "The endpoint URL cannot contain user information.",
        Key::EndpointQueryNotAllowed => "The endpoint URL cannot contain query parameters.",
        Key::EndpointFragmentNotAllowed => "The endpoint URL cannot contain a fragment.",
        Key::EndpointInsecureRemoteTransport => "Remote endpoints must use HTTPS.",

        Key::ModeHelp => {
            "System follows the operating system, and keeps following it while the app is \
             running."
        }
        Key::LightThemeHelp => "Used whenever the effective mode is light.",
        Key::DarkThemeHelp => "Used whenever the effective mode is dark.",
        Key::LanguageHelp => {
            "The language of the interface. Separate from the translation target below, which \
             is about documents."
        }
        Key::ProviderHelp => {
            "The wire format to speak, not the vendor. Choose one explicitly to manage its \
             credential; Best available selects a configured provider when an operation starts. \
             Anthropic uses ANTHROPIC_API_KEY and both OpenAI formats use OPENAI_API_KEY at their \
             vendor-default endpoints."
        }
        Key::ApiKeyHelp => {
            "Enter a replacement credential. It remains only in this masked field until you \
             store it securely or use it for this session, and a stored value is never shown."
        }
        Key::CredentialHelp => {
            "Enter a replacement credential. It remains only in this masked field until you \
             store it securely or use it for this session, and a stored value is never shown."
        }
        Key::LegacyCredentialHelp => {
            "An older settings file still contains a plaintext credential. Migration writes and \
             verifies secure storage before removing that plaintext value."
        }
        Key::BaseUrlHelp => {
            "Leave empty for the provider default. Custom remote endpoints require HTTPS; HTTP \
             is allowed only for a verified loopback address and uses no proxy. Include the API \
             base path, for example `http://127.0.0.1:11434/v1`."
        }
        Key::ModelHelp => {
            "Leave empty to use MARKTURBO_MODEL, then MARKTURBO_TRANSLATE_MODEL, then the provider default."
        }
        Key::TargetLanguageHelp => "A language name or code, e.g. `zh`, `ja`, `German`.",
        Key::SyncScrollingHelp => {
            "Scroll the preview to follow the editor, and to follow an outline click. The \
             mapping is proportional, so a document with one tall diagram moves further than \
             the eye expects."
        }
        Key::AutoRefreshHelp => {
            "Re-read a document when its file changes on disk. A tab with unsaved edits is \
             never refreshed — it keeps the reload/overwrite banner, because an automatic \
             refresh must not discard typed text."
        }
        Key::IncludeGlobalSkillsHelp => {
            "Search every harness's global directory (~/.claude/skills, ~/.agents/skills, …) \
             as well as this workspace."
        }
        Key::ShowInternalSkillsHelp => {
            "Skills marked `metadata.internal: true`, which the reference tooling hides by \
             default."
        }
        Key::GroupByHelp => "How the Skills list is organized.",
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
        Key::OpenFolder => "打开文件夹",
        Key::OpenFile => "打开文件",
        Key::OpenFolderPicker => "打开文件夹\u{2026}",
        Key::OpenFilePicker => "打开文件\u{2026}",
        Key::NewDocument => "新建",
        Key::Paste => "粘贴",
        Key::Workspace => "工作区",
        Key::Clear => "清除",
        Key::Close => "关闭",
        Key::Quit => "退出",
        Key::AgentContext => "代理上下文",
        Key::EffectiveAgentContext => "有效代理上下文",
        Key::Refresh => "刷新",
        Key::SearchFiles => "搜索文件",
        Key::ShowingFirstResults => "仅显示前几条结果。",
        Key::ResolvingInstructions => "正在解析指令…",
        Key::Reviewing => "正在审查…",
        Key::Revising => "正在修订…",
        Key::SendRevision => "发送修订",
        Key::PrepareRevision => "准备修订",
        Key::ApplyApproved => "应用已批准修改",
        Key::Approved => "已批准",
        Key::Rejected => "已拒绝",
        Key::SaveConflict => "保存冲突",
        Key::SaveConflictDetail => {
            "保存被拒绝：磁盘上的文件已更改。请重新加载、覆盖或另存为新文件。"
        }
        Key::Opened => "已打开",
        Key::SettingsSaved => "设置已保存。",
        Key::TrustScripts => "信任脚本",
        Key::ScriptsTrusted => "已信任：允许运行脚本",
        Key::ScriptsRestricted => "受限：脚本已禁用",
        Key::MoreLayouts => "更多布局",
        Key::FewerLayouts => "更少布局",
        Key::ReviewWithContext => "使用所选上下文审查",
        Key::RestoreAll => "全部恢复",
        Key::DiscardAll => "全部丢弃",
        Key::RestoredDocuments => "已从上次会话恢复未保存的文档",
        Key::RestoreKeepsSource => "恢复的文本在保存前不会改动原文件。",
        Key::SaveAll => "全部保存",
        Key::QuitTitle => "退出 MarkTurbo？",
        Key::QuitDescription => "部分文档有未保存的更改。",
        Key::QuitSaveFailed => "已取消退出：文档无法保存。",
        Key::OpenSample => "打开示例",
        Key::File => "文件",
        Key::Assumption => "假设",
        Key::Instructions => "指令文件",
        Key::System => "跟随系统",
        Key::Light => "浅色",
        Key::Dark => "深色",
        Key::Use => "使用",
        Key::SaveSettings => "保存",
        Key::KeyStoredForSession => "密钥已保存到当前会话。",
        Key::KeyStoredSecurely => "密钥已安全保存。",
        Key::ClipboardTextUnavailable => "剪贴板中没有文本，或剪贴板当前不可用。",
        Key::OpenBundledSample => "打开内置示例",
        Key::BundledSampleUnavailable => "无法打开内置示例。",
        Key::Translate => "翻译",
        Key::Review => "审查",
        Key::ReviewDocument => "审查文档",
        Key::ReviewSelection => "审查选区",
        Key::ReviewEmptySelection => "请先选择文本，再开始选区审查。",
        Key::ReviewLens => "工件视角",
        Key::ReviewLensPrompt => "Prompt",
        Key::ReviewLensSpecification => "Specification / Plan",
        Key::ReviewLensAgentInstructions => "Agent Instructions",
        Key::ReviewLensAgentSkill => "Agent Skill",
        Key::ReviewLensCorrection => "修正视角",
        Key::ReviewReadOnly => "审查是只读操作，不会修改源文本。",
        Key::ReviewSelectionContextOmitted => "选区审查不包含文档周围的上下文，不能代表完整工件。",
        Key::ReviewWaiting => "正在等待审查请求授权…",
        Key::ReviewCancelled => "已取消审查",
        Key::ReviewDocumentClosed => "文档已关闭，审查已取消",
        Key::ReviewDocumentChanged => "等待授权期间文档已更改。请检查更新后的范围并重试。",
        Key::ReviewStale => "此审查描述的是较旧的源版本，已过期。",
        Key::ReviewMalformed => "模型返回的审查数据格式无效，未显示为可信结果。",
        Key::ReviewOversized => "审查请求超出配置的大小限制。",
        Key::ReviewSkillPackageUnavailable => "此 Agent Skill 包当前无法用于审查。",
        Key::ReviewSkillPackageEntrypointMissing => "缺少 Agent Skill 入口文件。",
        Key::ReviewSkillPackageSelectionUnsupported => {
            "Agent Skill 审查覆盖整个包。请审查文档，或选择其他视角。"
        }
        Key::ReviewSkillPackageReadFailed => "无法读取 Agent Skill 包。",
        Key::ReviewSkillPackageChanged => "准备审查期间 Agent Skill 包已更改。请重试。",
        Key::ReviewSkillPackageNonUtf8Path => "Agent Skill 包包含无法发送的路径。",
        Key::ReviewSkillPackageOversized => "Agent Skill 包超过审查源内容大小限制。",
        Key::ReviewNoProvider => "未配置模型服务商，无法进行审查。",
        Key::ReviewMissingCredential => "已配置的模型服务商缺少凭据，无法进行审查。",
        Key::ReviewUnavailable => "审查不可用。请检查模型配置后重试。",
        Key::ReviewTimeout => "审查超时，源文档未被修改。",
        Key::ReviewFailed => "审查失败，源文档未被修改。",
        Key::ReviewReady => "审查已就绪",
        Key::ReviewPartial => "部分审查",
        Key::ReviewNoResult => "运行审查以检查此工件。",
        Key::ReviewWhatIUnderstand => "我的理解",
        Key::ReviewRequestedModel => "请求模型",
        Key::ReviewResponseModel => "响应模型",
        Key::ReviewStatedGoal => "明确目标",
        Key::ReviewContext => "相关上下文和输入",
        Key::ReviewConstraints => "约束与非目标",
        Key::ReviewNonGoals => "非目标",
        Key::ReviewDeliverable => "预期交付物",
        Key::ReviewSuccessEvidence => "成功证据",
        Key::ReviewAssumptions => "推断的假设",
        Key::ReviewDecisions => "未解决或相互矛盾的决策",
        Key::ReviewFindings => "发现",
        Key::ReviewNoFindings => "没有可定位的发现。",
        Key::ReviewQuestions => "澄清问题",
        Key::ReviewNoQuestions => "没有会影响结果的澄清问题。",
        Key::ReviewPriority => "优先级",
        Key::ReviewPriorityCritical => "关键",
        Key::ReviewPriorityHigh => "高",
        Key::ReviewPriorityMedium => "中",
        Key::ReviewPriorityLow => "低",
        Key::ReviewSourceStatement => "源文本陈述",
        Key::ReviewInference => "推断",
        Key::ReviewDocumentWide => "整个文档",
        Key::ReviewAnchor => "源锚点",
        Key::ReviewRun => "运行审查",
        Key::ReviewDismiss => "关闭审查",

        Key::RevisionAnswers => "修订答案",
        Key::RevisionRecoveredAnswers => "已恢复的修订答案",
        Key::RevisionCopyRecoveredAnswers => "复制已恢复答案",
        Key::RevisionDiscardRecoveredAnswers => "丢弃已恢复答案",
        Key::RevisionRecoveredAnswersCopied => "已复制恢复的修订答案",
        Key::RevisionAnswerUnanswered => "未回答",
        Key::RevisionAnswerIntentionallyUnspecified => "有意不指定",
        Key::RevisionAnswerAnswered => "已回答",
        Key::RevisionRun => "运行修订",
        Key::RevisionDiscardAnswers => "丢弃修订答案",
        Key::RevisionRetry => "重试",
        Key::RevisionDismiss => "关闭",
        Key::RevisionConsentTitle => "修订授权",
        Key::RevisionWaitingForConsent => "修订正在等待授权",
        Key::RevisionRunning => "正在运行修订",
        Key::RevisionPreparationFailed => "修订准备失败",
        Key::RevisionAuthorizationFailed => "修订授权失败",
        Key::RevisionProviderFailed => "修订服务失败",
        Key::RevisionApplyFailed => "修订无法应用",
        Key::RevisionSourceChanged => "审查的源文本已更改。请求修订前请重新运行 Review。",
        Key::RevisionScopeChangedDuringConsent => {
            "授权对话期间审查的源文本或 Agent Skill 包已更改。"
        }
        Key::RevisionDocumentClosed => "修订完成前文档已关闭。",
        Key::RevisionConsentCancelledAnswersRetained => "修订授权已取消；答案已保留。",
        Key::RevisionCancelledAnswersRetained => "修订已取消；答案已保留。",
        Key::RevisionAnswersRetained => "修订答案已保留。",
        Key::RevisionAnswersDiscarded => "已丢弃修订答案",
        Key::RevisionRecoveryRetirementFailed => "无法清除恢复检查点，已保留恢复的修订答案。",
        Key::RevisionStale => "修订已过期，无法应用。请先重新运行 Review。",
        Key::RevisionStaleInspection => "修订结果已过期，只能查看，不能应用",
        Key::RevisionReady => "修订已准备好查看",
        Key::RevisionReadyStale => "修订已准备好查看，但已过期",
        Key::RevisionApplied => "已应用批准的修订",
        Key::RevisionAppliedSavePending => "已应用批准的修订；等待保存",
        Key::RevisionNoApprovedChanges => "没有可应用的批准变更",
        Key::RevisionCopied => "已复制批准的修订预览",
        Key::RevisionResult => "修订结果",
        Key::RevisionChange => "修订变更",
        Key::RevisionChangeAccepted => "已接受",
        Key::RevisionChangeRejected => "已拒绝",
        Key::RevisionRationale => "理由",
        Key::RevisionAccept => "接受",
        Key::RevisionReject => "拒绝",
        Key::RevisionAcceptAll => "全部接受",
        Key::RevisionRejectAll => "全部拒绝",
        Key::RevisionCopy => "复制",
        Key::RevisionApply => "应用",
        Key::RevisionFinalPreview => "最终预览",
        Key::RevisionFinalPreviewSource => "最终批准的修订源文本",
        Key::RevisionCoverage => "问题覆盖",
        Key::RevisionCoverageValidated => "个已验证的问题覆盖条目",
        Key::RevisionCoverageQuestion => "问题",
        Key::RevisionCoverageRepresented => "由以下变更表示",
        Key::RevisionCoverageIntentionallyOmitted => "有意省略",
        Key::RevisionCoverageNotAddressed => "未处理",
        Key::SendToModel => "发送",
        Key::ModelRequestConsentTitle => "将内容发送给模型？",
        Key::ModelRequestWaiting => "正在等待模型请求授权…",
        Key::ModelRequestCancelled => "已取消模型请求",
        Key::ModelRequestDocumentClosed => "文档已关闭，模型请求已取消",
        Key::ModelRequestDocumentChanged => "等待授权期间文档已更改。请检查更新后的范围并重试。",
        Key::Settings => "设置",
        Key::Save => "保存",
        Key::SaveAsPicker => "另存为\u{2026}",
        Key::SaveAsSnapshotChanged => "另存为对话框打开期间文档已更改，待处理的关闭操作已取消。",
        Key::Replace => "替换",
        Key::Cancel => "取消",

        Key::PanelFiles => "文件",
        Key::PanelSearch => "搜索",
        Key::PanelHarness => "代理上下文",
        Key::PanelOutline => "大纲",
        Key::SectionSkills => "Skills",
        Key::SectionInstructions => "指令文件",
        Key::GroupBy => "分组方式",
        Key::Rescan => "重新扫描",
        Key::ViewLayout => "视图",
        Key::Details => "详情",
        Key::ToggleLeftPanel => "显示/隐藏侧边栏",
        Key::ToggleRightPanel => "显示/隐藏详情栏",
        Key::SidePanelWidth => "侧边栏宽度",
        Key::DetailsPanelWidth => "详情栏宽度",
        Key::CopyPath => "复制路径",
        Key::CopyRelativePath => "复制相对路径",

        Key::ScopeDocument => "当前文件",
        Key::ScopeOpenTabs => "已打开标签",
        Key::ScopeFolder => "文件夹",
        Key::ScopeHarness => "Harness",
        Key::Searching => "搜索中…",
        Key::TypeToSearch => "输入以搜索。",
        Key::NoMatches => "没有匹配项。",

        Key::Untitled => "未命名",
        Key::RecoveredSnapshot => "已恢复快照",
        Key::UnsavedChanges => "有未保存的更改",
        Key::NavigateBack => "后退",
        Key::NavigateForward => "前进",

        Key::OpenFolderToBegin => "打开一个文件夹以开始。",
        Key::OpenFolderToDiscover => "打开一个文件夹以发现 skills 和指令文件。",
        Key::OpenDocumentForOutline => "打开一个文档以查看其大纲。",
        Key::NoHeadings => "此文档没有标题。",
        Key::OpenAMarkdownFile => "打开一个 Markdown 文件以开始。",
        Key::Scanning => "扫描中…",
        Key::WelcomeTitle => "开始编写 Markdown 文档",
        Key::WelcomeSubtitle => "新建文档，或从电脑中打开已有文档。",
        Key::Recent => "最近打开",
        Key::RecentMissing => "文件不存在",
        Key::RecentUnavailable => "不可用",
        Key::DontShowWelcomeAgain => "不再显示",

        Key::ModeSource => "编辑",
        Key::ModeNative => "预览",
        Key::ModeWeb => "兼容预览",
        Key::ModeSplitNative => "分栏 · 预览",
        Key::ModeSplitWeb => "分栏 · 兼容预览",
        Key::WebPreviewFailed => "无法打开 Web 预览。",
        Key::WebPreviewRetry => "重试 Web 预览",
        Key::TrustThisDocument => "信任此文档",
        Key::Trusted => "已信任 ✓",
        Key::HtmlNeedsTrust => {
            "此 HTML 文件在沙箱中显示，它从磁盘加载的图片和样式表被阻止。信任此文档以加载它们。"
        }
        Key::FileChangedOnDisk => "此文件自打开后已在磁盘上被修改。",
        Key::ReloadFromDisk => "从磁盘重新加载",
        Key::Overwrite => "覆盖",

        Key::Watching => "监视中",
        Key::AutoRefresh => "外部修改时自动刷新",
        Key::AutoRefreshOn => "自动刷新已开启",

        Key::Origin => "来源",
        Key::Location => "位置",
        Key::DiscoveredIn => "发现于",
        Key::AlsoLinkedFrom => "同时链接自",
        Key::Files => "文件",
        Key::Validation => "校验",
        Key::Kind => "类型",
        Key::Status => "状态",
        Key::ChangedOnDisk => "文件已在磁盘上更改",
        Key::Saved => "已保存",
        Key::Open => "打开",
        Key::OpenSkillMd => "打开 SKILL.md",

        Key::Appearance => "外观",
        Key::Theme => "主题",
        Key::Mode => "模式",
        Key::LightTheme => "浅色主题",
        Key::DarkTheme => "深色主题",
        Key::Language_ => "界面语言",
        Key::Translation => "翻译",
        Key::ModelConfiguration => "模型配置",
        Key::Provider => "服务商",
        Key::Model => "模型",
        Key::BaseUrl => "接口地址",
        Key::EndpointStatus => "端点状态",
        Key::TargetLanguage => "目标语言",
        Key::Skills => "Skills",
        Key::Discovery => "发现",
        Key::IncludeGlobalSkills => "包含全局 skills",
        Key::ShowInternalSkills => "显示内部 skills",
        Key::SyncScrolling => "分栏时同步滚动",
        Key::ApiKey => "API key",
        Key::Credentials => "凭据",
        Key::Credential => "凭据",
        Key::EnvironmentCredential => "环境变量凭据",
        Key::LegacyCredential => "旧版明文凭据",
        Key::Editor => "编辑器",
        Key::SplitView => "分栏视图",
        Key::ProviderBestAvailable => "自动选择",

        Key::StoreCredentialSecurely => "安全存储",
        Key::UseCredentialForSession => "仅本次会话使用",
        Key::TestCredential => "测试",
        Key::DeleteCredential => "删除",
        Key::MigrateLegacyCredential => "安全迁移",
        Key::DeleteCredentialTitle => "删除此凭据？",
        Key::MigrateLegacyCredentialTitle => "迁移明文凭据？",
        Key::ChooseProviderForCredential => "请先选择服务商，再配置凭据。",
        Key::UnsupportedProvider => "当前配置的服务商不受支持。",
        Key::CredentialEndpointInvalid => "请先修正端点，再配置凭据。",
        Key::SecureStoreUnavailable => "安全存储不可用。请使用会话凭据或环境变量凭据。",
        Key::CredentialStoredPresent => "此端点已安全存储凭据。",
        Key::CredentialStoredAbsent => {
            "此端点没有安全存储的凭据，但会话凭据或环境变量凭据仍可能有效。"
        }
        Key::CredentialStoredSecurely => "凭据已安全存储。",
        Key::CredentialSessionActive => "凭据仅在本次会话中有效。",
        Key::CredentialTestRequested => "已请求测试凭据。",
        Key::CredentialDeleted => "已删除此端点的凭据。",
        Key::LegacyCredentialMigrated => "明文凭据已安全迁移。",
        Key::CredentialStoring => "正在安全存储凭据…",
        Key::CredentialDeleting => "正在删除凭据…",
        Key::CredentialMigrating => "正在迁移凭据…",
        Key::CredentialReadFailed => "无法读取安全存储的凭据。",
        Key::CredentialWriteFailed => "无法写入安全凭据。",
        Key::CredentialVerifyFailed => "无法验证安全凭据写入。",
        Key::CredentialDeleteFailed => "无法删除安全存储的凭据。",
        Key::CredentialInvalidEncoding => "安全存储的凭据不是有效 UTF-8。",
        Key::CredentialRequired => "请先输入凭据。",
        Key::CredentialMigrationSaveFailed => "凭据已安全存储，但无法删除设置中的明文值。",
        Key::EnvironmentCredentialAuthorizationSaveFailed => {
            "无法保存环境变量凭据授权。之前的授权状态保持不变。"
        }
        Key::SettingsPathUnavailable => "设置文件路径不可用。",
        Key::SaveEndpointBeforeCredential => "请先保存端点，再管理其凭据。",

        Key::EndpointInvalidUrl => "端点 URL 无效。",
        Key::EndpointUnsupportedScheme => "端点必须使用 HTTP 或 HTTPS。",
        Key::EndpointMissingHost => "端点 URL 缺少主机。",
        Key::EndpointUserInfoNotAllowed => "端点 URL 不能包含用户信息。",
        Key::EndpointQueryNotAllowed => "端点 URL 不能包含查询参数。",
        Key::EndpointFragmentNotAllowed => "端点 URL 不能包含片段。",
        Key::EndpointInsecureRemoteTransport => "远程端点必须使用 HTTPS。",

        Key::ModeHelp => "“系统”跟随操作系统，并在应用运行期间持续跟随。",
        Key::LightThemeHelp => "当实际模式为浅色时使用。",
        Key::DarkThemeHelp => "当实际模式为深色时使用。",
        Key::LanguageHelp => "界面所用的语言。与下方的翻译目标语言无关，后者针对的是文档。",
        Key::ProviderHelp => {
            "要使用的协议格式，而非服务商。请选择明确的服务商以管理其凭据；操作开始时，\
             “自动选择”会选择已配置的服务商。服务商默认端点上，Anthropic 使用 \
             ANTHROPIC_API_KEY，两种 OpenAI 格式使用 OPENAI_API_KEY。"
        }
        Key::ApiKeyHelp => {
            "输入替换用凭据。在选择安全存储或仅本次会话使用前，它只保留在此掩码输入框中；\
             已存储的值永不显示。"
        }
        Key::CredentialHelp => {
            "输入替换用凭据。在选择安全存储或仅本次会话使用前，它只保留在此掩码输入框中；\
             已存储的值永不显示。"
        }
        Key::LegacyCredentialHelp => {
            "旧版设置文件仍含有明文凭据。迁移会先写入并验证安全存储，再删除明文值。"
        }
        Key::BaseUrlHelp => {
            "留空则使用服务商默认端点。自定义远程端点必须使用 HTTPS；HTTP 仅允许已验证的回环地址，\
             且不使用代理。需包含 API 基础路径，例如 `http://127.0.0.1:11434/v1`。"
        }
        Key::ModelHelp => {
            "留空时依次使用 MARKTURBO_MODEL、MARKTURBO_TRANSLATE_MODEL 和服务商默认模型。"
        }
        Key::TargetLanguageHelp => "语言名称或代码，例如 `zh`、`ja`、`German`。",
        Key::SyncScrollingHelp => {
            "让预览跟随编辑器滚动，也跟随大纲点击。映射按比例进行，因此含有大幅图表的文档滚动幅度会超出预期。"
        }
        Key::AutoRefreshHelp => {
            "当文件在磁盘上发生变化时重新读取文档。有未保存修改的标签页永不刷新 —— 它会保留“重新加载/覆盖”\
             提示条，因为自动刷新绝不能丢弃已输入的文本。"
        }
        Key::IncludeGlobalSkillsHelp => {
            "除本工作区外，同时搜索各 harness 的全局目录（~/.claude/skills、~/.agents/skills 等）。"
        }
        Key::ShowInternalSkillsHelp => {
            "标记了 `metadata.internal: true` 的 skills，参考工具默认将其隐藏。"
        }
        Key::GroupByHelp => "Skills 列表的组织方式。",
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
        Key::OpenFolder,
        Key::OpenFile,
        Key::OpenFolderPicker,
        Key::OpenFilePicker,
        Key::NewDocument,
        Key::Paste,
        Key::ClipboardTextUnavailable,
        Key::OpenBundledSample,
        Key::BundledSampleUnavailable,
        Key::Translate,
        Key::Review,
        Key::ReviewDocument,
        Key::ReviewSelection,
        Key::ReviewEmptySelection,
        Key::ReviewLens,
        Key::ReviewLensPrompt,
        Key::ReviewLensSpecification,
        Key::ReviewLensAgentInstructions,
        Key::ReviewLensAgentSkill,
        Key::ReviewLensCorrection,
        Key::ReviewReadOnly,
        Key::ReviewSelectionContextOmitted,
        Key::ReviewWaiting,
        Key::ReviewCancelled,
        Key::ReviewDocumentClosed,
        Key::ReviewDocumentChanged,
        Key::ReviewStale,
        Key::ReviewMalformed,
        Key::ReviewOversized,
        Key::ReviewSkillPackageUnavailable,
        Key::ReviewSkillPackageEntrypointMissing,
        Key::ReviewSkillPackageSelectionUnsupported,
        Key::ReviewSkillPackageReadFailed,
        Key::ReviewSkillPackageChanged,
        Key::ReviewSkillPackageNonUtf8Path,
        Key::ReviewSkillPackageOversized,
        Key::ReviewNoProvider,
        Key::ReviewMissingCredential,
        Key::ReviewUnavailable,
        Key::ReviewTimeout,
        Key::ReviewFailed,
        Key::ReviewReady,
        Key::ReviewPartial,
        Key::ReviewNoResult,
        Key::ReviewWhatIUnderstand,
        Key::ReviewRequestedModel,
        Key::ReviewResponseModel,
        Key::ReviewStatedGoal,
        Key::ReviewContext,
        Key::ReviewConstraints,
        Key::ReviewNonGoals,
        Key::ReviewDeliverable,
        Key::ReviewSuccessEvidence,
        Key::ReviewAssumptions,
        Key::ReviewDecisions,
        Key::ReviewFindings,
        Key::ReviewNoFindings,
        Key::ReviewQuestions,
        Key::ReviewNoQuestions,
        Key::ReviewPriority,
        Key::ReviewPriorityCritical,
        Key::ReviewPriorityHigh,
        Key::ReviewPriorityMedium,
        Key::ReviewPriorityLow,
        Key::ReviewSourceStatement,
        Key::ReviewInference,
        Key::ReviewDocumentWide,
        Key::ReviewAnchor,
        Key::ReviewRun,
        Key::ReviewDismiss,
        Key::RevisionAnswers,
        Key::RevisionRecoveredAnswers,
        Key::RevisionCopyRecoveredAnswers,
        Key::RevisionDiscardRecoveredAnswers,
        Key::RevisionRecoveredAnswersCopied,
        Key::RevisionAnswerUnanswered,
        Key::RevisionAnswerIntentionallyUnspecified,
        Key::RevisionAnswerAnswered,
        Key::RevisionRun,
        Key::RevisionDiscardAnswers,
        Key::RevisionRetry,
        Key::RevisionDismiss,
        Key::RevisionConsentTitle,
        Key::RevisionWaitingForConsent,
        Key::RevisionRunning,
        Key::RevisionPreparationFailed,
        Key::RevisionAuthorizationFailed,
        Key::RevisionProviderFailed,
        Key::RevisionApplyFailed,
        Key::RevisionSourceChanged,
        Key::RevisionScopeChangedDuringConsent,
        Key::RevisionDocumentClosed,
        Key::RevisionConsentCancelledAnswersRetained,
        Key::RevisionCancelledAnswersRetained,
        Key::RevisionAnswersRetained,
        Key::RevisionAnswersDiscarded,
        Key::RevisionRecoveryRetirementFailed,
        Key::RevisionStale,
        Key::RevisionStaleInspection,
        Key::RevisionReady,
        Key::RevisionReadyStale,
        Key::RevisionApplied,
        Key::RevisionAppliedSavePending,
        Key::RevisionNoApprovedChanges,
        Key::RevisionCopied,
        Key::RevisionResult,
        Key::RevisionChange,
        Key::RevisionChangeAccepted,
        Key::RevisionChangeRejected,
        Key::RevisionRationale,
        Key::RevisionAccept,
        Key::RevisionReject,
        Key::RevisionAcceptAll,
        Key::RevisionRejectAll,
        Key::RevisionCopy,
        Key::RevisionApply,
        Key::RevisionFinalPreview,
        Key::RevisionFinalPreviewSource,
        Key::RevisionCoverage,
        Key::RevisionCoverageValidated,
        Key::RevisionCoverageQuestion,
        Key::RevisionCoverageRepresented,
        Key::RevisionCoverageIntentionallyOmitted,
        Key::RevisionCoverageNotAddressed,
        Key::SendToModel,
        Key::ModelRequestConsentTitle,
        Key::ModelRequestWaiting,
        Key::ModelRequestCancelled,
        Key::ModelRequestDocumentClosed,
        Key::ModelRequestDocumentChanged,
        Key::Settings,
        Key::Save,
        Key::SaveAsPicker,
        Key::SaveAsSnapshotChanged,
        Key::Replace,
        Key::Cancel,
        Key::PanelFiles,
        Key::PanelSearch,
        Key::PanelHarness,
        Key::PanelOutline,
        Key::SectionSkills,
        Key::SectionInstructions,
        Key::GroupBy,
        Key::Rescan,
        Key::ViewLayout,
        Key::Details,
        Key::ToggleLeftPanel,
        Key::ToggleRightPanel,
        Key::SidePanelWidth,
        Key::DetailsPanelWidth,
        Key::CopyPath,
        Key::CopyRelativePath,
        Key::ScopeDocument,
        Key::ScopeOpenTabs,
        Key::ScopeFolder,
        Key::ScopeHarness,
        Key::Searching,
        Key::TypeToSearch,
        Key::NoMatches,
        Key::Untitled,
        Key::RecoveredSnapshot,
        Key::UnsavedChanges,
        Key::NavigateBack,
        Key::NavigateForward,
        Key::OpenFolderToBegin,
        Key::OpenFolderToDiscover,
        Key::OpenDocumentForOutline,
        Key::NoHeadings,
        Key::OpenAMarkdownFile,
        Key::Scanning,
        Key::WelcomeTitle,
        Key::WelcomeSubtitle,
        Key::Recent,
        Key::RecentMissing,
        Key::RecentUnavailable,
        Key::DontShowWelcomeAgain,
        Key::ModeSource,
        Key::ModeNative,
        Key::ModeWeb,
        Key::ModeSplitNative,
        Key::ModeSplitWeb,
        Key::WebPreviewFailed,
        Key::WebPreviewRetry,
        Key::TrustThisDocument,
        Key::Trusted,
        Key::HtmlNeedsTrust,
        Key::FileChangedOnDisk,
        Key::ReloadFromDisk,
        Key::Overwrite,
        Key::Watching,
        Key::AutoRefresh,
        Key::AutoRefreshOn,
        Key::Origin,
        Key::Location,
        Key::DiscoveredIn,
        Key::AlsoLinkedFrom,
        Key::Files,
        Key::Validation,
        Key::Kind,
        Key::Status,
        Key::ChangedOnDisk,
        Key::Saved,
        Key::Open,
        Key::OpenSkillMd,
        Key::Appearance,
        Key::Theme,
        Key::Mode,
        Key::LightTheme,
        Key::DarkTheme,
        Key::Language_,
        Key::Translation,
        Key::ModelConfiguration,
        Key::Provider,
        Key::Model,
        Key::BaseUrl,
        Key::EndpointStatus,
        Key::TargetLanguage,
        Key::Skills,
        Key::Discovery,
        Key::IncludeGlobalSkills,
        Key::ShowInternalSkills,
        Key::SyncScrolling,
        Key::ApiKey,
        Key::Credentials,
        Key::Credential,
        Key::EnvironmentCredential,
        Key::LegacyCredential,
        Key::Editor,
        Key::SplitView,
        Key::ProviderBestAvailable,
        Key::StoreCredentialSecurely,
        Key::UseCredentialForSession,
        Key::TestCredential,
        Key::DeleteCredential,
        Key::MigrateLegacyCredential,
        Key::DeleteCredentialTitle,
        Key::MigrateLegacyCredentialTitle,
        Key::ChooseProviderForCredential,
        Key::UnsupportedProvider,
        Key::CredentialEndpointInvalid,
        Key::SecureStoreUnavailable,
        Key::CredentialStoredPresent,
        Key::CredentialStoredAbsent,
        Key::CredentialStoredSecurely,
        Key::CredentialSessionActive,
        Key::CredentialTestRequested,
        Key::CredentialDeleted,
        Key::LegacyCredentialMigrated,
        Key::CredentialStoring,
        Key::CredentialDeleting,
        Key::CredentialMigrating,
        Key::CredentialReadFailed,
        Key::CredentialWriteFailed,
        Key::CredentialVerifyFailed,
        Key::CredentialDeleteFailed,
        Key::CredentialInvalidEncoding,
        Key::CredentialRequired,
        Key::CredentialMigrationSaveFailed,
        Key::EnvironmentCredentialAuthorizationSaveFailed,
        Key::SettingsPathUnavailable,
        Key::SaveEndpointBeforeCredential,
        Key::EndpointInvalidUrl,
        Key::EndpointUnsupportedScheme,
        Key::EndpointMissingHost,
        Key::EndpointUserInfoNotAllowed,
        Key::EndpointQueryNotAllowed,
        Key::EndpointFragmentNotAllowed,
        Key::EndpointInsecureRemoteTransport,
        Key::ModeHelp,
        Key::LightThemeHelp,
        Key::DarkThemeHelp,
        Key::LanguageHelp,
        Key::ProviderHelp,
        Key::ApiKeyHelp,
        Key::CredentialHelp,
        Key::LegacyCredentialHelp,
        Key::BaseUrlHelp,
        Key::ModelHelp,
        Key::TargetLanguageHelp,
        Key::SyncScrollingHelp,
        Key::AutoRefreshHelp,
        Key::IncludeGlobalSkillsHelp,
        Key::ShowInternalSkillsHelp,
        Key::GroupByHelp,
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
    fn the_html_sandbox_banner_names_the_way_out() {
        // A banner that says only "blocked" leaves the user with a broken page
        // and no next step; both languages must point at Trust.
        assert!(t(Language::English, Key::HtmlNeedsTrust).contains("Trust"));
        assert!(t(Language::Chinese, Key::HtmlNeedsTrust).contains("信任"));
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
        for key in [
            Key::OpenFilePicker,
            Key::OpenFolderPicker,
            Key::SaveAsPicker,
        ] {
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
    fn filenames_and_technical_terms_stay_untranslated() {
        // `SKILL.md` is a filename: translating it would name a file that does
        // not exist. Same for the harness vocabulary the ecosystem uses.
        assert!(t(Language::Chinese, Key::OpenSkillMd).contains("SKILL.md"));
        assert_eq!(t(Language::Chinese, Key::PanelHarness), "代理上下文");
        assert!(t(Language::Chinese, Key::OpenFolderToDiscover).contains("skills"));
    }

    #[test]
    fn credential_settings_strings_are_translated_and_content_free() {
        for language in Language::ALL {
            for key in [
                Key::ModelConfiguration,
                Key::EndpointStatus,
                Key::Credential,
                Key::StoreCredentialSecurely,
                Key::UseCredentialForSession,
                Key::TestCredential,
                Key::DeleteCredential,
                Key::MigrateLegacyCredential,
                Key::CredentialStoredSecurely,
                Key::CredentialSessionActive,
                Key::CredentialDeleted,
                Key::SecureStoreUnavailable,
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
        assert_ne!(
            t(Language::English, Key::DeleteCredentialTitle),
            t(Language::Chinese, Key::DeleteCredentialTitle)
        );
    }
}
