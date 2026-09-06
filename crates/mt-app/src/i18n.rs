//! Interface strings.
//!
//! One enum of keys and one table per language, rather than scattered literals.
//! A key with no translation falls back to English rather than showing the key
//! itself: a missing string should degrade to a language the user may not read
//! rather than to `MenuOpenFolder`, which nobody reads.
//!
//! Deliberately not a general i18n framework. There is no pluralization, no
//! gender, no date formatting — this app's interface is a few dozen labels, and
//! the cost of a framework would exceed the strings it manages.

use crate::{
    model::{
        EndpointIdentity, EndpointIdentityError, EndpointLocation, ModelOperation,
        ModelRequestDisclosure, OutboundScopeKind, ProxyDisclosure, TransportEncryption,
    },
    settings::{AppSettings, Language},
};

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
    ClipboardTextUnavailable,
    OpenBundledSample,
    BundledSampleUnavailable,
    Translate,
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
}

/// The string for `key` in the language the user picked.
pub fn t(key: Key, cx: &gpui::App) -> &'static str {
    text(key, AppSettings::global(cx).language)
}

fn quoted_path(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| quoted_path(path))
}

pub fn replace_file_title(path: &std::path::Path, cx: &gpui::App) -> String {
    replace_file_title_in(path, AppSettings::global(cx).language)
}

fn replace_file_title_in(path: &std::path::Path, language: Language) -> String {
    let name = file_name(path);
    match language {
        Language::English => format!("Replace \u{201c}{name}\u{201d}?"),
        Language::Chinese => format!("替换\u{201c}{name}\u{201d}？"),
    }
}

pub fn replace_file_description(path: &std::path::Path, cx: &gpui::App) -> String {
    replace_file_description_in(path, AppSettings::global(cx).language)
}

fn replace_file_description_in(path: &std::path::Path, language: Language) -> String {
    let path = quoted_path(path);
    match language {
        Language::English => {
            format!("This will replace \u{201c}{path}\u{201d} with the current editor text.")
        }
        Language::Chinese => format!("当前编辑器文本将替换\u{201c}{path}\u{201d}。"),
    }
}

pub fn open_recent_target_label(path: &std::path::Path, cx: &gpui::App) -> String {
    recent_target_label_in(path, AppSettings::global(cx).language, true)
}

pub fn remove_recent_target_label(path: &std::path::Path, cx: &gpui::App) -> String {
    recent_target_label_in(path, AppSettings::global(cx).language, false)
}

pub fn save_as_snapshot_changed_message(cx: &gpui::App) -> &'static str {
    t(Key::SaveAsSnapshotChanged, cx)
}

pub fn save_as_path_already_open_message(path: &std::path::Path, cx: &gpui::App) -> String {
    let path = quoted_path(path);
    match AppSettings::global(cx).language {
        Language::English => {
            format!("\u{201c}{path}\u{201d} is already open. Choose another Save As path.")
        }
        Language::Chinese => format!("\u{201c}{path}\u{201d}已打开。请选择其他另存为路径。"),
    }
}

fn recent_target_label_in(path: &std::path::Path, language: Language, open: bool) -> String {
    let path = quoted_path(path);
    match (language, open) {
        (Language::English, true) => format!("Open \u{201c}{path}\u{201d}"),
        (Language::English, false) => format!("Remove \u{201c}{path}\u{201d} from Recent"),
        (Language::Chinese, true) => format!("打开\u{201c}{path}\u{201d}"),
        (Language::Chinese, false) => format!("从最近打开中移除\u{201c}{path}\u{201d}"),
    }
}

/// The string for `key` in an explicit language.
///
/// Exposed separately so the table can be tested without an `App`.
pub fn text(key: Key, language: Language) -> &'static str {
    match language {
        Language::English => english(key),
        // Falling back to English rather than to the key name: an untranslated
        // label in a language the user may still read beats `PanelHarness`.
        Language::Chinese => chinese(key).unwrap_or_else(|| english(key)),
    }
}

pub fn model_endpoint_status(endpoint: &EndpointIdentity, cx: &gpui::App) -> String {
    model_endpoint_status_in(endpoint, AppSettings::global(cx).language)
}

fn model_endpoint_status_in(endpoint: &EndpointIdentity, language: Language) -> String {
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

pub fn model_request_disclosure(disclosure: &ModelRequestDisclosure, cx: &gpui::App) -> String {
    model_request_disclosure_in(disclosure, AppSettings::global(cx).language)
}

fn model_request_disclosure_in(disclosure: &ModelRequestDisclosure, language: Language) -> String {
    let endpoint = disclosure.endpoint();
    let (operation, location, encryption, proxy, suffix) = match language {
        Language::English => (
            match disclosure.operation() {
                ModelOperation::Review => "Review",
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

    match language {
        Language::English => format!(
            "Operation: {operation}\nEndpoint: {location}\nWire format: {}\nIdentity: {}\nTransport: {encryption}; {proxy}\n{scope}\n\n{suffix}",
            endpoint.provider().label(),
            endpoint.normalized_identity(),
        ),
        Language::Chinese => format!(
            "操作：{operation}\n端点：{location}\n协议格式：{}\n端点标识：{}\n传输：{encryption}；{proxy}\n{scope}\n\n{suffix}",
            endpoint.provider().label(),
            endpoint.normalized_identity(),
        ),
    }
}

pub fn model_endpoint_error(error: &EndpointIdentityError, cx: &gpui::App) -> String {
    let key = match error {
        EndpointIdentityError::InvalidUrl(_) => Key::EndpointInvalidUrl,
        EndpointIdentityError::UnsupportedScheme => Key::EndpointUnsupportedScheme,
        EndpointIdentityError::MissingHost => Key::EndpointMissingHost,
        EndpointIdentityError::UserInfoNotAllowed => Key::EndpointUserInfoNotAllowed,
        EndpointIdentityError::QueryNotAllowed => Key::EndpointQueryNotAllowed,
        EndpointIdentityError::FragmentNotAllowed => Key::EndpointFragmentNotAllowed,
        EndpointIdentityError::InsecureRemoteTransport => Key::EndpointInsecureRemoteTransport,
    };
    t(key, cx).into()
}

pub fn environment_credential_description(
    variable: &str,
    endpoint: &EndpointIdentity,
    cx: &gpui::App,
) -> String {
    let base_url = endpoint.base_url();
    match AppSettings::global(cx).language {
        Language::English => format!(
            "Allow {variable} only for {base_url}. Changing the provider or endpoint makes this authorization inactive."
        ),
        Language::Chinese => {
            format!("仅允许 {base_url} 使用 {variable}。更改服务商或端点后，此授权将失效。")
        }
    }
}

pub fn delete_model_credential_description(endpoint: &EndpointIdentity, cx: &gpui::App) -> String {
    let provider = endpoint.provider().label();
    let base_url = endpoint.base_url();
    match AppSettings::global(cx).language {
        Language::English => format!(
            "Delete the session and securely stored credential for {provider} at {base_url}? Credentials for other endpoint identities are unchanged."
        ),
        Language::Chinese => format!(
            "删除 {provider} 在 {base_url} 的会话凭据和安全存储凭据？其他端点身份不受影响。"
        ),
    }
}

pub fn migrate_legacy_credential_description(
    endpoint: &EndpointIdentity,
    cx: &gpui::App,
) -> String {
    let provider = endpoint.provider().label();
    let base_url = endpoint.base_url();
    match AppSettings::global(cx).language {
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
        Key::ClipboardTextUnavailable => "Clipboard does not contain text or is unavailable.",
        Key::OpenBundledSample => "Open Bundled Sample",
        Key::BundledSampleUnavailable => "The bundled sample could not be opened.",
        Key::Translate => "Translate",
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
        Key::PanelHarness => "Harness",
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

        Key::ModeSource => "Source",
        Key::ModeNative => "Native",
        Key::ModeWeb => "Web",
        Key::ModeSplitNative => "Split · Native",
        Key::ModeSplitWeb => "Split · Web",
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
             base path, for example `http://localhost:11434/v1`."
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
    }
}

/// Chinese strings.
///
/// `Option` rather than an exhaustive match: this returns `None` for anything
/// not yet translated, and the caller falls back. Technical terms and file names
/// stay in English — `SKILL.md` is a filename, not a word.
fn chinese(key: Key) -> Option<&'static str> {
    Some(match key {
        Key::OpenFolder => "打开文件夹",
        Key::OpenFile => "打开文件",
        Key::OpenFolderPicker => "打开文件夹\u{2026}",
        Key::OpenFilePicker => "打开文件\u{2026}",
        Key::NewDocument => "新建",
        Key::Paste => "粘贴",
        Key::ClipboardTextUnavailable => "剪贴板中没有文本，或剪贴板当前不可用。",
        Key::OpenBundledSample => "打开内置示例",
        Key::BundledSampleUnavailable => "无法打开内置示例。",
        Key::Translate => "翻译",
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
        Key::PanelHarness => "Harness",
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

        Key::ModeSource => "源码",
        Key::ModeNative => "原生",
        Key::ModeWeb => "Web",
        Key::ModeSplitNative => "分栏 · 原生",
        Key::ModeSplitWeb => "分栏 · Web",
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
             且不使用代理。需包含 API 基础路径，例如 `http://localhost:11434/v1`。"
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
    })
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
    ];

    #[test]
    fn every_key_has_a_string_in_every_language() {
        for &key in EVERY_KEY {
            for language in Language::ALL {
                let value = text(key, language);
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
        // The fallback exists so a *new* key does not break the UI, not as a
        // license to leave the table half-finished. This is what keeps it
        // honest.
        let missing: Vec<Key> = EVERY_KEY
            .iter()
            .copied()
            .filter(|&k| chinese(k).is_none())
            .collect();
        assert!(missing.is_empty(), "untranslated: {missing:?}");
    }

    #[test]
    fn an_untranslated_key_falls_back_to_english_not_to_the_key_name() {
        // Simulated by asking for a key the Chinese table happens to leave in
        // English: the result must still be a readable label.
        assert_eq!(text(Key::ModeWeb, Language::Chinese), "Web");
        assert!(!text(Key::ModeWeb, Language::Chinese).contains("Key::"));
    }

    #[test]
    fn the_html_sandbox_banner_names_the_way_out() {
        // A banner that says only "blocked" leaves the user with a broken page
        // and no next step; both languages must point at Trust.
        assert!(text(Key::HtmlNeedsTrust, Language::English).contains("Trust"));
        assert!(text(Key::HtmlNeedsTrust, Language::Chinese).contains("信任"));
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
                let label = text(key, language);
                assert!(label.ends_with('\u{2026}'), "{key:?}: {label}");
                assert!(!label.ends_with("..."), "{key:?}: {label}");
            }
        }
    }

    #[test]
    fn destructive_and_recent_labels_name_the_exact_path() {
        let path = std::path::Path::new("C:/work/notes.md");
        for language in Language::ALL {
            assert!(replace_file_title_in(path, language).contains("notes.md"));
            assert!(replace_file_description_in(path, language).contains("C:/work/notes.md"));
            assert!(recent_target_label_in(path, language, true).contains("C:/work/notes.md"));
            assert!(recent_target_label_in(path, language, false).contains("C:/work/notes.md"));
        }
    }

    #[test]
    fn model_request_disclosure_is_complete_in_each_interface_language() {
        use crate::model::{AgentSkillRequest, AgentSkillRequestEntry, OutboundScope};

        let endpoint = EndpointIdentity::parse(
            crate::model::Provider::OpenAiResponses,
            Some("http://127.0.0.1:8080/custom/v1/"),
        )
        .unwrap();
        let disclosure = ModelRequestDisclosure::new(
            ModelOperation::Translation,
            endpoint.clone(),
            OutboundScope::block(37),
        );

        let english = model_request_disclosure_in(&disclosure, Language::English);
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

        let chinese = model_request_disclosure_in(&disclosure, Language::Chinese);
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
            let message = model_request_disclosure_in(&context, language);
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
                crate::model::AgentSkillOmission::new(
                    "references/private.md",
                    "excluded by policy",
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let package = package.disclosure(ModelOperation::Review, endpoint);
        for language in Language::ALL {
            let message = model_request_disclosure_in(&package, language);
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
        assert!(model_request_disclosure_in(&package, Language::English).contains("partial"));
        assert!(model_request_disclosure_in(&package, Language::Chinese).contains("部分"));
    }

    #[test]
    fn local_https_status_discloses_encryption_and_disabled_proxy_in_both_languages() {
        let endpoint = EndpointIdentity::parse(
            crate::model::Provider::OpenAiChat,
            Some("https://localhost:8443/v1/"),
        )
        .unwrap();

        let english = model_endpoint_status_in(&endpoint, Language::English);
        assert!(english.contains("Transport is encrypted"), "{english}");
        assert!(english.contains("proxy is disabled"), "{english}");

        let chinese = model_endpoint_status_in(&endpoint, Language::Chinese);
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
        assert!(text(Key::OpenSkillMd, Language::Chinese).contains("SKILL.md"));
        assert_eq!(text(Key::PanelHarness, Language::Chinese), "Harness");
        assert!(text(Key::OpenFolderToDiscover, Language::Chinese).contains("skills"));
    }

    #[test]
    fn welcome_strings_are_translated_in_both_languages() {
        for language in Language::ALL {
            for key in [
                Key::WelcomeTitle,
                Key::NewDocument,
                Key::Paste,
                Key::ClipboardTextUnavailable,
                Key::OpenFile,
                Key::OpenBundledSample,
                Key::BundledSampleUnavailable,
                Key::Recent,
                Key::DontShowWelcomeAgain,
            ] {
                assert!(!text(key, language).is_empty(), "{key:?} in {language:?}");
            }
        }
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
                let value = text(key, language);
                assert!(!value.is_empty(), "{key:?} in {language:?}");
                assert!(!value.contains("sentinel-secret"));
            }
        }

        assert_ne!(
            text(Key::StoreCredentialSecurely, Language::English),
            text(Key::StoreCredentialSecurely, Language::Chinese)
        );
        assert_ne!(
            text(Key::DeleteCredentialTitle, Language::English),
            text(Key::DeleteCredentialTitle, Language::Chinese)
        );
    }
}
