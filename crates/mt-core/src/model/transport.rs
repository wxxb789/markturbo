use std::sync::LazyLock;
use std::time::Duration;

use genai::adapter::AdapterKind;
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ServiceTarget};

use super::{EndpointIdentity, EndpointLocation, ModelConfig, Provider};

pub(crate) fn provider_adapter(provider: Provider) -> AdapterKind {
    match provider {
        Provider::AnthropicMessages => AdapterKind::Anthropic,
        Provider::OpenAiChat => AdapterKind::OpenAI,
        Provider::OpenAiResponses => AdapterKind::OpenAIResp,
    }
}

pub(crate) fn service_target(config: &ModelConfig, credential: &str) -> ServiceTarget {
    ServiceTarget {
        endpoint: Endpoint::from_owned(config.endpoint().base_url()),
        auth: AuthData::Key(credential.to_owned()),
        model: ModelIden::new(
            provider_adapter(config.provider()),
            config.model().to_owned(),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyPolicy {
    System,
    Disabled,
}

pub(crate) fn proxy_policy(endpoint: &EndpointIdentity) -> ProxyPolicy {
    if endpoint.location() == EndpointLocation::Local {
        ProxyPolicy::Disabled
    } else {
        ProxyPolicy::System
    }
}

pub(crate) fn client_for_endpoint(endpoint: &EndpointIdentity) -> Result<&'static Client, ()> {
    static SYSTEM_CLIENT: LazyLock<Result<Client, ()>> =
        LazyLock::new(|| build_client(ProxyPolicy::System));
    static NO_PROXY_CLIENT: LazyLock<Result<Client, ()>> =
        LazyLock::new(|| build_client(ProxyPolicy::Disabled));

    let client = match proxy_policy(endpoint) {
        ProxyPolicy::System => &*SYSTEM_CLIENT,
        ProxyPolicy::Disabled => &*NO_PROXY_CLIENT,
    };
    client.as_ref().map_err(|_| ())
}

fn build_client(proxy_policy: ProxyPolicy) -> Result<Client, ()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120));
    if proxy_policy == ProxyPolicy::Disabled {
        builder = builder.no_proxy();
    }
    let reqwest = builder.build().map_err(|_| ())?;
    Ok(Client::builder().with_reqwest(reqwest).build())
}

pub(crate) fn request_failure_hint(error: &genai::Error) -> &'static str {
    use genai::webc::Error as WebError;

    let web_error = match error {
        genai::Error::WebAdapterCall { webc_error, .. }
        | genai::Error::WebModelCall { webc_error, .. } => Some(webc_error),
        _ => None,
    };
    match web_error {
        Some(WebError::ResponseFailedStatus { status, .. }) if status.is_redirection() => {
            "the endpoint returned a redirect, which markturbo does not follow; verify the configured base URL."
        }
        Some(WebError::ResponseFailedStatus { status, .. })
            if matches!(status.as_u16(), 401 | 403) =>
        {
            "the endpoint rejected authentication; replace or verify the credential."
        }
        Some(WebError::ResponseFailedStatus { status, .. }) if status.as_u16() == 404 => {
            "the endpoint path was not found; verify the API base path and wire format."
        }
        Some(WebError::ResponseFailedStatus { status, .. }) if status.as_u16() == 429 => {
            "the endpoint rate-limited the request; retry after the provider's stated delay."
        }
        Some(WebError::ResponseFailedStatus { status, .. }) if status.is_server_error() => {
            "the endpoint reported a server failure; retry later or inspect the provider separately."
        }
        Some(WebError::ResponseFailedStatus { .. }) => {
            "the endpoint rejected the request; verify the model, endpoint, and credential."
        }
        Some(WebError::Reqwest(error)) if error.is_timeout() => {
            "the connection timed out; verify endpoint reachability."
        }
        Some(WebError::Reqwest(error)) if error.is_connect() => {
            "the connection could not be established; verify endpoint reachability and proxy settings."
        }
        Some(WebError::ResponseFailedNotJson { .. })
        | Some(WebError::ResponseFailedInvalidJson { .. }) => {
            "the endpoint returned a non-JSON response; verify the API base path and wire format."
        }
        _ => "verify the endpoint, model, credential, and network access.",
    }
}

/// The one shared async runtime. HTTP clients are cached separately by proxy
/// policy so loopback no-proxy behavior cannot bleed into remote traffic.
pub(crate) fn runtime() -> std::io::Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: LazyLock<std::io::Result<tokio::runtime::Runtime>> = LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("markturbo-model")
            .enable_all()
            .build()
    });
    match &*RUNTIME {
        Ok(runtime) => Ok(runtime),
        Err(error) => Err(std::io::Error::new(
            error.kind(),
            "model runtime unavailable",
        )),
    }
}
