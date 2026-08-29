# Upgrade GPUI Without Preview Regression

Upgrade the pinned Zed/GPUI revision and the `gpui-component` dependency family.
Keep recursive layout stack safety explicit now that `stacker` is opt-in, and
align Web table headers with the Native secondary foreground token.

Also update `notify-debouncer-full`, `toml`, and `log`. Do not update the
`windows` crate while `lb-wry` still shares the older source boundary.

Verify the release-profile workspace tests, Python harness, Windows WebView
acceptance, Clippy, and the intended GPUI feature graph. An upgrade that compiles
but destabilizes preview behavior is not complete.
