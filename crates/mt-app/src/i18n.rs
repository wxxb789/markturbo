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

use crate::settings::{AppSettings, Language};

/// Every string the interface shows.
///
/// Adding a variant is a compile error in each language's `match`, which is the
/// point: a language that silently lost a string would be discovered by a user,
/// not by the build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    // Title bar and commands
    OpenFolder,
    Translate,
    Settings,
    Save,

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
    Provider,
    Model,
    BaseUrl,
    TargetLanguage,
    Skills,
    Discovery,
    IncludeGlobalSkills,
    ShowInternalSkills,
    SyncScrolling,
    ApiKey,
    Editor,
    SplitView,
    ProviderBestAvailable,

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

fn english(key: Key) -> &'static str {
    match key {
        Key::OpenFolder => "Open Folder",
        Key::Translate => "Translate",
        Key::Settings => "Settings",
        Key::Save => "Save",

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
        Key::Provider => "Provider",
        Key::Model => "Model",
        Key::BaseUrl => "Base URL",
        Key::TargetLanguage => "Target language",
        Key::Skills => "Skills",
        Key::Discovery => "Discovery",
        Key::IncludeGlobalSkills => "Include global skills",
        Key::ShowInternalSkills => "Show internal skills",
        Key::SyncScrolling => "Sync scrolling in Split",
        Key::ApiKey => "API key",
        Key::Editor => "Editor",
        Key::SplitView => "Split view",
        Key::ProviderBestAvailable => "Best available",

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
            "The wire format to speak, not the vendor — any server that speaks one works. \
             Anthropic Messages reads ANTHROPIC_API_KEY; both OpenAI formats read \
             OPENAI_API_KEY — unless an API key is set below, which takes priority."
        }
        Key::ApiKeyHelp => {
            "Takes priority over the environment variable. Leave empty to use that instead — a \
             key in the environment never touches disk, which is the safer option if you want \
             it. Stored as plain text in settings.toml."
        }
        Key::BaseUrlHelp => {
            "Leave empty for the vendor's own endpoint. Set it to reach an OpenAI-compatible \
             server — vLLM, Ollama, OpenRouter, LM Studio, Azure — which needs nothing else, \
             since the wire format is the same. Include the version segment, e.g. \
             `http://localhost:11434/v1`."
        }
        Key::ModelHelp => "Leave empty for the provider's default.",
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
        Key::Translate => "翻译",
        Key::Settings => "设置",
        Key::Save => "保存",

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
        Key::Provider => "服务商",
        Key::Model => "模型",
        Key::BaseUrl => "接口地址",
        Key::TargetLanguage => "目标语言",
        Key::Skills => "Skills",
        Key::Discovery => "发现",
        Key::IncludeGlobalSkills => "包含全局 skills",
        Key::ShowInternalSkills => "显示内部 skills",
        Key::SyncScrolling => "分栏时同步滚动",
        Key::ApiKey => "API key",
        Key::Editor => "编辑器",
        Key::SplitView => "分栏视图",
        Key::ProviderBestAvailable => "自动选择",

        Key::ModeHelp => "“系统”跟随操作系统，并在应用运行期间持续跟随。",
        Key::LightThemeHelp => "当实际模式为浅色时使用。",
        Key::DarkThemeHelp => "当实际模式为深色时使用。",
        Key::LanguageHelp => "界面所用的语言。与下方的翻译目标语言无关，后者针对的是文档。",
        Key::ProviderHelp => {
            "要使用的接口格式，而非服务商 —— 任何使用该格式的服务都可用。Anthropic Messages 读取 \
             ANTHROPIC_API_KEY，两种 OpenAI 格式都读取 OPENAI_API_KEY —— 除非在下方填写了 \
             API key，那将优先生效。"
        }
        Key::ApiKeyHelp => {
            "优先于环境变量。留空则改用环境变量 —— 环境中的 key 不会写入磁盘，若你在意这一点，那是更安全的选择。\
             此处填写的内容以明文存储在 settings.toml 中。"
        }
        Key::BaseUrlHelp => {
            "留空则使用服务商自己的接口地址。填写后可指向任何 OpenAI 兼容的服务 —— vLLM、Ollama、\
             OpenRouter、LM Studio、Azure —— 因为接口格式相同，无需其他配置。需包含版本路径，\
             例如 `http://localhost:11434/v1`。"
        }
        Key::ModelHelp => "留空则使用服务商的默认模型。",
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
        Key::Translate,
        Key::Settings,
        Key::Save,
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
        Key::Provider,
        Key::Model,
        Key::BaseUrl,
        Key::TargetLanguage,
        Key::Skills,
        Key::Discovery,
        Key::IncludeGlobalSkills,
        Key::ShowInternalSkills,
        Key::SyncScrolling,
        Key::ApiKey,
        Key::Editor,
        Key::SplitView,
        Key::ProviderBestAvailable,
        Key::ModeHelp,
        Key::LightThemeHelp,
        Key::DarkThemeHelp,
        Key::LanguageHelp,
        Key::ProviderHelp,
        Key::ApiKeyHelp,
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
}
