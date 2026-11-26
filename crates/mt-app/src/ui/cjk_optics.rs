use nsh_core::prelude::*;

use crate::ui::{
    fonts::{Companion, CompanionStatus},
    prims,
    tokens::{self, CJK_OPTICS_CAPTURE_VARIANTS, FONT_MODEL, FONT_UI, WEIGHT_MODEL},
};

pub(crate) const CAPTURE_ENV: &str = "MARKTURBO_GOAL17_CJK_OPTICS";
const CAPTURE_VIEW_ID: &str = "goal17-cjk-optics";
const PARAGRAPH: &str = include_str!("cjk-optics-paragraph.txt");

pub(crate) fn capture_requested() -> bool {
    std::env::var_os(CAPTURE_ENV).is_some_and(|value| value == "1")
}

pub(crate) fn capture(companion: Companion) -> View {
    set_color_scheme(nsh_core::platform::Appearance::Light);
    let paragraph = SharedString::from(PARAGRAPH.trim());
    let status = companion.status;
    view()
        .test_id(CAPTURE_VIEW_ID)
        .label("CJK optical sizing comparison")
        .flex_1()
        .h(relative(1.0))
        .w_full()
        .px(px(tokens::SPACE_5))
        .flex_col()
        .justify_center()
        .items_center()
        .gap(px(tokens::SPACE_4))
        .bg(move || tokens::palette().room)
        .text_color(move || tokens::palette().ink)
        .child(prims::ink("CJK optical sizing"))
        .child(
            view()
                .test_id("cjk-optics-font-status")
                .label("CJK companion font status")
                .child(prims::faint(move || {
                    SharedString::from(match status.get() {
                        CompanionStatus::Installed => "CJK companion font loaded",
                        CompanionStatus::Applying => "Loading CJK companion font",
                        _ => "CJK companion font unavailable",
                    })
                })),
        )
        .child(
            view()
                .w_full()
                .flex_row()
                .items_start()
                .gap(px(tokens::SPACE_4))
                .children(CJK_OPTICS_CAPTURE_VARIANTS.into_iter().map(|variant| {
                    view()
                        .test_id(variant.id)
                        .label(variant.label)
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_col()
                        .gap(px(tokens::SPACE_3))
                        .child(
                            text(variant.label)
                                .font_family(FONT_UI)
                                .font_weight(tokens::WEIGHT_MEDIUM)
                                .text_size(px(tokens::TYPE_UI))
                                .line_height(px(tokens::TYPE_UI_LINE)),
                        )
                        .child(
                            prims::script_text(
                                paragraph.clone(),
                                tokens::TYPE_MODEL,
                                WEIGHT_MODEL,
                                variant.optics,
                            )
                            .font_family(FONT_MODEL)
                            .font_weight(WEIGHT_MODEL)
                            .text_size(px(tokens::TYPE_MODEL))
                            .line_height(px(tokens::TYPE_MODEL_LINE)),
                        )
                })),
        )
}

#[cfg(test)]
mod tests {
    use nsh_engine::EngineConfig;
    use nsh_testing::Headless;

    use super::*;

    fn installed_companion() -> Companion {
        let companion = Companion::new();
        companion.status.set(CompanionStatus::Installed);
        companion
    }

    #[component]
    fn OpticalRunProbe() -> impl IntoElement {
        prims::script_text(
            "Latin 42e\u{301} 中文かな한글",
            tokens::TYPE_MODEL,
            WEIGHT_MODEL,
            CJK_OPTICS_CAPTURE_VARIANTS[2].optics,
        )
        .font_family(FONT_MODEL)
        .font_weight(WEIGHT_MODEL)
        .text_size(px(tokens::TYPE_MODEL))
        .line_height(px(tokens::TYPE_MODEL_LINE))
    }

    #[component]
    fn ProductionOpticsProbe() -> impl IntoElement {
        prims::script_text(
            "Latin 中文かな한글",
            tokens::TYPE_MODEL,
            WEIGHT_MODEL,
            tokens::CJK_OPTICS,
        )
        .font_family(FONT_MODEL)
        .font_weight(WEIGHT_MODEL)
        .text_size(px(tokens::TYPE_MODEL))
        .line_height(px(tokens::TYPE_MODEL_LINE))
    }

    #[component]
    fn ScriptBoundaryProbe() -> impl IntoElement {
        prims::script_text(
            "A·B | e\u{0305} | 中\u{05B0} | 中文·中文 | 中文, Latin | Latin, 中文",
            tokens::TYPE_MODEL,
            WEIGHT_MODEL,
            CJK_OPTICS_CAPTURE_VARIANTS[2].optics,
        )
        .font_family(FONT_MODEL)
        .font_weight(WEIGHT_MODEL)
        .text_size(px(tokens::TYPE_MODEL))
    }

    #[component]
    fn CaptureViewProbe() -> impl IntoElement {
        capture(installed_companion())
    }

    #[test]
    fn the_size_and_next_weight_variant_reaches_cjk_spans_only() {
        let mut app = Headless::start(EngineConfig::new(
            nsh_testing::windowed_root::<OpticalRunProbe>().tokens(tokens::theme()),
        ))
        .expect("start the CJK optics probe");
        app.wait_for("text:Latin 42e\u{301} 中文かな한글")
            .expect("the mixed-script paragraph");

        let latin = app
            .screen()
            .span_style("text:Latin")
            .expect("the Latin run style");
        let latin_digits = app
            .screen()
            .span_style("text:42e\u{301}")
            .expect("the Latin digits and mark style");
        let cjk = app
            .screen()
            .span_style("text:中文かな한글")
            .expect("the CJK run style");

        assert_eq!(latin.font_size.0, tokens::TYPE_MODEL);
        assert_eq!(latin.font_weight, WEIGHT_MODEL.0);
        assert_eq!(latin_digits.font_size.0, tokens::TYPE_MODEL);
        assert_eq!(latin_digits.font_weight, WEIGHT_MODEL.0);
        assert_eq!(cjk.font_size.0, tokens::TYPE_MODEL + 1.0);
        assert_eq!(cjk.font_weight, WEIGHT_MODEL.0 + 100);
        assert_eq!(latin.font_family.unwrap().name().as_ref(), FONT_MODEL);
        assert_eq!(cjk.font_family.unwrap().name().as_ref(), FONT_UI);
    }

    #[test]
    fn the_production_default_steps_cjk_runs_on_the_system_fallback_list() {
        let mut app = Headless::start(EngineConfig::new(
            nsh_testing::windowed_root::<ProductionOpticsProbe>().tokens(tokens::theme()),
        ))
        .expect("start the production optics probe");
        app.wait_for("text:Latin 中文かな한글")
            .expect("the mixed-script paragraph");

        let latin = app
            .screen()
            .span_style("text:Latin")
            .expect("the Latin run style");
        let cjk = app
            .screen()
            .span_style("text:中文かな한글")
            .expect("the CJK run style");

        assert_eq!(latin.font_size.0, tokens::TYPE_MODEL);
        assert_eq!(latin.font_weight, WEIGHT_MODEL.0);
        assert_eq!(latin.font_family.unwrap().name().as_ref(), FONT_MODEL);
        assert_eq!(cjk.font_size.0, tokens::TYPE_MODEL + 1.0);
        assert_eq!(cjk.font_weight, WEIGHT_MODEL.0 + 100);
        assert_eq!(cjk.font_family.unwrap().name().as_ref(), FONT_UI);
    }

    #[test]
    fn primary_scripts_and_graphemes_control_optical_styles() {
        let mut app = Headless::start(EngineConfig::new(
            nsh_testing::windowed_root::<ScriptBoundaryProbe>().tokens(tokens::theme()),
        ))
        .expect("start the script-boundary probe");
        app.wait_for("text:A·B | e\u{0305} | 中\u{05B0} | 中文·中文 | 中文, Latin | Latin, 中文")
            .expect("the mixed-script text");

        let style = |selector: &str| {
            let style = app.screen().span_style(selector).expect(selector);
            (
                style.font_size.0,
                style.font_weight,
                style.font_family.unwrap().name().to_string(),
            )
        };
        let latin = (tokens::TYPE_MODEL, WEIGHT_MODEL.0, FONT_MODEL.to_owned());
        let cjk = (
            tokens::TYPE_MODEL + 1.0,
            WEIGHT_MODEL.0 + 100,
            FONT_UI.to_owned(),
        );

        for selector in ["text:A·B", "text:e\u{0305}", "text:, >> nth=1"] {
            assert_eq!(style(selector), latin, "{selector}");
        }
        for selector in ["text:中\u{05B0}", "text:中文·中文", "text:, >> nth=0"] {
            assert_eq!(style(selector), cjk, "{selector}");
        }
    }

    #[test]
    fn the_capture_variants_preserve_the_base_and_next_weight_values() {
        let cjk = CJK_OPTICS_CAPTURE_VARIANTS.map(|variant| {
            (
                variant.optics.size(tokens::TYPE_MODEL, true),
                variant.optics.weight(WEIGHT_MODEL, true).0,
            )
        });
        assert_eq!(cjk, [(22.0, 300), (23.0, 300), (23.0, 400)]);
        let latin = CJK_OPTICS_CAPTURE_VARIANTS.map(|variant| {
            (
                variant.optics.size(tokens::TYPE_MODEL, false),
                variant.optics.weight(WEIGHT_MODEL, false).0,
            )
        });
        assert_eq!(latin, [(22.0, 300); 3]);
    }

    #[test]
    fn capture_view_renders_all_variants_with_the_shared_paragraph() {
        let mut app = Headless::start(EngineConfig::new(
            nsh_testing::windowed_root::<CaptureViewProbe>().tokens(tokens::theme()),
        ))
        .expect("start the capture view");
        app.wait_for("text:CJK optical sizing")
            .expect("the comparison heading");
        for variant in CJK_OPTICS_CAPTURE_VARIANTS {
            app.wait_for(&format!("id:{}", variant.id))
                .expect("a variant column");
            app.wait_for(&format!("text:{}", variant.label))
                .expect("a variant label");
        }

        let texts = app.screen().texts();
        assert_eq!(
            texts.iter().filter(|text| text.contains("Latin")).count(),
            3
        );
        assert_eq!(texts.iter().filter(|text| text.contains("中文")).count(), 3);

        let cjk_styles = CJK_OPTICS_CAPTURE_VARIANTS.map(|variant| {
            app.screen()
                .span_style(&format!("id:{} >> text:中文", variant.id))
                .expect("the CJK run in each capture column")
        });
        let styles = cjk_styles.map(|style| {
            (
                style.font_size.0,
                style.font_weight,
                style.font_family.unwrap().name().to_string(),
            )
        });
        assert_eq!(
            styles,
            [
                (tokens::TYPE_MODEL, WEIGHT_MODEL.0, FONT_MODEL.to_owned()),
                (
                    tokens::TYPE_MODEL + 1.0,
                    WEIGHT_MODEL.0,
                    FONT_MODEL.to_owned()
                ),
                (
                    tokens::TYPE_MODEL + 1.0,
                    WEIGHT_MODEL.0 + 100,
                    FONT_UI.to_owned()
                ),
            ]
        );
    }

    #[test]
    fn capture_view_reports_the_registered_companion_status() {
        let mut app = Headless::start(EngineConfig::new(
            nsh_testing::windowed_root::<CaptureViewProbe>().tokens(tokens::theme()),
        ))
        .expect("start the capture view");
        app.wait_for("text:CJK companion font loaded")
            .expect("the registered companion status");
    }
}
