//! Localized status copy for the review, revision and translation flows.

use mt_core::settings::Language;

use crate::ui::titlebar;

pub(crate) enum S {
    ReviewUnavailable,
    ReviewRequestRejected,
    ContextSelectionRejected,
    ContextRequestRejected,
    ReviewConsentRejected,
    RevisionReady,
    RevisionUnavailable,
    RevisionAnswersRejected,
    RevisionModelUnavailable,
    RevisionRequestRejected,
    RevisionConsentRejected,
    RevisionFailed,
    RevisionTaskFailed,
    RevisionOtherTab,
    RevisionUnknownChange,
    RevisionCannotApply,
    RevisionCannotUndo,
    RevisionApplied,
    RevisionNoChanges,
    TranslationNoText,
    TranslationUnavailable,
    TranslationFailed,
    CancelledNothingApplied,
}

impl S {
    pub(crate) fn text(self, language: Language) -> &'static str {
        match (self, language) {
            (S::ReviewUnavailable, Language::English) => "Review is unavailable",
            (S::ReviewUnavailable, Language::Chinese) => "审查不可用",
            (S::ReviewRequestRejected, Language::English) => "The review request was rejected",
            (S::ReviewRequestRejected, Language::Chinese) => "审查请求被拒绝",
            (S::ContextSelectionRejected, Language::English) => {
                "The Agent Context selection was rejected"
            }
            (S::ContextSelectionRejected, Language::Chinese) => "智能体上下文选择被拒绝",
            (S::ContextRequestRejected, Language::English) => {
                "The Agent Context could not join the review"
            }
            (S::ContextRequestRejected, Language::Chinese) => "智能体上下文无法加入审查",
            (S::ReviewConsentRejected, Language::English) => "The review consent was rejected",
            (S::ReviewConsentRejected, Language::Chinese) => "审查授权被拒绝",
            (S::RevisionReady, Language::English) => {
                "Revision ready. Each accepted change enters the buffer at once; nothing is saved."
            }
            (S::RevisionReady, Language::Chinese) => {
                "修订已就绪。接受的修改会立即写入缓冲区；不会自动保存。"
            }
            (S::RevisionUnavailable, Language::English) => {
                "Run Review first, then prepare a revision."
            }
            (S::RevisionUnavailable, Language::Chinese) => "请先运行审查，再准备修订。",
            (S::RevisionAnswersRejected, Language::English) => "Answers rejected",
            (S::RevisionAnswersRejected, Language::Chinese) => "回答被拒绝",
            (S::RevisionModelUnavailable, Language::English) => "Revision unavailable",
            (S::RevisionModelUnavailable, Language::Chinese) => "修订不可用",
            (S::RevisionRequestRejected, Language::English) => "The revision request was rejected",
            (S::RevisionRequestRejected, Language::Chinese) => "修订请求被拒绝",
            (S::RevisionConsentRejected, Language::English) => "The revision consent was rejected",
            (S::RevisionConsentRejected, Language::Chinese) => "修订授权被拒绝",
            (S::RevisionFailed, Language::English) => "Revision request failed",
            (S::RevisionFailed, Language::Chinese) => "修订请求失败",
            (S::RevisionTaskFailed, Language::English) => "The revision could not be completed",
            (S::RevisionTaskFailed, Language::Chinese) => "修订未能完成",
            (S::RevisionOtherTab, Language::English) => {
                "This revision belongs to another tab. Switch back to that tab to accept or reject changes."
            }
            (S::RevisionOtherTab, Language::Chinese) => {
                "此修订属于另一个标签页。请切回该标签页以接受或拒绝修改。"
            }
            (S::RevisionUnknownChange, Language::English) => {
                "This change is not part of the revision."
            }
            (S::RevisionUnknownChange, Language::Chinese) => "此修改不属于该修订。",
            (S::RevisionCannotApply, Language::English) => {
                "Can't apply — this section was edited since the review."
            }
            (S::RevisionCannotApply, Language::Chinese) => "无法应用——此段落自审查后已被修改。",
            (S::RevisionCannotUndo, Language::English) => {
                "Can't undo — this section was edited after the change was accepted."
            }
            (S::RevisionCannotUndo, Language::Chinese) => {
                "无法撤销——此段落在接受修订后又被修改过。"
            }
            (S::RevisionApplied, Language::English) => {
                "Approved changes applied to the document — press {mod+S} to save to disk."
            }
            (S::RevisionApplied, Language::Chinese) => {
                "已批准的修改已写入文档——按 {mod+S} 保存到磁盘。"
            }
            (S::RevisionNoChanges, Language::English) => {
                "No changes were approved — the document is unchanged."
            }
            (S::RevisionNoChanges, Language::Chinese) => "未批准任何修改——文档保持不变。",
            (S::TranslationNoText, Language::English) => "No translatable text in this document.",
            (S::TranslationNoText, Language::Chinese) => "此文档中没有可翻译的文本。",
            (S::TranslationUnavailable, Language::English) => "Translation is unavailable",
            (S::TranslationUnavailable, Language::Chinese) => "翻译不可用",
            (S::TranslationFailed, Language::English) => "Translation failed",
            (S::TranslationFailed, Language::Chinese) => "翻译失败",
            (S::CancelledNothingApplied, Language::English) => {
                "Cancelled — nothing will be applied."
            }
            (S::CancelledNothingApplied, Language::Chinese) => "已取消——不会应用任何内容。",
        }
    }
}

/// The status for an operation that failed, with the error's own text.
pub(crate) fn failed(s: S, language: Language, error: &dyn std::fmt::Display) -> String {
    let label = s.text(language);
    match language {
        Language::English => format!("{label} ({error})."),
        Language::Chinese => format!("{label}（{error}）。"),
    }
}

/// Why a revision request failed, with what happens to the answers.
pub(crate) fn revision_failed(language: Language, error: &dyn std::fmt::Display) -> String {
    let label = S::RevisionFailed.text(language);
    match language {
        Language::English => {
            format!("{label} ({error}). Your answers are kept — try again.")
        }
        Language::Chinese => format!("{label}（{error}）。你的回答已保留——请重试。"),
    }
}

/// Why a revision task could not finish, with what happens to the answers.
pub(crate) fn revision_task_failed(language: Language, error: &dyn std::fmt::Display) -> String {
    let label = S::RevisionTaskFailed.text(language);
    match language {
        Language::English => {
            format!("{label} ({error}). Your answers are kept — try again.")
        }
        Language::Chinese => format!("{label}（{error}）。你的回答已保留——请重试。"),
    }
}

/// Why a translation failed, with what happened to the document.
pub(crate) fn translation_failed(language: Language, error: &dyn std::fmt::Display) -> String {
    let label = S::TranslationFailed.text(language);
    match language {
        Language::English => {
            format!("{label} ({error}). The document was not changed — try again.")
        }
        Language::Chinese => format!("{label}（{error}）。文档未做任何更改——请重试。"),
    }
}

/// What a completed translation did, and the key that saves it.
pub(crate) fn translation_done(language: Language, segments: usize, target: &str) -> String {
    let shortcut = titlebar::shortcut(false, "S");
    match language {
        Language::English => {
            format!("Translated {segments} segments into {target}. Press {shortcut} to save.")
        }
        Language::Chinese => {
            format!("已将 {segments} 个段落翻译为 {target}。按 {shortcut} 保存。")
        }
    }
}

/// The ready line, with its key.
pub(crate) fn revision_applied(language: Language) -> String {
    let shortcut = titlebar::shortcut(false, "S");
    S::RevisionApplied
        .text(language)
        .replace("{mod+S}", &shortcut)
}
