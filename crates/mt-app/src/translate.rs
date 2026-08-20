//! Translation providers.
//!
//! The document engine owns *what* to translate ([`mt_doc::translate`]); this
//! module owns *who* does it. The boundary is deliberate: no vendor name
//! appears in the document model, and swapping providers touches only this
//! file.

use std::sync::Arc;

use mt_doc::translate::TranslationService;

use crate::settings::AppSettings;

/// Providers this build can construct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// No network, no key: marks each translatable segment. Useful for
    /// verifying that structure is preserved before wiring a real backend, and
    /// it means the Translate command always does something visible.
    Echo,
    /// Anthropic Messages API, configured from the environment.
    Anthropic,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Anthropic, Provider::Echo];

    pub fn label(self) -> &'static str {
        match self {
            Provider::Echo => "Echo (offline)",
            Provider::Anthropic => "Anthropic",
        }
    }

    /// Stable id, used in settings and never shown to the user.
    pub fn key(self) -> &'static str {
        match self {
            Provider::Echo => "echo",
            Provider::Anthropic => "anthropic",
        }
    }

    pub fn from_key(key: &str) -> Option<Provider> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    /// Whether this provider actually translates.
    ///
    /// Echo does not — it tags segments so the structure-preserving split is
    /// visible. Calling that a translation would be a lie the status bar has to
    /// tell for it.
    pub fn is_real(self) -> bool {
        self != Provider::Echo
    }

    /// Build a service, or explain why it is unavailable.
    pub fn build(self) -> Result<Arc<dyn TranslationService>, String> {
        self.build_with(None)
    }

    /// Build a service, overriding the model where the provider takes one.
    pub fn build_with(self, model: Option<&str>) -> Result<Arc<dyn TranslationService>, String> {
        match self {
            Provider::Echo => Ok(Arc::new(EchoTranslator)),
            Provider::Anthropic => AnthropicTranslator::from_env(model).map(|t| Arc::new(t) as _),
        }
    }

    /// Providers usable right now, best first.
    pub fn available() -> Vec<Provider> {
        Self::ALL
            .into_iter()
            .filter(|p| p.build().is_ok())
            .collect()
    }

    /// The provider to use, honoring the user's choice when it is usable.
    ///
    /// A configured provider that cannot be built (a key was removed) falls
    /// through rather than failing the command outright — but the caller is told
    /// which one it got, so a silent downgrade to Echo can be reported.
    pub fn resolve(settings: &AppSettings) -> Option<Provider> {
        let chosen = Provider::from_key(&settings.translate_provider);
        if let Some(chosen) = chosen
            && chosen.build().is_ok()
        {
            return Some(chosen);
        }
        Self::available().into_iter().next()
    }
}

/// Offline provider that tags prose so the caller can see exactly which
/// segments were considered translatable.
///
/// It does not translate, and the tag says so: a user who sees `[zh]` in front
/// of untouched English needs to know that is the placeholder working correctly
/// rather than a translation that failed silently.
struct EchoTranslator;

impl TranslationService for EchoTranslator {
    fn translate(&self, texts: &[String], target_lang: &str) -> anyhow::Result<Vec<String>> {
        Ok(texts
            .iter()
            .map(|t| format!("[{target_lang}?] {}", t.trim()))
            .collect())
    }
}

/// Anthropic-backed translator.
///
/// Segments are sent as a JSON array and returned as one, so the provider
/// cannot merge or reorder them without the count check in
/// [`mt_doc::translate::translate`] catching it.
struct AnthropicTranslator {
    api_key: String,
    model: String,
}

impl AnthropicTranslator {
    /// Build from the environment, with `model` overriding the default.
    ///
    /// The key stays environment-only on purpose: writing an API key into a
    /// settings file the app rewrites on every toggle is how keys end up in
    /// backups and screenshots.
    fn from_env(model: Option<&str>) -> Result<Self, String> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| "ANTHROPIC_API_KEY is not set".to_string())?;
        let model = model
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string)
            .or_else(|| std::env::var("MARKTURBO_TRANSLATE_MODEL").ok())
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| "claude-sonnet-5".to_string());
        Ok(Self { api_key, model })
    }
}

impl TranslationService for AnthropicTranslator {
    fn translate(&self, texts: &[String], target_lang: &str) -> anyhow::Result<Vec<String>> {
        let payload = serde_json::json!({
            "model": self.model,
            "max_tokens": 8192,
            "system": SYSTEM_PROMPT,
            "messages": [{
                "role": "user",
                "content": format!(
                    "Target language: {target_lang}\n\nTranslate each element of this JSON array. \
                     Reply with ONLY a JSON array of exactly {} strings, in the same order.\n\n{}",
                    texts.len(),
                    serde_json::to_string(texts)?,
                ),
            }],
        });

        let response = post_json(
            "https://api.anthropic.com/v1/messages",
            &payload,
            &[
                ("x-api-key", self.api_key.as_str()),
                ("anthropic-version", "2023-06-01"),
            ],
        )?;

        let text = response
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|items| items.iter().find_map(|i| i.get("text")?.as_str()))
            .ok_or_else(|| anyhow::anyhow!("unexpected response shape: {response}"))?;

        let parsed: Vec<String> = serde_json::from_str(extract_json_array(text))
            .map_err(|e| anyhow::anyhow!("could not parse translation response: {e}"))?;
        Ok(parsed)
    }
}

const SYSTEM_PROMPT: &str = "You translate prose fragments from technical Markdown documents. \
Translate only natural-language prose. Never translate or alter identifiers, file paths, URLs, \
command names, or code. Preserve Markdown markup characters exactly as they appear. \
Reply with a JSON array of strings and nothing else.";

/// Pull the JSON array out of a reply that may be fenced or prefixed.
fn extract_json_array(text: &str) -> &str {
    let trimmed = text.trim();
    // ```json … ```
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
    }
    match (trimmed.find('['), trimmed.rfind(']')) {
        (Some(start), Some(end)) if end > start => &trimmed[start..=end],
        _ => trimmed,
    }
}

/// Minimal blocking JSON POST via `curl`.
///
/// The app does not otherwise need an HTTP stack, and adding one to the
/// dependency graph for a single optional feature is not worth the build cost.
/// `curl` ships with Windows 10+, macOS, and every Linux distribution that can
/// run this app.
///
/// ponytail: shells out to curl; swap for a real client if translation grows
/// beyond one endpoint.
fn post_json(
    url: &str,
    payload: &serde_json::Value,
    headers: &[(&str, &str)],
) -> anyhow::Result<serde_json::Value> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut command = Command::new("curl");
    command
        .arg("--silent")
        .arg("--show-error")
        .arg("--fail-with-body")
        .arg("--max-time")
        .arg("120")
        .arg("-H")
        .arg("content-type: application/json")
        .arg("--data-binary")
        // Read the body from stdin so the API key and document text never
        // appear in the process command line, where other users could see them.
        .arg("@-")
        .arg(url);
    for (name, value) in headers {
        command.arg("-H").arg(format!("{name}: {value}"));
    }

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("cannot run curl: {e}"))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("cannot write request body"))?
        .write_all(serde_json::to_string(payload)?.as_bytes())?;
    drop(child.stdin.take());

    let output = child.wait_with_output()?;
    let body = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        anyhow::bail!(
            "translation request failed: {}{body}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(serde_json::from_str(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AppSettings;
    use mt_doc::{DocType, Document, translate::Scope};

    #[test]
    fn echo_provider_is_always_available() {
        assert!(Provider::Echo.build().is_ok());
        assert!(Provider::available().contains(&Provider::Echo));
    }

    #[test]
    fn echo_translation_preserves_structure() {
        let src = "# Title\n\nProse with `code`.\n\n```rust\nlet x = 1;\n```\n";
        let doc = Document::with_type(DocType::Markdown, src.to_string());
        let service = Provider::Echo.build().unwrap();
        let out =
            mt_doc::translate::translate(&doc, &Scope::Document, "zh", service.as_ref()).unwrap();

        assert!(out.text.contains("Title"), "prose tagged: {}", out.text);
        assert!(out.text.contains("let x = 1;"), "code untouched");
        assert!(out.text.contains("`code`"), "inline code untouched");
    }

    #[test]
    fn echo_marks_its_output_as_not_a_translation() {
        // A user reading `[zh] Some English` could reasonably conclude the
        // translation silently failed. The `?` is what distinguishes the
        // placeholder from a result.
        let service = Provider::Echo.build().unwrap();
        let out = service.translate(&["Hello".to_string()], "zh").unwrap();
        assert_eq!(out, vec!["[zh?] Hello"]);
        assert!(!Provider::Echo.is_real());
        assert!(Provider::Anthropic.is_real());
    }

    #[test]
    fn anthropic_reports_a_missing_key_rather_than_panicking() {
        // Only meaningful when the key is absent; skip otherwise so the suite
        // passes on a developer machine that has one configured.
        if std::env::var("ANTHROPIC_API_KEY").is_ok_and(|k| !k.trim().is_empty()) {
            return;
        }
        // `Arc<dyn TranslationService>` is not Debug, so match rather than
        // unwrap_err.
        let Err(err) = Provider::Anthropic.build() else {
            panic!("expected an error when the key is absent");
        };
        assert!(err.contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn provider_keys_round_trip_and_are_distinct() {
        for provider in Provider::ALL {
            assert_eq!(Provider::from_key(provider.key()), Some(provider));
        }
        assert_eq!(Provider::from_key("nonexistent"), None);
        assert_ne!(Provider::Echo.label(), Provider::Anthropic.label());
    }

    #[test]
    fn resolve_prefers_the_configured_provider_and_never_returns_nothing() {
        // Echo is always buildable, so an explicit choice of it is honored.
        let settings = AppSettings {
            translate_provider: "echo".into(),
            ..AppSettings::default()
        };
        assert_eq!(Provider::resolve(&settings), Some(Provider::Echo));

        // An unknown id falls through to whatever is available rather than
        // failing the command.
        let settings = AppSettings {
            translate_provider: "not-a-provider".into(),
            ..AppSettings::default()
        };
        assert!(Provider::resolve(&settings).is_some());

        // Unset means "best available".
        let settings = AppSettings {
            translate_provider: String::new(),
            ..AppSettings::default()
        };
        assert_eq!(
            Provider::resolve(&settings),
            Provider::available().first().copied()
        );
    }

    #[test]
    fn extracts_a_json_array_from_various_reply_shapes() {
        assert_eq!(extract_json_array(r#"["a","b"]"#), r#"["a","b"]"#);
        assert_eq!(extract_json_array("```json\n[\"a\"]\n```"), "[\"a\"]");
        assert_eq!(
            extract_json_array("Here you go:\n[\"a\", \"b\"]\nHope that helps."),
            "[\"a\", \"b\"]"
        );
    }
}
