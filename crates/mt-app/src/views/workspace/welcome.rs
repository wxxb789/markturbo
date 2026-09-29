//! First-use presentation and recently opened target availability.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::ParentElement as _;
use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement as _, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use mt_core::recovery::RecoveryKey;

use super::Workspace;
use crate::i18n;

pub(super) struct WelcomeState {
    pub(super) scroll: ScrollHandle,
    recent_issues: HashMap<PathBuf, Option<i18n::Key>>,
    sample_available: bool,
}

impl Default for WelcomeState {
    fn default() -> Self {
        Self {
            scroll: ScrollHandle::new(),
            recent_issues: HashMap::new(),
            sample_available: false,
        }
    }
}

const WELCOME_NEW_ACCESSIBILITY_ID: &str = "markturbo-welcome-new";
const WELCOME_PASTE_ACCESSIBILITY_ID: &str = "markturbo-welcome-paste";
const WELCOME_OPEN_FILE_ACCESSIBILITY_ID: &str = "markturbo-welcome-open-file";
const WELCOME_OPEN_FOLDER_ACCESSIBILITY_ID: &str = "markturbo-welcome-open-folder";
const WELCOME_OPEN_SAMPLE_ACCESSIBILITY_ID: &str = "markturbo-welcome-open-sample";
const WELCOME_DONT_SHOW_ACCESSIBILITY_ID: &str = "markturbo-welcome-dont-show-again";
pub(super) const WELCOME_KEY_CONTEXT: &str = "Welcome";

pub(super) fn should_show_welcome(initial: Option<&Path>, show_welcome_on_startup: bool) -> bool {
    initial.is_none() && show_welcome_on_startup
}

pub(super) fn recent_target_issue(target: &mt_core::settings::RecentTarget) -> Option<i18n::Key> {
    if !target.path.exists() {
        return Some(i18n::Key::RecentMissing);
    }
    match target.kind {
        mt_core::settings::RecentTargetKind::File
            if target.path.is_file() && mt_core::workspace::is_openable(&target.path) =>
        {
            None
        }
        mt_core::settings::RecentTargetKind::Workspace if target.path.is_dir() => None,
        _ => Some(i18n::Key::RecentUnavailable),
    }
}

impl Workspace {
    /// Capture filesystem availability when the Welcome surface is entered.
    ///
    /// Rendering may happen repeatedly as the window changes size; these probes
    /// belong at the state boundary. Opening a target rechecks its availability.
    pub(super) fn refresh_welcome_availability(&mut self, cx: &gpui_kit::App) {
        self.welcome.recent_issues = crate::settings::AppSettings::global(cx)
            .recent_targets
            .iter()
            .map(|target| (target.path.clone(), recent_target_issue(target)))
            .collect();
        self.welcome.sample_available = crate::app_paths::bundled_sample_available();
    }

    fn welcome_recent_target_issue(
        &self,
        target: &mt_core::settings::RecentTarget,
    ) -> Option<i18n::Key> {
        self.welcome
            .recent_issues
            .get(&target.path)
            .copied()
            .flatten()
    }

    pub(super) fn record_recent_file(&self, path: PathBuf, cx: &mut Context<Self>) {
        self.record_recent_target(path, mt_core::settings::RecentTargetKind::File, cx);
    }

    pub(super) fn record_recent_workspace(&self, path: PathBuf, cx: &mut Context<Self>) {
        self.record_recent_target(path, mt_core::settings::RecentTargetKind::Workspace, cx);
    }

    pub(super) fn record_recent_target(
        &self,
        path: PathBuf,
        kind: mt_core::settings::RecentTargetKind,
        cx: &mut Context<Self>,
    ) {
        let display_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let target = mt_core::settings::RecentTarget::new(path, kind, display_name);
        if crate::settings::AppSettings::global(cx)
            .recent_targets
            .first()
            == Some(&target)
        {
            return;
        }
        crate::settings::AppSettings::update(cx, move |settings| {
            settings.record_recent_target(target);
        });
    }

    pub(super) fn open_recent_target(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let target = crate::settings::AppSettings::global(cx)
            .recent_targets
            .iter()
            .find(|target| target.path == path)
            .cloned();
        let Some(target) = target else { return false };
        if recent_target_issue(&target).is_some() {
            return false;
        }
        self.open_target(target.path, true, window, cx)
    }

    pub(super) fn remove_recent_target(&mut self, path: &Path, cx: &mut Context<Self>) {
        crate::settings::AppSettings::update(cx, |settings| {
            settings.remove_recent_target(path);
        });
    }

    pub(super) fn open_bundled_sample(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_bundled_sample_result(crate::app_paths::bundled_sample_dir(), window, cx);
    }

    pub(super) fn open_bundled_sample_result(
        &mut self,
        sample: std::io::Result<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match sample {
            Ok(path) => {
                self.open_target(path, true, window, cx);
            }
            Err(_) => self.set_status(i18n::t(i18n::Key::BundledSampleUnavailable, cx).into(), cx),
        }
    }

    pub(super) fn dont_show_welcome_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::settings::AppSettings::update(cx, |settings| {
            settings.show_welcome_on_startup = false;
        });
        self.new_memory(String::new(), window, cx);
    }

    pub(super) fn render_welcome(&self, cx: &Context<Self>) -> AnyElement {
        let recents = &crate::settings::AppSettings::global(cx).recent_targets;
        let sample_available = self.welcome.sample_available;

        v_flex()
            .id("welcome")
            .role(gpui_kit::Role::Group)
            .aria_label(i18n::t(i18n::Key::WelcomeTitle, cx))
            .size_full()
            .min_h_0()
            .items_center()
            .overflow_y_scroll()
            .track_scroll(&self.welcome.scroll)
            .px_6()
            .py_8()
            .child(
                v_flex()
                    .w(px(560.))
                    .max_w_full()
                    .flex_shrink_0()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::BookOpen).large())
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(i18n::t(i18n::Key::WelcomeTitle, cx)),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(i18n::t(i18n::Key::WelcomeSubtitle, cx)),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                Button::new("welcome-new")
                                    .icon(IconName::Plus)
                                    .label(i18n::t(i18n::Key::NewDocument, cx))
                                    .accessibility_id(WELCOME_NEW_ACCESSIBILITY_ID)
                                    .primary()
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_new_document(&super::NewDocument, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("welcome-paste")
                                    .icon(IconName::Copy)
                                    .label(i18n::t(i18n::Key::Paste, cx))
                                    .accessibility_id(WELCOME_PASTE_ACCESSIBILITY_ID)
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_paste_into_new(&super::PasteIntoNew, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("welcome-open-file")
                                    .icon(IconName::File)
                                    .label(i18n::t(i18n::Key::OpenFilePicker, cx))
                                    .accessibility_id(WELCOME_OPEN_FILE_ACCESSIBILITY_ID)
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_open_file(&super::OpenFile, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("welcome-open-folder")
                                    .icon(IconName::FolderOpen)
                                    .label(i18n::t(i18n::Key::OpenFolderPicker, cx))
                                    .accessibility_id(WELCOME_OPEN_FOLDER_ACCESSIBILITY_ID)
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_open_folder(&super::OpenFolder, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("welcome-open-sample")
                                    .icon(IconName::BookOpen)
                                    .label(i18n::t(i18n::Key::OpenBundledSample, cx))
                                    .accessibility_id(WELCOME_OPEN_SAMPLE_ACCESSIBILITY_ID)
                                    .disabled(!sample_available)
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_bundled_sample(window, cx);
                                    })),
                            ),
                    )
                    .when(!recents.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .mt_4()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(i18n::t(i18n::Key::Recent, cx)),
                                )
                                .children(recents.iter().map(|target| {
                                    let issue = self.welcome_recent_target_issue(target);
                                    let path_text = SharedString::from(
                                        target.path.to_string_lossy().into_owned(),
                                    );
                                    let label = if target.display_name.is_empty() {
                                        path_text.clone()
                                    } else {
                                        target
                                            .path
                                            .parent()
                                            .map(|parent| {
                                                SharedString::from(format!(
                                                    "{}  {}",
                                                    target.display_name,
                                                    parent.display()
                                                ))
                                            })
                                            .unwrap_or_else(|| {
                                                SharedString::from(target.display_name.clone())
                                            })
                                    };
                                    let open_label =
                                        i18n::open_recent_target_label(&target.path, cx);
                                    let remove_label =
                                        i18n::remove_recent_target_label(&target.path, cx);
                                    let identity =
                                        RecoveryKey::for_path(&target.path).as_str().to_owned();
                                    let path = target.path.clone();
                                    let remove_path = path.clone();
                                    let open_id =
                                        SharedString::from(format!("welcome-recent-{identity}"));
                                    let open_accessibility_id = SharedString::from(format!(
                                        "markturbo-welcome-recent-{identity}"
                                    ));
                                    let remove_id = SharedString::from(format!(
                                        "welcome-recent-remove-{identity}"
                                    ));
                                    let status_id = SharedString::from(format!(
                                        "markturbo-welcome-recent-status-{identity}"
                                    ));
                                    let icon = match target.kind {
                                        mt_core::settings::RecentTargetKind::File => IconName::File,
                                        mt_core::settings::RecentTargetKind::Workspace => {
                                            IconName::Folder
                                        }
                                    };
                                    let open_button = if issue.is_some() {
                                        BaseButton::new(open_id)
                                            .role(gpui_kit::Role::Button)
                                            .disabled(true)
                                            .accessibility_label(open_label)
                                            .accessibility_id(open_accessibility_id)
                                            .a11y_synthetic_children(|builder| {
                                                builder.parent_node().set_disabled();
                                            })
                                            .styles(|styles| {
                                                styles.disabled(|style| {
                                                    style
                                                        .bg(cx
                                                            .theme()
                                                            .input_background()
                                                            .opacity(0.5))
                                                        .border_color(cx.theme().input.opacity(0.5))
                                                        .text_color(
                                                            cx.theme()
                                                                .muted_foreground
                                                                .opacity(0.5),
                                                        )
                                                        .shadow_none()
                                                })
                                            })
                                            .flex()
                                            .flex_1()
                                            .min_w_0()
                                            .h_8()
                                            .px_2p5()
                                            .gap_2()
                                            .items_center()
                                            .justify_center()
                                            .rounded(cx.theme().radius)
                                            .border_1()
                                            .child(Icon::new(icon).small())
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .truncate()
                                                    .child(label),
                                            )
                                            .into_any_element()
                                    } else {
                                        Button::new(open_id)
                                            .icon(icon)
                                            .label(label)
                                            .accessibility_label(open_label)
                                            .tooltip(path_text.clone())
                                            .accessibility_id(open_accessibility_id)
                                            .flex_1()
                                            .min_w_0()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.open_recent_target(&path, window, cx);
                                            }))
                                            .into_any_element()
                                    };
                                    h_flex()
                                        .w_full()
                                        .gap_1()
                                        .items_center()
                                        .child(open_button)
                                        .when_some(issue, move |this, issue| {
                                            let label = i18n::t(issue, cx);
                                            this.child(
                                                div()
                                                    .id(status_id.clone())
                                                    .role(gpui_kit::Role::Label)
                                                    .aria_value(label)
                                                    .accessibility_id(status_id)
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(label),
                                            )
                                        })
                                        .child(
                                            Button::new(remove_id)
                                                .icon(IconName::Close)
                                                .accessibility_label(remove_label)
                                                .accessibility_id(SharedString::from(format!(
                                                    "markturbo-welcome-recent-remove-{identity}"
                                                )))
                                                .small()
                                                .ghost()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.remove_recent_target(&remove_path, cx);
                                                })),
                                        )
                                })),
                        )
                    })
                    .child(
                        Button::new("welcome-dont-show-again")
                            .label(i18n::t(i18n::Key::DontShowWelcomeAgain, cx))
                            .accessibility_id(WELCOME_DONT_SHOW_ACCESSIBILITY_ID)
                            .text()
                            .w_full()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dont_show_welcome_again(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}
