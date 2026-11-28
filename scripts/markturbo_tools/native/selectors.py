"""The one owner of every UI selector the native harnesses drive.

Each AutomationId is the Rust view's `.test_id(..)` (`crates/mt-app/src/ui`),
and UIA exposes exactly those strings; `scripts/tests/test_native_selectors.py`
guards them against that production source with the inline test modules
stripped. The consent hold must outlast `ui/motion.rs` `FILAMENT_CHARGE_MS`,
which the same guard checks.

This module imports nothing from the package, so both `runtime` and the goal
harnesses can import it without cycles.
"""

from __future__ import annotations

EDITOR_NAME = "Markdown source"

# The title row and title menu (`ui/titlebar.rs`, `ui/shell.rs`).
MENU_BUTTON_AUTOMATION_ID = "title-settings"
MENU_SAVE_AUTOMATION_ID = "title-menu-save"
MENU_SAVE_AS_AUTOMATION_ID = "title-menu-save-as"
MENU_WEB_AUTOMATION_ID = "title-menu-web"
MENU_QUIT_AUTOMATION_ID = "title-menu-quit"
VIEW_BUTTON_AUTOMATION_ID = "title-view"
TITLE_FILE_AUTOMATION_ID = "title-file"
TITLE_REVIEW_AUTOMATION_ID = "title-review"
MENU_CONTEXT_AUTOMATION_ID = "title-menu-context"
MENU_SETTINGS_AUTOMATION_ID = "title-menu-settings"
REVIEW_MENU_AUTOMATION_ID = "title-menu-review-selection"

# The sidebar and its close control (`ui/rail.rs`, `ui/shell.rs`).
SIDEBAR_PANEL_AUTOMATION_ID = "sidebar-panel"
SIDEBAR_ROW_AUTOMATION_ID = "sidebar-file"
SIDEBAR_OUTSIDE_ROW_AUTOMATION_ID = "sidebar-outside-row"
RAIL_FILE_ROW_AUTOMATION_ID = "rail-file-row"
TAB_CLOSE_AUTOMATION_ID = "rail-close"

# The welcome and workspace home (`ui/welcome.rs`).
WELCOME_OPEN_FOLDER_AUTOMATION_ID = "welcome-open-folder"
WELCOME_NEW_FILE_AUTOMATION_ID = "welcome-new-file"
WELCOME_PASTE_AUTOMATION_ID = "welcome-paste"
WELCOME_SAMPLE_AUTOMATION_ID = "welcome-open-sample"
WELCOME_RECENT_AUTOMATION_ID = "welcome-recent"
WELCOME_ACTION_AUTOMATION_IDS = (
    WELCOME_OPEN_FOLDER_AUTOMATION_ID,
    WELCOME_NEW_FILE_AUTOMATION_ID,
    WELCOME_PASTE_AUTOMATION_ID,
    WELCOME_SAMPLE_AUTOMATION_ID,
)
WORKSPACE_HOME_FILE_AUTOMATION_ID = "workspace-home-file"

# The save and close guards (`ui/notices.rs`).
GUARD_BUTTON_AUTOMATION_IDS = {
    "Save": "guard-save",
    "Discard": "guard-dont-save",
    "Cancel": "guard-cancel",
}
QUIT_GUARD_BUTTON_AUTOMATION_IDS = dict(GUARD_BUTTON_AUTOMATION_IDS)
OVERWRITE_GUARD_BUTTON_AUTOMATION_IDS = {
    "Overwrite": "guard-overwrite",
    "Cancel": "guard-cancel",
}
CONFLICT_KEEP_MINE_AUTOMATION_ID = "conflict-keep-mine"
RESTORE_AUTOMATION_ID = "recovery-restore-0"
# Review, consent and Revision (`ui/review.rs`, `ui/consent.rs`,
# `ui/revision.rs`).
REVIEW_CLOSE_AUTOMATION_ID = "review-close"
REVIEW_CANCEL_AUTOMATION_ID = "review-cancel"
REVIEW_AGAIN_AUTOMATION_ID = "review-again"
REVIEW_ANSWER_INPUT_AUTOMATION_ID = "review-answer-input"
REVIEW_ANSWER_AUTOMATION_ID = "review-answer-submit"
PREPARE_REVISION_AUTOMATION_ID = "review-prepare"
CONSENT_SEND_AUTOMATION_ID = "consent-send"
CONSENT_HOLD_SECONDS = 1.8
REVISION_ACCEPT_AUTOMATION_ID = "revision-accept"
REVISION_REJECT_AUTOMATION_ID = "revision-reject"
REVISION_DONE_AUTOMATION_ID = "revision-done"
REVISION_UNDO_AUTOMATION_ID = "revision-undo"
REVISION_COPY_AUTOMATION_ID = "revision-copy"
REVISION_PROPOSAL_AFTER_AUTOMATION_ID = "revision-proposal-after"

# Context, preview and Settings (`ui/context.rs`, `ui/preview.rs`,
# `ui/settings.rs`).
CONTEXT_CLOSE_AUTOMATION_ID = "context-close"
WEB_VIEW_AUTOMATION_ID = "web-view"
# The Settings page proves itself by its root view; auto-save leaves no Save
# control (`ui/settings.rs`; asserted in `goal17_settings.rs`).
SETTINGS_PAGE_AUTOMATION_ID = "settings-page"
# The Goal 17 CJK optical-size comparison view (`ui/cjk_optics.rs`,
# `ui/tokens.rs` `CJK_OPTICS_CAPTURE_VARIANTS`).
CJK_OPTICS_AUTOMATION_ID = "goal17-cjk-optics"
CJK_OPTICS_VARIANTS = (
    ("cjk-optics-plus-0", "+0"),
    ("cjk-optics-plus-1px", "+1px"),
    ("cjk-optics-plus-1px-next-weight", "+1px / next companion weight"),
)
