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
//! sits — so [`Schema`] owns those three answers and everything else is shared.

use std::sync::Arc;

use mt_doc::translate::TranslationService;

use crate::settings::AppSettings;

/// A request/response wire format.
///
/// Named for the API shape rather than the vendor: an OpenAI-compatible
/// endpoint (vLLM, Ollama, OpenRouter, LM Studio, Azure) speaks Chat
/// Completions regardless of who runs it, so pointing the base URL at one is
/// all that is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schema {
    /// `POST /v1/messages` — Anthropic.
    AnthropicMessages,
    /// `POST /v1/chat/completions` — OpenAI and every compatible server.
    OpenAiChat,
    /// `POST /v1/responses` — OpenAI's newer API.
    OpenAiResponses,
}

impl Schema {
    pub const ALL: [Schema; 3] = [
        Schema::AnthropicMessages,
        Schema::OpenAiChat,
        Schema::OpenAiResponses,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Schema::AnthropicMessages => "Anthropic Messages",
            Schema::OpenAiChat => "OpenAI Chat Completions",
            Schema::OpenAiResponses => "OpenAI Responses",
        }
    }

    /// Stable id, stored in settings and never shown to the user.
    pub fn key(self) -> &'static str {
        match self {
            Schema::AnthropicMessages => "anthropic",
            Schema::OpenAiChat => "openai-chat",
            Schema::OpenAiResponses => "openai-responses",
        }
    }

    pub fn from_key(key: &str) -> Option<Schema> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }

    /// The base URL used when the user has not set one.
    pub fn default_base_url(self) -> &'static str {
        match self {
            Schema::AnthropicMessages => "https://api.anthropic.com",
            Schema::OpenAiChat | Schema::OpenAiResponses => "https://api.openai.com",
        }
    }

    /// The path appended to the base URL.
    fn path(self) -> &'static str {
        match self {
            Schema::AnthropicMessages => "/v1/messages",
            Schema::OpenAiChat => "/v1/chat/completions",
            Schema::OpenAiResponses => "/v1/responses",
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

    /// The environment variable the API key is read from.
    ///
    /// Environment-only, and deliberately so: writing a key into a settings file
    /// the app rewrites on every toggle is how keys end up in backups and
    /// screenshots.
    pub fn key_env(self) -> &'static str {
        match self {
            Schema::AnthropicMessages => "ANTHROPIC_API_KEY",
            Schema::OpenAiChat | Schema::OpenAiResponses => "OPENAI_API_KEY",
        }
    }

    /// The model used when the user has not named one.
    pub fn default_model(self) -> &'static str {
        match self {
            Schema::AnthropicMessages => "claude-sonnet-5",
            Schema::OpenAiChat | Schema::OpenAiResponses => "gpt-5",
        }
    }

    /// How the key is presented.
    fn auth_headers(self, key: &str) -> Vec<(String, String)> {
        match self {
            Schema::AnthropicMessages => vec![
                ("x-api-key".to_string(), key.to_string()),
                ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ],
            Schema::OpenAiChat | Schema::OpenAiResponses => {
                vec![("authorization".to_string(), format!("Bearer {key}"))]
            }
        }
    }

    /// Build the request body.
    fn request(self, model: &str, system: &str, user: &str) -> serde_json::Value {
        match self {
            Schema::AnthropicMessages => serde_json::json!({
                "model": model,
                "max_tokens": 8192,
                "system": system,
                "messages": [{ "role": "user", "content": user }],
            }),
            Schema::OpenAiChat => serde_json::json!({
                "model": model,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user },
                ],
            }),
            Schema::OpenAiResponses => serde_json::json!({
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
            Schema::AnthropicMessages => response
                .get("content")
                .and_then(|c| c.as_array())
                .and_then(|items| items.iter().find_map(|i| i.get("text")?.as_str()))
                .map(str::to_string),
            Schema::OpenAiChat => response
                .get("choices")
                .and_then(|c| c.as_array())
                .and_then(|items| items.first())
                .and_then(|choice| choice.get("message")?.get("content")?.as_str())
                .map(str::to_string),
            Schema::OpenAiResponses => response
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

/// Providers this build can construct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// No network, no key: marks each translatable segment. Useful for
    /// verifying that structure is preserved before wiring a real backend, and
    /// it means the Translate command always does something visible.
    Echo,
    /// A model endpoint speaking one of the supported wire formats.
    Api(Schema),
}

impl Provider {
    pub const ALL: [Provider; 4] = [
        Provider::Api(Schema::AnthropicMessages),
        Provider::Api(Schema::OpenAiChat),
        Provider::Api(Schema::OpenAiResponses),
        Provider::Echo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Provider::Echo => "Echo (offline)",
            Provider::Api(schema) => schema.label(),
        }
    }

    /// Stable id, used in settings and never shown to the user.
    pub fn key(self) -> &'static str {
        match self {
            Provider::Echo => "echo",
            Provider::Api(schema) => schema.key(),
        }
    }

    pub fn from_key(key: &str) -> Option<Provider> {
        if key == "echo" {
            return Some(Provider::Echo);
        }
        Schema::from_key(key).map(Provider::Api)
    }

    /// Whether this provider actually translates.
    ///
    /// Echo does not — it tags segments so the structure-preserving split is
    /// visible. Calling that a translation would be a lie the status bar has to
    /// tell for it.
    pub fn is_real(self) -> bool {
        self != Provider::Echo
    }

    /// Build a service with the defaults, or explain why it is unavailable.
    pub fn build(self) -> Result<Arc<dyn TranslationService>, String> {
        self.build_with(&AppSettings::default())
    }

    /// Build a service configured from `settings`.
    ///
    /// The base URL and model come from settings where set, and from the
    /// schema's defaults where not — so a user who only picks a schema gets a
    /// working endpoint, and one pointing at a self-hosted server overrides just
    /// the URL.
    pub fn build_with(self, settings: &AppSettings) -> Result<Arc<dyn TranslationService>, String> {
        let schema = match self {
            Provider::Echo => return Ok(Arc::new(EchoTranslator)),
            Provider::Api(schema) => schema,
        };
        let api_key = std::env::var(schema.key_env())
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| format!("{} is not set", schema.key_env()))?;

        let base_url = non_empty(&settings.translate_base_url)
            .unwrap_or_else(|| schema.default_base_url().to_string());
        let model = non_empty(&settings.translate_model)
            .or_else(|| non_empty_env("MARKTURBO_TRANSLATE_MODEL"))
            .unwrap_or_else(|| schema.default_model().to_string());

        Ok(Arc::new(ApiTranslator {
            schema,
            base_url,
            api_key,
            model,
        }))
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
            && chosen.build_with(settings).is_ok()
        {
            return Some(chosen);
        }
        Self::available().into_iter().next()
    }
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn non_empty_env(var: &str) -> Option<String> {
    non_empty(&std::env::var(var).ok()?)
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

/// A translator over any of the supported wire formats.
///
/// Segments are sent as a JSON array and returned as one, so the provider
/// cannot merge or reorder them without the count check in
/// [`mt_doc::translate::translate`] catching it.
struct ApiTranslator {
    schema: Schema,
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
        let payload = self.schema.request(&self.model, SYSTEM_PROMPT, &user);

        let headers = self.schema.auth_headers(&self.api_key);
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let url = self.schema.endpoint(&self.base_url);
        let response = post_json(&url, &payload, &headers)?;

        let text = self.schema.extract_text(&response)?;
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
        assert!(Provider::Api(Schema::OpenAiChat).is_real());
    }

    #[test]
    fn an_api_provider_reports_a_missing_key_rather_than_panicking() {
        for schema in Schema::ALL {
            // Only meaningful when the key is absent; skip otherwise so the
            // suite passes on a machine that has one configured.
            if std::env::var(schema.key_env()).is_ok_and(|k| !k.trim().is_empty()) {
                continue;
            }
            // `Arc<dyn TranslationService>` is not Debug, so match rather than
            // unwrap_err.
            let Err(err) = Provider::Api(schema).build() else {
                panic!("{} built without a key", schema.label());
            };
            assert!(err.contains(schema.key_env()), "{err}");
        }
    }

    #[test]
    fn provider_keys_round_trip_and_are_distinct() {
        for provider in Provider::ALL {
            assert_eq!(Provider::from_key(provider.key()), Some(provider));
        }
        assert_eq!(Provider::from_key("nonexistent"), None);

        let labels: std::collections::HashSet<&str> =
            Provider::ALL.iter().map(|p| p.label()).collect();
        assert_eq!(labels.len(), Provider::ALL.len(), "labels must be distinct");
        let keys: std::collections::HashSet<&str> = Provider::ALL.iter().map(|p| p.key()).collect();
        assert_eq!(keys.len(), Provider::ALL.len(), "keys must be distinct");
    }

    #[test]
    fn the_anthropic_key_still_names_the_same_provider() {
        // Settings written before the other two schemas existed store
        // `"anthropic"`; that must keep resolving rather than falling back to
        // Echo and silently ceasing to translate.
        assert_eq!(
            Provider::from_key("anthropic"),
            Some(Provider::Api(Schema::AnthropicMessages))
        );
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
    fn each_schema_builds_the_body_its_api_documents() {
        let body = Schema::AnthropicMessages.request("m", "sys", "hi");
        assert_eq!(body["system"], "sys", "Anthropic takes a top-level system");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert!(body["max_tokens"].is_number(), "Anthropic requires it");

        let body = Schema::OpenAiChat.request("m", "sys", "hi");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "sys");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "hi");

        let body = Schema::OpenAiResponses.request("m", "sys", "hi");
        assert_eq!(body["instructions"], "sys");
        assert_eq!(body["input"], "hi");

        for schema in Schema::ALL {
            assert_eq!(schema.request("the-model", "s", "u")["model"], "the-model");
        }
    }

    #[test]
    fn each_schema_finds_the_text_in_its_own_response_shape() {
        let reply = serde_json::json!({
            "content": [{ "type": "text", "text": "hello" }]
        });
        assert_eq!(
            Schema::AnthropicMessages.extract_text(&reply).unwrap(),
            "hello"
        );

        let reply = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "hello" } }]
        });
        assert_eq!(Schema::OpenAiChat.extract_text(&reply).unwrap(), "hello");

        // The documented Responses shape.
        let reply = serde_json::json!({
            "output": [{
                "type": "message",
                "content": [{ "type": "output_text", "text": "hello" }]
            }]
        });
        assert_eq!(
            Schema::OpenAiResponses.extract_text(&reply).unwrap(),
            "hello"
        );
        // …and the flattened convenience field some servers add.
        let reply = serde_json::json!({ "output_text": "hello" });
        assert_eq!(
            Schema::OpenAiResponses.extract_text(&reply).unwrap(),
            "hello"
        );
    }

    #[test]
    fn an_unrecognized_response_is_an_error_carrying_the_body() {
        // A wrong-schema reply is the likeliest misconfiguration — pointing an
        // OpenAI base URL at the Anthropic schema, say — and the error has to
        // show what came back or it is unactionable.
        let reply = serde_json::json!({ "error": { "message": "nope" } });
        for schema in Schema::ALL {
            let err = schema.extract_text(&reply).unwrap_err().to_string();
            assert!(err.contains("nope"), "{schema:?}: {err}");
        }
    }

    #[test]
    fn each_schema_authenticates_the_way_its_api_expects() {
        let headers = Schema::AnthropicMessages.auth_headers("secret");
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "x-api-key" && v == "secret")
        );
        assert!(headers.iter().any(|(k, _)| k == "anthropic-version"));

        for schema in [Schema::OpenAiChat, Schema::OpenAiResponses] {
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
        assert_ne!(Schema::OpenAiChat.path(), Schema::OpenAiResponses.path());
        let paths: std::collections::HashSet<&str> = Schema::ALL.iter().map(|s| s.path()).collect();
        assert_eq!(paths.len(), Schema::ALL.len());
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        // Users paste base URLs with and without one; `//v1/messages` is routed
        // differently by some gateways and rejected by others.
        for schema in Schema::ALL {
            let with = schema.endpoint("https://example.invalid/");
            let without = schema.endpoint("https://example.invalid");
            assert_eq!(with, without, "{schema:?}");
            assert!(!with.contains("invalid//"), "{with}");
            assert!(with.ends_with(schema.path()), "{with}");
        }
        // A base URL carrying a path prefix (a gateway mount point) is kept.
        assert_eq!(
            Schema::OpenAiChat.endpoint("https://gw.invalid/openai"),
            "https://gw.invalid/openai/v1/chat/completions"
        );
    }

    #[test]
    fn every_schema_has_usable_defaults() {
        // A user who picks a schema and nothing else must get a working
        // endpoint, so none of these may be empty.
        for schema in Schema::ALL {
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
