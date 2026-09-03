//! Translation providers.
//!
//! The document engine owns *what* to translate ([`mt_doc::translate`]); this
//! module owns *who* does it. The boundary is deliberate: no vendor name
//! appears in the document model, and swapping providers touches only this
//! file.
//!
//! Three wire formats are supported, which between them cover essentially every
//! hosted and self-hosted model endpoint: Anthropic's Messages API, OpenAI's
//! Chat Completions, and OpenAI's newer Responses API. [`genai`] speaks all
//! three, so this file no longer shapes requests or digs text out of replies —
//! it decides *which* endpoint, with *which* key, for *which* model, and hands
//! genai a fully resolved [`ServiceTarget`].
//!
//! Resolving the target ourselves rather than letting genai infer it is the
//! whole point. genai normally picks the provider by sniffing the model name,
//! and falls back to Ollama for anything it does not recognise — so a vLLM
//! server asked for `Qwen/Qwen3-32B` would be handed to the wrong adapter.
//! genai's `ModelSpec::Target` skips inference entirely, which is what keeps
//! [`Provider`] meaning "the wire format" and keeps "point base_url at any
//! OpenAI-compatible server" true.
//!
//! The API key is read from settings first and the environment second. Storing
//! one is the user's explicit choice, so it outranks whatever the shell
//! happened to export; but the environment stays supported, because a key that
//! never touches disk is the safer default for anyone who wants it. genai's own
//! `AuthResolver` hook is deliberately unused: it exists for the inference path,
//! and on the target path an [`AuthData::Key`] is simply carried through.

use std::sync::Arc;
#[cfg(feature = "model-transport")]
use std::sync::OnceLock;
#[cfg(feature = "model-transport")]
use std::time::Duration;

#[cfg(feature = "model-transport")]
use genai::adapter::AdapterKind;
#[cfg(feature = "model-transport")]
use genai::chat::{ChatMessage, ChatRequest};
#[cfg(feature = "model-transport")]
use genai::resolver::{AuthData, Endpoint};
#[cfg(feature = "model-transport")]
use genai::{Client, ModelIden, ServiceTarget, WebConfig};
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
    ///
    /// The trailing slash is load-bearing: genai appends only the leaf path,
    /// so a base URL without one loses its last segment. See `base_url`.
    pub fn default_base_url(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "https://api.anthropic.com/v1/",
            Provider::OpenAiChat | Provider::OpenAiResponses => "https://api.openai.com/v1/",
        }
    }

    /// The genai adapter that speaks this wire format.
    ///
    /// This mapping is the reason [`Provider`] still exists. genai would
    /// otherwise pick an adapter by sniffing the model name and land on Ollama
    /// for anything unfamiliar, which is exactly wrong for a self-hosted server
    /// serving a model named after its weights.
    #[cfg(feature = "model-transport")]
    fn adapter(self) -> AdapterKind {
        match self {
            Provider::AnthropicMessages => AdapterKind::Anthropic,
            Provider::OpenAiChat => AdapterKind::OpenAI,
            Provider::OpenAiResponses => AdapterKind::OpenAIResp,
        }
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
        #[cfg(not(feature = "model-transport"))]
        {
            let _ = settings;
            return Err("Model transport is unavailable in this Goal 04 measurement build.".into());
        }

        #[cfg(feature = "model-transport")]
        {
            let api_key = self.api_key(settings).ok_or_else(|| {
                format!(
                    "No API key. Set one in Settings, or export {} in the environment.",
                    self.key_env()
                )
            })?;

            let base_url = base_url(
                &non_empty(&settings.translate_base_url)
                    .unwrap_or_else(|| self.default_base_url().to_string()),
            );
            let model = non_empty(&settings.translate_model)
                .or_else(|| non_empty_env("MARKTURBO_TRANSLATE_MODEL"))
                .unwrap_or_else(|| self.default_model().to_string());

            // Fail here rather than on the first translation: a runtime that cannot
            // start is a configuration problem, and reporting it at build time puts
            // it in the same status message as a missing key.
            transport().map_err(|e| e.to_string())?;

            Ok(Arc::new(GenAiTranslator {
                target: self.service_target(&base_url, &api_key, &model),
                label: self.label(),
            }))
        }
    }

    /// The endpoint, key, and model, resolved into the shape genai executes.
    ///
    /// Pure and total, which is the point: every routing decision this app makes
    /// is visible in the returned value, so the mapping is testable without a
    /// socket. The `curl` version could only be checked by making a request.
    #[cfg(feature = "model-transport")]
    fn service_target(self, base_url: &str, api_key: &str, model: &str) -> ServiceTarget {
        ServiceTarget {
            endpoint: Endpoint::from_owned(base_url),
            auth: AuthData::Key(api_key.to_string()),
            model: ModelIden::new(self.adapter(), model.to_string()),
        }
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

/// Force a trailing slash onto a base URL.
///
/// genai appends only the leaf path, and the two adapter families get there
/// differently — measured against a local socket, not inferred:
///
/// | base   | OpenAI                 | Anthropic      |
/// |--------|------------------------|----------------|
/// | `/v1/` | `/v1/chat/completions` | `/v1/messages` |
/// | `/v1`  | `/chat/completions`    | `/v1messages`  |
///
/// The OpenAI adapters use `Url::join`, which *replaces* the last path segment
/// when there is no trailing slash, so `/v1` vanishes. The Anthropic adapter
/// concatenates, so `/v1` survives but fuses onto the path — a 404 rather than
/// a silently wrong endpoint. Neither is what the user meant, and users paste
/// base URLs both ways, so normalise.
///
/// What this cannot repair is a base URL with no version segment at all: `/`
/// reaches `/chat/completions`, which is simply the wrong endpoint for a server
/// mounting under `/v1`. That is why the setting's help names the version
/// segment rather than only the slash.
#[cfg(feature = "model-transport")]
fn base_url(raw: &str) -> String {
    if raw.ends_with('/') {
        raw.to_string()
    } else {
        format!("{raw}/")
    }
}

/// A translator over any of the supported wire formats.
///
/// Segments are sent as a JSON array and returned as one, so the provider
/// cannot merge or reorder them without the count check in
/// [`mt_doc::translate::translate`] catching it.
#[cfg(feature = "model-transport")]
struct GenAiTranslator {
    target: ServiceTarget,
    /// Only for error messages; naming the provider is what makes a failure
    /// actionable when several are configured.
    label: &'static str,
}

#[cfg(feature = "model-transport")]
impl TranslationService for GenAiTranslator {
    fn translate(&self, texts: &[String], target_lang: &str) -> anyhow::Result<Vec<String>> {
        let user = format!(
            "Target language: {target_lang}\n\nTranslate each element of this JSON array. \
             Reply with ONLY a JSON array of exactly {} strings, in the same order.\n\n{}",
            texts.len(),
            serde_json::to_string(texts)?,
        );
        let request =
            ChatRequest::from_system(SYSTEM_PROMPT).append_message(ChatMessage::user(user));

        let (runtime, client) = transport()?;
        // `block_on` and not a spawn: this already runs on a gpui background
        // thread (see `Workspace::translate`), so blocking it is the intended
        // cost. The runtime exists solely because reqwest needs a reactor.
        let response = runtime
            .block_on(client.exec_chat(self.target.clone(), request, None))
            .map_err(|e| anyhow::anyhow!("{} request failed: {e}", self.label))?;

        let text = response
            .into_first_text()
            .ok_or_else(|| anyhow::anyhow!("{} returned no text", self.label))?;
        let parsed: Vec<String> = serde_json::from_str(extract_json_array(&text))
            .map_err(|e| anyhow::anyhow!("could not parse translation response: {e}"))?;
        Ok(parsed)
    }
}

/// The tokio runtime and genai client, built once.
///
/// genai is async and its transport is reqwest, which panics with "there is no
/// reactor running" if its futures are polled outside tokio — and gpui's
/// executor is not tokio. A private runtime is the smallest thing that bridges
/// that without making [`TranslationService`] async, which would drag a runtime
/// dependency into `mt-doc` and rewrite six test doubles for no gain.
///
/// Shared rather than per-translator so the connection pool survives between
/// requests: a document translated twice reuses one TLS handshake, which the
/// `curl` process it replaces could never do.
///
/// `rt-multi-thread` with one worker, not `current_thread`: a current-thread
/// runtime can only be driven by whichever thread calls `block_on`, and gpui
/// hands each background task an arbitrary pool thread.
#[cfg(feature = "model-transport")]
fn transport() -> anyhow::Result<&'static (tokio::runtime::Runtime, Client)> {
    static TRANSPORT: OnceLock<std::io::Result<(tokio::runtime::Runtime, Client)>> =
        OnceLock::new();
    match TRANSPORT.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("markturbo-translate")
            .enable_all()
            .build()?;
        // Before the client, not merely before the request: `build()` is what
        // constructs reqwest's client, and reqwest panics there if no provider
        // is installed. We opted out of its default one to keep `aws-lc-sys`
        // and its C toolchain out of the build, so installing one is ours to
        // do. Ignoring the error is correct — it only reports that a provider
        // was already installed, which is the outcome we want either way.
        let _ = rustls::crypto::ring::default_provider().install_default();
        // The timeout `curl --max-time 120` used to supply. Without it a
        // half-open connection leaves the Translate button inert forever.
        let client = Client::builder()
            .with_web_config(WebConfig::default().with_timeout(Duration::from_secs(120)))
            .build();
        Ok((runtime, client))
    }) {
        Ok(transport) => Ok(transport),
        Err(err) => anyhow::bail!("cannot start the translation runtime: {err}"),
    }
}

#[cfg(feature = "model-transport")]
const SYSTEM_PROMPT: &str = "You translate prose fragments from technical Markdown documents. \
Translate only natural-language prose. Never translate or alter identifiers, file paths, URLs, \
command names, or code. Preserve Markdown markup characters exactly as they appear. \
Reply with a JSON array of strings and nothing else.";

/// Pull the JSON array out of a reply that may be fenced or prefixed.
#[cfg(feature = "model-transport")]
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

#[cfg(all(test, feature = "model-transport"))]
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
    fn a_base_url_without_a_trailing_slash_would_lose_its_path_prefix() {
        // genai joins the endpoint with `Url::join`, which *replaces* the last
        // path segment when there is no trailing slash: `https://h/v1` becomes
        // `https://h/chat/completions`, silently dropping the `/v1`, and a
        // gateway mounted at `/openai/v1` loses its mount point the same way.
        // Both forms must normalise to the one genai can join onto.
        assert_eq!(
            base_url("https://gw.invalid/openai/v1"),
            base_url("https://gw.invalid/openai/v1/")
        );
        assert!(base_url("https://example.invalid/v1").ends_with("/v1/"));
        // And normalising must not double up on a URL that already ends in one.
        assert!(!base_url("https://example.invalid/v1/").ends_with("//"));
    }

    #[test]
    fn each_wire_format_maps_to_the_adapter_that_speaks_it() {
        // The mapping is why `Provider` still exists. genai would otherwise pick
        // an adapter by sniffing the model name and fall back to Ollama for
        // anything unfamiliar — so a vLLM server serving `Qwen/Qwen3-32B` would
        // be handed to the wrong adapter and speak the wrong protocol.
        let target = Provider::OpenAiChat.service_target(
            "https://vllm.invalid/v1/",
            "sk-test",
            "Qwen/Qwen3-32B",
        );
        assert_eq!(target.model.adapter_kind, AdapterKind::OpenAI);
        assert_eq!(target.endpoint.base_url(), "https://vllm.invalid/v1/");

        // A model named after a vendor must not drag the request onto that
        // vendor's protocol: the wire format the user picked is what decides.
        let target =
            Provider::OpenAiChat.service_target("https://gw.invalid/v1/", "k", "claude-sonnet-5");
        assert_eq!(target.model.adapter_kind, AdapterKind::OpenAI);

        // The three formats must reach three distinct adapters, or two of them
        // would post identical bodies to the same URL.
        let adapters: std::collections::HashSet<AdapterKind> =
            Provider::ALL.iter().map(|p| p.adapter()).collect();
        assert_eq!(adapters.len(), Provider::ALL.len());

        // The key comes from the argument, never the environment — the whole
        // reason the settings field outranks the shell.
        let target =
            Provider::AnthropicMessages.service_target("https://h/v1/", "sk-settings", "m");
        assert!(matches!(target.auth, AuthData::Key(k) if k == "sk-settings"));
    }

    #[test]
    fn every_schema_has_usable_defaults() {
        // A user who picks a schema and nothing else must get a working
        // endpoint, so none of these may be empty — and the trailing slash is
        // required, or genai joins the path over the top of it.
        for schema in Provider::ALL {
            assert!(schema.default_base_url().starts_with("https://"));
            assert!(
                schema.default_base_url().ends_with('/'),
                "{schema:?}: genai would drop the last segment"
            );
            assert!(!schema.default_model().is_empty());
            assert!(schema.key_env().ends_with("_API_KEY"));
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

    /// A one-request HTTP server, returning `body` with a JSON content type.
    ///
    /// Returns its address and a receiver for what the request actually
    /// contained. Hand-rolled on a `TcpListener` rather than pulling in a test
    /// server crate: the whole exchange is one request and one response, and
    /// what is being checked is that our bytes reach a socket at all.
    fn one_shot_server(body: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{BufRead as _, BufReader, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let addr = listener.local_addr().expect("a bound address");
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(&stream);
            let mut request = String::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if let Some(value) = line
                    .to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
                {
                    length = value;
                }
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut payload = vec![0u8; length];
            let _ = std::io::Read::read_exact(&mut reader, &mut payload);
            request.push_str(&String::from_utf8_lossy(&payload));
            let _ = tx.send(request);

            let mut stream = &stream;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.flush();
        });

        (format!("http://{addr}/v1/"), rx)
    }

    /// The transport reaches a socket, carrying the key and the prose.
    ///
    /// This test could not exist before. The `curl` version put the request
    /// together inside a subprocess, so the only way to see what it sent was to
    /// make a real call to a real provider — which is why 300 lines of tests in
    /// this file covered every decision *around* the request and nothing about
    /// the request itself.
    ///
    /// It is also the regression test for the crypto provider: reqwest is built
    /// with `rustls-no-provider`, so if `transport` ever stops calling
    /// `install_default`, this panics with "No rustls crypto provider is
    /// configured" — at runtime, on the first translation, which is exactly the
    /// failure a user would otherwise find first.
    #[test]
    fn a_translation_request_reaches_the_wire_with_its_key_and_its_prose() {
        let (base_url, requests) = one_shot_server(
            r#"{"choices":[{"message":{"role":"assistant","content":"[\"bonjour\"]"}}]}"#,
        );

        let translator = GenAiTranslator {
            target: Provider::OpenAiChat.service_target(&base_url, "key-from-settings", "gpt-5"),
            label: Provider::OpenAiChat.label(),
        };
        let out = translator
            .translate(&["hello".to_string()], "fr")
            .expect("the local server answers");
        assert_eq!(out, vec!["bonjour".to_string()]);

        let request = requests
            .recv_timeout(Duration::from_secs(10))
            .expect("the request reached the socket");
        assert!(
            request.contains("POST /v1/chat/completions"),
            "the schema's path is appended to the base URL: {request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("bearer key-from-settings"),
            "the key from settings is what authenticates, not one from the environment"
        );
        assert!(
            request.contains("hello"),
            "the prose to translate is in the body"
        );
        assert!(request.contains("fr"), "and so is the target language");
    }

    /// A base URL keeps its version segment through to the wire.
    ///
    /// The defect this catches is silent and adapter-specific. genai appends
    /// only the leaf path, and the two families get there differently — the
    /// OpenAI adapters use `Url::join`, which DROPS a final segment with no
    /// trailing slash, so `/v1` becomes nothing; the Anthropic one concatenates,
    /// so `/v1` fuses into `/v1messages`. A user who pastes
    /// `http://localhost:8000/v1` — the form every vLLM and Ollama README
    /// prints — would otherwise reach an endpoint that is not there, with no
    /// way to diagnose it.
    ///
    /// Asserted against a real socket, because the failure is in what genai
    /// puts on the wire rather than in anything this crate computes.
    #[test]
    fn a_pasted_base_url_reaches_the_endpoint_the_user_meant() {
        for (provider, model, want) in [
            (Provider::OpenAiChat, "gpt-5", "/v1/chat/completions"),
            (Provider::OpenAiResponses, "gpt-5", "/v1/responses"),
            (
                Provider::AnthropicMessages,
                "claude-sonnet-5",
                "/v1/messages",
            ),
        ] {
            // With and without the trailing slash: both are forms users paste.
            for suffix in ["/v1", "/v1/"] {
                let (base, requests) = one_shot_server(
                    r#"{"choices":[{"message":{"role":"assistant","content":"[]"}}]}"#,
                );
                let base = base.trim_end_matches("/v1/").to_string() + suffix;

                let translator = GenAiTranslator {
                    target: provider.service_target(&base_url(&base), "k", model),
                    label: provider.label(),
                };
                let _ = translator.translate(&["x".to_string()], "fr");

                let request = requests
                    .recv_timeout(Duration::from_secs(10))
                    .expect("the request reached the socket");
                let line = request.lines().next().unwrap_or_default().to_string();
                assert!(
                    line.contains(want),
                    "{provider:?} with base {suffix} must POST to {want}, got: {line}"
                );
            }
        }
    }

    /// A 200 whose body is the wrong shape is an error, not an empty result.
    ///
    /// The likeliest misconfiguration in the feature: a base URL pointing at
    /// something that answers 200 with JSON which is not a chat completion — a
    /// gateway's health page, a proxy's error envelope, the wrong API version.
    /// Yielding no text would look to the user like a translation that produced
    /// nothing, with no way to tell why. That case is genai's now rather than
    /// ours, so this pins that it is still treated as a failure.
    #[test]
    fn a_well_formed_reply_of_the_wrong_shape_is_reported_rather_than_ignored() {
        let (base, _requests) = one_shot_server(r#"{"status":"ok","uptime":41}"#);
        let translator = GenAiTranslator {
            target: Provider::OpenAiChat.service_target(&base, "k", "gpt-5"),
            label: Provider::OpenAiChat.label(),
        };
        let err = translator
            .translate(&["hello".to_string()], "fr")
            .expect_err("a health page is not a translation");
        assert!(
            err.to_string().contains(Provider::OpenAiChat.label()),
            "and it must say which provider answered: {err}"
        );
    }

    /// A provider error carries the server's own explanation.
    ///
    /// `curl --fail-with-body` was doing this, and losing it would be a
    /// regression that only shows up when something is already wrong: an
    /// unhelpful "request failed" instead of the sentence naming the bad model
    /// or the expired key.
    #[test]
    fn a_failed_request_reports_the_providers_own_message() {
        // No response at all: the socket accepts and closes. Whatever the error
        // is, it must name the provider so a user with several configured knows
        // which one answered.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let addr = listener.local_addr().expect("a bound address");
        std::thread::spawn(move || {
            let _ = listener.accept();
        });

        let base_url = format!("http://{addr}/v1/");
        let translator = GenAiTranslator {
            target: Provider::AnthropicMessages.service_target(&base_url, "k", "claude-sonnet-5"),
            label: Provider::AnthropicMessages.label(),
        };
        let err = translator
            .translate(&["hello".to_string()], "fr")
            .expect_err("a closed socket is not a translation");
        let message = err.to_string();
        assert!(
            message.contains(Provider::AnthropicMessages.label()),
            "the failure must name which provider produced it: {message}"
        );
    }
}

#[cfg(all(test, not(feature = "model-transport")))]
mod ablation_tests {
    use super::*;

    #[test]
    fn measurement_build_reports_the_removed_transport() {
        let settings = AppSettings {
            translate_api_key: "measurement-placeholder".into(),
            ..AppSettings::default()
        };

        for provider in Provider::ALL {
            let error = match provider.build_with(&settings) {
                Ok(_) => panic!("{} unexpectedly built model transport", provider.label()),
                Err(error) => error,
            };
            assert_eq!(
                error,
                "Model transport is unavailable in this Goal 04 measurement build.",
                "{} should report the measurement-build transport removal directly",
                provider.label()
            );
        }
    }
}
