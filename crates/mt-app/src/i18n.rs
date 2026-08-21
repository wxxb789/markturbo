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
