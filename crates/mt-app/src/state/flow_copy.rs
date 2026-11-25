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
    RevisionRequestRejected,
    RevisionConsentRejected,
    RevisionFailed,
    RevisionTaskFailed,
    RevisionOtherTab,
    ReviewWaitingElsewhere,
    RevisionWaitingElsewhere,
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
            (S::ReviewWaitingElsewhere, Language::English) => {
                "Waiting for Review on another document."
            }
            (S::ReviewWaitingElsewhere, Language::Chinese) => "正在等待另一个文档的审查。",
            (S::RevisionWaitingElsewhere, Language::English) => {
                "Waiting for Revision on another document."
            }
            (S::RevisionWaitingElsewhere, Language::Chinese) => "正在等待另一个文档的修订。",
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

/// The localized label, with the app's own English detail in the English
/// line only: the detail is provider or parser text the app itself produces,
/// so the Chinese line states the outcome and the next step instead of
/// splicing it (a provider's English is not the operating system's text; see
/// the contract's Chinese error scope).
pub(crate) fn failed(s: S, language: Language, error: &dyn std::fmt::Display) -> String {
    let label = s.text(language);
    match language {
        Language::English => format!("{label} ({error})."),
        Language::Chinese => format!("{label}。"),
    }
}

// A Revision that could not be prepared because the model or its credential
// is not set up: the outcome and the next step, in both languages, with no
// provider or platform detail.
pub(crate) fn revision_unavailable(language: Language) -> String {
    match language {
        Language::English => {
            "Revision is unavailable. Check the model and API key in Settings.".to_owned()
        }
        Language::Chinese => "修订不可用。请在设置中检查模型和 API 密钥。".to_owned(),
    }
}

// A Translation that could not be prepared because the model or its
// credential is not set up: the outcome and the next step, in both
// languages, with no provider or platform detail.
pub(crate) fn translation_unavailable(language: Language) -> String {
    match language {
        Language::English => {
            "Translation is unavailable. Check the model and API key in Settings.".to_owned()
        }
        Language::Chinese => "翻译不可用。请在设置中检查模型和 API 密钥。".to_owned(),
    }
}

pub(crate) fn revision_failed(language: Language, error: &dyn std::fmt::Display) -> String {
    let label = S::RevisionFailed.text(language);
    match language {
        Language::English => {
            format!("{label} ({error}). Your answers are kept — try again.")
        }
        Language::Chinese => format!("{label}。你的回答已保留——请重试。"),
    }
}

pub(crate) fn revision_task_failed(language: Language, error: &dyn std::fmt::Display) -> String {
    let label = S::RevisionTaskFailed.text(language);
    match language {
        Language::English => {
            format!("{label} ({error}). Your answers are kept — try again.")
        }
        Language::Chinese => format!("{label}。你的回答已保留——请重试。"),
    }
}

/// Why a translation failed, with what happened to the document. Every
/// `TranslationError` is the app's own English, so the Chinese line states
/// the outcome without splicing it; only the operating system's text may keep
/// its language.
pub(crate) fn translation_failed(language: Language, error: &dyn std::fmt::Display) -> String {
    let label = S::TranslationFailed.text(language);
    match language {
        Language::English => {
            format!("{label} ({error}). The document was not changed — try again.")
        }
        Language::Chinese => format!("{label}。文档未做任何更改——请重试。"),
    }
}

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

pub(crate) fn revision_applied(language: Language) -> String {
    let shortcut = titlebar::shortcut(false, "S");
    S::RevisionApplied
        .text(language)
        .replace("{mod+S}", &shortcut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_core::{
        model::{EndpointIdentity, Provider},
        translate::provider::TranslationError,
    };

    #[test]
    fn waiting_elsewhere_names_the_kind_in_both_languages() {
        assert_eq!(
            S::ReviewWaitingElsewhere.text(Language::English),
            "Waiting for Review on another document."
        );
        assert_eq!(
            S::ReviewWaitingElsewhere.text(Language::Chinese),
            "正在等待另一个文档的审查。"
        );
        assert_eq!(
            S::RevisionWaitingElsewhere.text(Language::English),
            "Waiting for Revision on another document."
        );
        assert_eq!(
            S::RevisionWaitingElsewhere.text(Language::Chinese),
            "正在等待另一个文档的修订。"
        );
    }

    #[test]
    fn a_prepare_failure_says_the_next_step_in_both_languages() {
        assert_eq!(
            revision_unavailable(Language::English),
            "Revision is unavailable. Check the model and API key in Settings."
        );
        assert_eq!(
            revision_unavailable(Language::Chinese),
            "修订不可用。请在设置中检查模型和 API 密钥。"
        );
        assert_eq!(
            translation_unavailable(Language::English),
            "Translation is unavailable. Check the model and API key in Settings."
        );
        assert_eq!(
            translation_unavailable(Language::Chinese),
            "翻译不可用。请在设置中检查模型和 API 密钥。"
        );
        for message in [
            revision_unavailable(Language::Chinese),
            translation_unavailable(Language::Chinese),
        ] {
            assert!(
                !message.contains("OpenAI") && !message.contains("error"),
                "no provider or platform detail in the Chinese line: {message}"
            );
        }
    }

    #[test]
    fn a_chinese_translation_failure_omits_the_app_english() {
        let endpoint =
            EndpointIdentity::parse(Provider::OpenAiChat, None).expect("the default endpoint");
        let error = TranslationError::InvalidTranslationResponse {
            provider: Provider::OpenAiChat,
            endpoint,
        };
        let message = translation_failed(Language::Chinese, &error);
        assert!(
            !message.contains("invalid") && !message.contains("OpenAI"),
            "the app's English detail is not spliced: {message}"
        );
        assert!(message.contains("翻译失败"), "the localized label stays");
        assert!(message.contains("请重试"), "the next step stays");
    }

    #[test]
    fn a_chinese_revision_failure_omits_the_provider_english() {
        let error = "Revision provider response did not match revision-v1";
        let message = revision_failed(Language::Chinese, &error);
        assert!(
            !message.contains("provider") && !message.contains("revision-v1"),
            "the provider's English is not spliced: {message}"
        );
        assert!(
            message.contains("修订请求失败"),
            "the localized label stays"
        );
        assert!(message.contains("你的回答已保留"), "the next step stays");
    }

    #[test]
    fn a_chinese_generic_failure_omits_the_app_english() {
        let error = "TranslationError::InvalidTranslationResponse";
        let message = failed(S::TranslationUnavailable, Language::Chinese, &error);
        assert!(
            !message.contains("TranslationError") && !message.contains("Invalid"),
            "the app's English is not spliced: {message}"
        );
        assert!(message.contains("翻译不可用"), "the localized label stays");
        // The English line keeps the detail.
        let english = failed(S::TranslationUnavailable, Language::English, &error);
        assert!(english.contains(error), "the English line keeps the detail");
    }
}
