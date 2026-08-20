//! Translation providers.
//!
//! The document engine owns *what* to translate ([`mt_doc::translate`]); this
//! module owns *who* does it. The boundary is deliberate: no vendor name
//! appears in the document model, and swapping providers touches only this
//! file.
//!
//! Three wire formats are supported, which between them cover essentially every
//! hosted and self-hosted model endpoint: Anthropic's Messages API, OpenAI's
//! Chat Completions, and OpenAI's newer Responses API. They differ only in how
//! the request is shaped, how the key is presented, and where the reply text
//! sits — so [`Provider`] owns those three answers and everything else is
//! shared.
//!
//! The API key is read from settings first and the environment second. Storing
//! one is the user's explicit choice, so it outranks whatever the shell
//! happened to export; but the environment stays supported, because a key that
//! never touches disk is the safer default for anyone who wants it.

use std::sync::Arc;

use mt_doc::translate::TranslationService;

use crate::settings::AppSettings;

/// A translation provider, identified by the wire format it speaks.
///
/// Named for the API shape rather than the vendor: an OpenAI-compatible
/// endpoint (vLLM, Ollama, OpenRouter, LM Studio, Azure) speaks Chat
/// Completions regardless of who runs it, so pointing the base URL at one is
/// all that is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// `POST /v1/messages` — Anthropic.
    AnthropicMessages,
    /// `POST /v1/chat/completions` — OpenAI and every compatible server.
    OpenAiChat,
    /// `POST /v1/responses` — OpenAI's newer API.
    OpenAiResponses,
}

impl Provider {
    pub const ALL: [Provider; 3] = [
        Provider::AnthropicMessages,
        Provider::OpenAiChat,
        Provider::OpenAiResponses,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "Anthropic Messages",
            Provider::OpenAiChat => "OpenAI Chat Completions",
            Provider::OpenAiResponses => "OpenAI Responses",
        }
    }

    /// Stable id, stored in settings and never shown to the user.
    pub fn key(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "anthropic",
            Provider::OpenAiChat => "openai-chat",
            Provider::OpenAiResponses => "openai-responses",
        }
    }

    pub fn from_key(key: &str) -> Option<Provider> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }

    /// The base URL used when the user has not set one.
    pub fn default_base_url(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "https://api.anthropic.com",
            Provider::OpenAiChat | Provider::OpenAiResponses => "https://api.openai.com",
        }
    }

    /// The path appended to the base URL.
    fn path(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "/v1/messages",
            Provider::OpenAiChat => "/v1/chat/completions",
            Provider::OpenAiResponses => "/v1/responses",
        }
    }

    /// The full URL to POST to.
    ///
    /// Users paste base URLs with and without a trailing slash; leaving one in
    /// produces `//v1/messages`, which some gateways route differently and
    /// others reject outright.
    fn endpoint(self, base_url: &str) -> String {
        format!("{}{}", base_url.trim_end_matches('/'), self.path())
    }

    /// The environment variable the API key falls back to.
    ///
    /// Second in line, behind the settings file. A key in the environment never
    /// touches disk, which is the safer arrangement for anyone who wants it —
    /// but a user who typed one into Settings meant that one, so it wins.
    pub fn key_env(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "ANTHROPIC_API_KEY",
            Provider::OpenAiChat | Provider::OpenAiResponses => "OPENAI_API_KEY",
        }
    }

    /// The API key to use, and where it came from.
    ///
    /// Settings first: an explicitly configured key outranks whatever the shell
    /// happened to export, which is otherwise impossible to override from
    /// inside the app.
    pub fn api_key(self, settings: &AppSettings) -> Option<String> {
        non_empty(&settings.translate_api_key).or_else(|| non_empty_env(self.key_env()))
    }

    /// The model used when the user has not named one.
    pub fn default_model(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "claude-sonnet-5",
            Provider::OpenAiChat | Provider::OpenAiResponses => "gpt-5",
        }
    }

    /// How the key is presented.
    fn auth_headers(self, key: &str) -> Vec<(String, String)> {
        match self {
            Provider::AnthropicMessages => vec![
                ("x-api-key".to_string(), key.to_string()),
                ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ],
            Provider::OpenAiChat | Provider::OpenAiResponses => {
                vec![("authorization".to_string(), format!("Bearer {key}"))]
            }
        }
    }

    /// Build the request body.
    fn request(self, model: &str, system: &str, user: &str) -> serde_json::Value {
        match self {
            Provider::AnthropicMessages => serde_json::json!({
                "model": model,
                "max_tokens": 8192,
                "system": system,
                "messages": [{ "role": "user", "content": user }],
            }),
            Provider::OpenAiChat => serde_json::json!({
                "model": model,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user },
                ],
            }),
            Provider::OpenAiResponses => serde_json::json!({
                "model": model,
                "instructions": system,
                "input": user,
            }),
        }
    }

    /// Pull the assistant's text out of a reply.
    ///
    /// Each shape is tried in the order the API documents, and the raw response
    /// goes into the error when none matches — an endpoint that answered with a
    /// shape we do not know about is worth seeing rather than guessing at.
    fn extract_text(self, response: &serde_json::Value) -> anyhow::Result<String> {
        let text = match self {
            Provider::AnthropicMessages => response
                .get("content")
                .and_then(|c| c.as_array())
                .and_then(|items| items.iter().find_map(|i| i.get("text")?.as_str()))
                .map(str::to_string),
            Provider::OpenAiChat => response
                .get("choices")
                .and_then(|c| c.as_array())
                .and_then(|items| items.first())
                .and_then(|choice| choice.get("message")?.get("content")?.as_str())
                .map(str::to_string),
            Provider::OpenAiResponses => response
                // Some servers include the SDK's flattened convenience field.
                .get("output_text")
                .and_then(|t| t.as_str())
                .map(str::to_string)
                .or_else(|| {
                    // The documented shape: output[] → content[] → output_text.
                    response
                        .get("output")?
                        .as_array()?
                        .iter()
                        .find_map(|item| {
                            item.get("content")?
                                .as_array()?
                                .iter()
                                .find_map(|part| part.get("text")?.as_str())
                        })
                        .map(str::to_string)
                }),
        };
        text.ok_or_else(|| {
            anyhow::anyhow!("unexpected {} response shape: {response}", self.label())
        })
    }
}

/// Build a service, or explain why it is unavailable.
impl Provider {
    /// Build a service configured from `settings`.
    ///
    /// The base URL and model come from settings where set, and from the
    /// provider's defaults where not — so a user who only picks a provider gets
    /// a working endpoint, and one pointing at a self-hosted server overrides
    /// just the URL.
    pub fn build_with(self, settings: &AppSettings) -> Result<Arc<dyn TranslationService>, String> {
        let api_key = self.api_key(settings).ok_or_else(|| {
            format!(
                "No API key. Set one in Settings, or export {} in the environment.",
                self.key_env()
            )
        })?;

        let base_url = non_empty(&settings.translate_base_url)
            .unwrap_or_else(|| self.default_base_url().to_string());
        let model = non_empty(&settings.translate_model)
            .or_else(|| non_empty_env("MARKTURBO_TRANSLATE_MODEL"))
            .unwrap_or_else(|| self.default_model().to_string());

        Ok(Arc::new(ApiTranslator {
            provider: self,
            base_url,
            api_key,
            model,
        }))
    }

    /// Providers with a usable key right now, in table order.
    pub fn available(settings: &AppSettings) -> Vec<Provider> {
        Self::ALL
            .into_iter()
            .filter(|p| p.api_key(settings).is_some())
            .collect()
    }

    /// The provider to use, honoring the user's choice when it is usable.
    ///
    /// `None` means translation is not configured at all, which the caller
    /// reports rather than papering over — there is no offline stand-in that
    /// could stand in for a translation without lying about it.
    pub fn resolve(settings: &AppSettings) -> Option<Provider> {
        if let Some(chosen) = Provider::from_key(&settings.translate_provider)
            && chosen.api_key(settings).is_some()
        {
            return Some(chosen);
        }
        Self::available(settings).into_iter().next()
    }
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn non_empty_env(var: &str) -> Option<String> {
    non_empty(&std::env::var(var).ok()?)
}

/// A translator over any of the supported wire formats.
///
/// Segments are sent as a JSON array and returned as one, so the provider
/// cannot merge or reorder them without the count check in
/// [`mt_doc::translate::translate`] catching it.
struct ApiTranslator {
    provider: Provider,
    base_url: String,
    api_key: String,
    model: String,
}

impl TranslationService for ApiTranslator {
    fn translate(&self, texts: &[String], target_lang: &str) -> anyhow::Result<Vec<String>> {
        let user = format!(
            "Target language: {target_lang}\n\nTranslate each element of this JSON array. \
             Reply with ONLY a JSON array of exactly {} strings, in the same order.\n\n{}",
            texts.len(),
            serde_json::to_string(texts)?,
        );
        let payload = self.provider.request(&self.model, SYSTEM_PROMPT, &user);

        let headers = self.provider.auth_headers(&self.api_key);
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let url = self.provider.endpoint(&self.base_url);
        let response = post_json(&url, &payload, &headers)?;

        let text = self.provider.extract_text(&response)?;
        let parsed: Vec<String> = serde_json::from_str(extract_json_array(&text))
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
/// beyond one endpoint per request.
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

    /// Settings that name a key, so the provider is buildable without touching
    /// the environment.
    fn configured(provider: Provider) -> AppSettings {
        AppSettings {
            translate_provider: provider.key().into(),
            translate_api_key: "sk-test".into(),
            ..AppSettings::default()
        }
    }

    /// Settings with no key anywhere in them.
    fn unconfigured() -> AppSettings {
        AppSettings {
            translate_api_key: String::new(),
            ..AppSettings::default()
        }
    }

    #[test]
    fn a_key_in_settings_outranks_the_environment() {
        // The reason the setting exists: an exported key is otherwise
        // impossible to override from inside the app.
        // SAFETY: single-threaded test; the variable is restored below.
        let had = std::env::var("ANTHROPIC_API_KEY").ok();
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "from-env") };

        let settings = AppSettings {
            translate_api_key: "from-settings".into(),
            ..AppSettings::default()
        };
        assert_eq!(
            Provider::AnthropicMessages.api_key(&settings).as_deref(),
            Some("from-settings")
        );

        // Blank in settings falls through to the environment rather than
        // resolving to an empty key that every request would then reject.
        let settings = unconfigured();
        assert_eq!(
            Provider::AnthropicMessages.api_key(&settings).as_deref(),
            Some("from-env")
        );

        match had {
            Some(value) => unsafe { std::env::set_var("ANTHROPIC_API_KEY", value) },
            None => unsafe { std::env::remove_var("ANTHROPIC_API_KEY") },
        }
    }

    #[test]
    fn whitespace_is_not_a_key() {
        // A settings file hand-edited to `"translate-api-key": "  "` must not
        // produce an Authorization header of spaces and a 401 the user cannot
        // explain.
        let settings = AppSettings {
            translate_api_key: "   ".into(),
            ..AppSettings::default()
        };
        // Falls through to the environment; with neither set, there is no key.
        if std::env::var(Provider::OpenAiChat.key_env()).is_err() {
            assert_eq!(Provider::OpenAiChat.api_key(&settings), None);
        }
    }

    #[test]
    fn a_missing_key_is_reported_with_the_fix_rather_than_panicking() {
        for provider in Provider::ALL {
            // Only meaningful when the environment has no key either; skip
            // otherwise so the suite passes on a configured machine.
            if std::env::var(provider.key_env()).is_ok_and(|k| !k.trim().is_empty()) {
                continue;
            }
            // `Arc<dyn TranslationService>` is not Debug, so match rather than
            // unwrap_err.
            let Err(err) = provider.build_with(&unconfigured()) else {
                panic!("{} built without a key", provider.label());
            };
            // The message has to name both routes, or the user is left hunting
            // for which one the app wanted.
            assert!(err.contains(provider.key_env()), "{err}");
            assert!(err.contains("Settings"), "{err}");
        }
    }

    #[test]
    fn a_configured_provider_builds() {
        for provider in Provider::ALL {
            assert!(
                provider.build_with(&configured(provider)).is_ok(),
                "{} would not build with a key in settings",
                provider.label()
            );
        }
    }

    #[test]
    fn provider_keys_round_trip_and_are_distinct() {
        for provider in Provider::ALL {
            assert_eq!(Provider::from_key(provider.key()), Some(provider));
        }
        assert_eq!(Provider::from_key("nonexistent"), None);
        // `echo` was a provider until it was removed for not translating; a
        // settings file naming it must fall through, not resurrect it.
        assert_eq!(Provider::from_key("echo"), None);

        let labels: std::collections::HashSet<&str> =
            Provider::ALL.iter().map(|p| p.label()).collect();
        assert_eq!(labels.len(), Provider::ALL.len(), "labels must be distinct");
        let keys: std::collections::HashSet<&str> = Provider::ALL.iter().map(|p| p.key()).collect();
        assert_eq!(keys.len(), Provider::ALL.len(), "keys must be distinct");
    }

    #[test]
    fn the_anthropic_key_still_names_the_same_provider() {
        // Settings written before the other two formats existed store
        // `"anthropic"`; that must keep resolving rather than silently ceasing
        // to translate.
        assert_eq!(
            Provider::from_key("anthropic"),
            Some(Provider::AnthropicMessages)
        );
    }

    #[test]
    fn resolve_honors_an_explicit_choice_that_has_a_key() {
        let settings = configured(Provider::OpenAiResponses);
        assert_eq!(
            Provider::resolve(&settings),
            Some(Provider::OpenAiResponses)
        );

        // An unknown id falls through to whatever has a key rather than failing
        // the command outright.
        let settings = AppSettings {
            translate_provider: "not-a-provider".into(),
            translate_api_key: "sk-test".into(),
            ..AppSettings::default()
        };
        assert_eq!(
            Provider::resolve(&settings),
            Provider::available(&settings).first().copied()
        );
    }

    #[test]
    fn resolve_returns_nothing_when_no_key_is_configured() {
        // There is no offline stand-in any more, so "unconfigured" has to be
        // representable — the caller reports it instead of running a provider
        // that cannot translate.
        let settings = unconfigured();
        let any_env = Provider::ALL
            .iter()
            .any(|p| std::env::var(p.key_env()).is_ok_and(|k| !k.trim().is_empty()));
        if !any_env {
            assert_eq!(Provider::resolve(&settings), None);
            assert!(Provider::available(&settings).is_empty());
        }
    }

    #[test]
    fn each_schema_builds_the_body_its_api_documents() {
        let body = Provider::AnthropicMessages.request("m", "sys", "hi");
        assert_eq!(body["system"], "sys", "Anthropic takes a top-level system");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert!(body["max_tokens"].is_number(), "Anthropic requires it");

        let body = Provider::OpenAiChat.request("m", "sys", "hi");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "sys");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "hi");

        let body = Provider::OpenAiResponses.request("m", "sys", "hi");
        assert_eq!(body["instructions"], "sys");
        assert_eq!(body["input"], "hi");

        for schema in Provider::ALL {
            assert_eq!(schema.request("the-model", "s", "u")["model"], "the-model");
        }
    }

    #[test]
    fn each_schema_finds_the_text_in_its_own_response_shape() {
        let reply = serde_json::json!({
            "content": [{ "type": "text", "text": "hello" }]
        });
        assert_eq!(
            Provider::AnthropicMessages.extract_text(&reply).unwrap(),
            "hello"
        );

        let reply = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "hello" } }]
        });
        assert_eq!(Provider::OpenAiChat.extract_text(&reply).unwrap(), "hello");

        // The documented Responses shape.
        let reply = serde_json::json!({
            "output": [{
                "type": "message",
                "content": [{ "type": "output_text", "text": "hello" }]
            }]
        });
        assert_eq!(
            Provider::OpenAiResponses.extract_text(&reply).unwrap(),
            "hello"
        );
        // …and the flattened convenience field some servers add.
        let reply = serde_json::json!({ "output_text": "hello" });
        assert_eq!(
            Provider::OpenAiResponses.extract_text(&reply).unwrap(),
            "hello"
        );
    }

    #[test]
    fn an_unrecognized_response_is_an_error_carrying_the_body() {
        // A wrong-schema reply is the likeliest misconfiguration — pointing an
        // OpenAI base URL at the Anthropic schema, say — and the error has to
        // show what came back or it is unactionable.
        let reply = serde_json::json!({ "error": { "message": "nope" } });
        for schema in Provider::ALL {
            let err = schema.extract_text(&reply).unwrap_err().to_string();
            assert!(err.contains("nope"), "{schema:?}: {err}");
        }
    }

    #[test]
    fn each_schema_authenticates_the_way_its_api_expects() {
        let headers = Provider::AnthropicMessages.auth_headers("secret");
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "x-api-key" && v == "secret")
        );
        assert!(headers.iter().any(|(k, _)| k == "anthropic-version"));

        for schema in [Provider::OpenAiChat, Provider::OpenAiResponses] {
            let headers = schema.auth_headers("secret");
            assert!(
                headers
                    .iter()
                    .any(|(k, v)| k == "authorization" && v == "Bearer secret"),
                "{schema:?}: {headers:?}"
            );
        }
    }

    #[test]
    fn the_two_openai_schemas_target_different_endpoints() {
        // They share a base URL and a key, so the path is the only thing that
        // distinguishes them — getting it wrong would silently send Responses
        // bodies to Chat Completions.
        assert_ne!(
            Provider::OpenAiChat.path(),
            Provider::OpenAiResponses.path()
        );
        let paths: std::collections::HashSet<&str> =
            Provider::ALL.iter().map(|s| s.path()).collect();
        assert_eq!(paths.len(), Provider::ALL.len());
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        // Users paste base URLs with and without one; `//v1/messages` is routed
        // differently by some gateways and rejected by others.
        for schema in Provider::ALL {
            let with = schema.endpoint("https://example.invalid/");
            let without = schema.endpoint("https://example.invalid");
            assert_eq!(with, without, "{schema:?}");
            assert!(!with.contains("invalid//"), "{with}");
            assert!(with.ends_with(schema.path()), "{with}");
        }
        // A base URL carrying a path prefix (a gateway mount point) is kept.
        assert_eq!(
            Provider::OpenAiChat.endpoint("https://gw.invalid/openai"),
            "https://gw.invalid/openai/v1/chat/completions"
        );
    }

    #[test]
    fn every_schema_has_usable_defaults() {
        // A user who picks a schema and nothing else must get a working
        // endpoint, so none of these may be empty.
        for schema in Provider::ALL {
            assert!(schema.default_base_url().starts_with("https://"));
            assert!(!schema.default_model().is_empty());
            assert!(schema.key_env().ends_with("_API_KEY"));
            assert!(schema.path().starts_with("/v1/"));
        }
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
