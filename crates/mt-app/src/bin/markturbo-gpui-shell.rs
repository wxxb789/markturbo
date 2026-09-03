#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use gpui::*;
use gpui_component::Root;
use mt_app::{assets::Assets, startup};

const APP_ID: &str = "io.github.wxxb789.markturbo";

struct BareShell {
    focus: FocusHandle,
}

impl Focusable for BareShell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for BareShell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("bare-shell")
            .size_full()
            .track_focus(&self.focus)
            .on_action(|_: &startup::AcknowledgeStartupInput, _, _| {
                startup::record(startup::StartupEvent::FirstInputHandled);
            })
    }
}

fn main() {
    startup::record(startup::StartupEvent::ProcessStarted);

    #[cfg(target_os = "windows")]
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true")
    };

    gpui_platform::application().with_assets(Assets).run(|cx| {
        cx.set_app_identity(APP_ID, "markturbo");
        gpui_component::init(cx);
        startup::init(cx);

        let mut window_size = size(px(1400.0), px(900.0));
        if let Some(display) = cx.primary_display() {
            let bounds = display.bounds().size;
            window_size.width = window_size.width.min(bounds.width * 0.9);
            window_size.height = window_size.height.min(bounds.height * 0.9);
        }
        let window_bounds = Bounds::centered(None, window_size, cx);

        cx.spawn(async move |cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(window_bounds)),
                app_id: Some(APP_ID.to_owned()),
                window_min_size: Some(Size {
                    width: px(720.),
                    height: px(480.),
                }),
                kind: WindowKind::Normal,
                ..gpui_component::TitleBar::window_options()
            };
            let mut shell = None;
            let window = cx
                .open_window(options, |window, cx| {
                    let view = cx.new(|cx| BareShell {
                        focus: cx.focus_handle(),
                    });
                    shell = Some(view.clone());
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("failed to open bare GPUI window");

            window
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    window.set_window_title("markturbo");
                    if let Some(shell) = shell {
                        window.focus(&shell.read(cx).focus_handle(cx), cx);
                    }
                    startup::record(startup::StartupEvent::InitialStateReady(
                        startup::InitialStartupState::Bare,
                    ));
                    startup::schedule_first_frame_milestone(window);
                    cx.on_release(|_, cx| cx.quit()).detach();
                })
                .expect("failed to configure bare GPUI window");
        })
        .detach();
    });
}
