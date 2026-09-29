use std::fmt;

use sha2::{Digest as _, Sha256};
use url::{Host, Url};

use super::{
    CredentialTarget, EndpointIdentity, EndpointIdentityError, EndpointLocation, EndpointScheme,
    MODEL_CREDENTIAL_APPLICATION, ModelConfig, Provider, ProxyDisclosure, TransportDisclosure,
    TransportEncryption,
};

impl Provider {
    pub const fn default_base_url(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "https://api.anthropic.com/v1/",
            Provider::OpenAiChat | Provider::OpenAiResponses => "https://api.openai.com/v1/",
        }
    }

    pub const fn default_model(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "claude-sonnet-5",
            Provider::OpenAiChat | Provider::OpenAiResponses => "gpt-5",
        }
    }
}

impl ModelConfig {
    pub fn new(
        provider: Provider,
        model: Option<&str>,
        base_url: Option<&str>,
    ) -> Result<Self, EndpointIdentityError> {
        Ok(Self {
            provider,
            model: model
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .unwrap_or_else(|| provider.default_model())
                .to_owned(),
            endpoint: EndpointIdentity::parse(provider, base_url)?,
        })
    }

    pub const fn provider(&self) -> Provider {
        self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn endpoint(&self) -> &EndpointIdentity {
        &self.endpoint
    }
}

impl EndpointScheme {
    pub const fn as_str(self) -> &'static str {
        match self {
            EndpointScheme::Http => "http",
            EndpointScheme::Https => "https",
        }
    }

    const fn default_port(self) -> u16 {
        match self {
            EndpointScheme::Http => 80,
            EndpointScheme::Https => 443,
        }
    }
}

impl fmt::Display for EndpointScheme {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TransportDisclosure {
    const HTTPS: Self = Self {
        encryption: TransportEncryption::Encrypted,
        proxy: ProxyDisclosure::MayUseConfiguredProxy,
    };

    const LOCAL_HTTPS: Self = Self {
        encryption: TransportEncryption::Encrypted,
        proxy: ProxyDisclosure::Disabled,
    };

    const LOOPBACK_HTTP: Self = Self {
        encryption: TransportEncryption::Unencrypted,
        proxy: ProxyDisclosure::Disabled,
    };

    pub const fn encryption(self) -> TransportEncryption {
        self.encryption
    }

    pub const fn proxy(self) -> ProxyDisclosure {
        self.proxy
    }

    pub const fn is_encrypted(self) -> bool {
        matches!(self.encryption, TransportEncryption::Encrypted)
    }

    pub const fn uses_proxy(self) -> bool {
        matches!(self.proxy, ProxyDisclosure::MayUseConfiguredProxy)
    }
}

impl EndpointIdentity {
    /// Parse a custom base URL, or the provider default when absent or blank.
    pub fn parse(provider: Provider, raw: Option<&str>) -> Result<Self, EndpointIdentityError> {
        let candidate = raw
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
            .unwrap_or(provider.default_base_url());
        let mut parsed = Url::parse(candidate).map_err(EndpointIdentityError::InvalidUrl)?;

        if has_explicit_userinfo(candidate)
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(EndpointIdentityError::UserInfoNotAllowed);
        }
        if parsed.query().is_some() {
            return Err(EndpointIdentityError::QueryNotAllowed);
        }
        if parsed.fragment().is_some() {
            return Err(EndpointIdentityError::FragmentNotAllowed);
        }

        let scheme = match parsed.scheme() {
            "http" => EndpointScheme::Http,
            "https" => EndpointScheme::Https,
            _ => return Err(EndpointIdentityError::UnsupportedScheme),
        };
        let (host, loopback) = match parsed.host() {
            Some(Host::Domain(host)) => (host.to_owned(), false),
            Some(Host::Ipv4(host)) => (host.to_string(), host.is_loopback()),
            Some(Host::Ipv6(host)) => (host.to_string(), host.is_loopback()),
            None => return Err(EndpointIdentityError::MissingHost),
        };

        if scheme == EndpointScheme::Http && !loopback {
            return Err(EndpointIdentityError::InsecureRemoteTransport);
        }

        let effective_port = parsed
            .port_or_known_default()
            .expect("http and https always have a known default port");
        if effective_port == scheme.default_port() {
            parsed
                .set_port(None)
                .expect("an http or https URL can always clear its port");
        }

        let mut api_base_path = parsed.path().to_owned();
        if !api_base_path.ends_with('/') {
            api_base_path.push('/');
            parsed.set_path(&api_base_path);
            api_base_path = parsed.path().to_owned();
        }

        let location = if loopback {
            EndpointLocation::Local
        } else {
            EndpointLocation::Remote
        };
        let transport = match (scheme, location) {
            (EndpointScheme::Http, EndpointLocation::Local) => TransportDisclosure::LOOPBACK_HTTP,
            (EndpointScheme::Https, EndpointLocation::Local) => TransportDisclosure::LOCAL_HTTPS,
            (EndpointScheme::Https, EndpointLocation::Remote) => TransportDisclosure::HTTPS,
            (EndpointScheme::Http, EndpointLocation::Remote) => {
                unreachable!("remote http endpoints were rejected above")
            }
        };

        Ok(Self {
            application: MODEL_CREDENTIAL_APPLICATION,
            provider,
            scheme,
            host,
            effective_port,
            api_base_path,
            base_url: parsed.to_string(),
            location,
            transport,
        })
    }

    pub const fn application(&self) -> &'static str {
        self.application
    }

    pub const fn provider(&self) -> Provider {
        self.provider
    }

    pub const fn scheme(&self) -> EndpointScheme {
        self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn effective_port(&self) -> u16 {
        self.effective_port
    }

    pub fn api_base_path(&self) -> &str {
        &self.api_base_path
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Canonical endpoint identity with the effective port made explicit.
    pub fn normalized_identity(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!(
            "{}://{}:{}{}",
            self.scheme, host, self.effective_port, self.api_base_path
        )
    }

    pub const fn location(&self) -> EndpointLocation {
        self.location
    }

    pub const fn transport(&self) -> TransportDisclosure {
        self.transport
    }

    /// Whether this identity may use the provider's ambient vendor credential.
    pub fn is_vendor_default(&self) -> bool {
        Self::parse(self.provider, None).is_ok_and(|default| default == *self)
    }

    /// Stable, non-secret Windows Credential Manager target identity.
    pub fn credential_target(&self) -> CredentialTarget {
        let mut identity = Sha256::new();
        let port = self.effective_port.to_string();
        for component in [
            self.application,
            self.provider.key(),
            self.scheme.as_str(),
            self.host.as_str(),
            port.as_str(),
            self.api_base_path.as_str(),
        ] {
            identity.update(component.as_bytes());
            identity.update([0]);
        }
        let identity = identity.finalize();
        CredentialTarget(format!(
            "{}:model-credential:v2|wire={}|host={}|identity-sha256={identity:x}",
            self.application,
            self.provider.key(),
            self.host,
        ))
    }
}

fn has_explicit_userinfo(raw: &str) -> bool {
    let Some((_, after_scheme)) = raw.split_once("://") else {
        return false;
    };
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    after_scheme[..authority_end].contains('@')
}

pub(super) fn endpoint_input_is_safe_to_persist(raw: &str) -> bool {
    raw.trim().is_empty() || EndpointIdentity::parse(Provider::OpenAiChat, Some(raw)).is_ok()
}

impl fmt::Display for EndpointIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EndpointIdentityError::InvalidUrl(error) => {
                write!(formatter, "invalid endpoint URL: {error}")
            }
            EndpointIdentityError::UnsupportedScheme => {
                formatter.write_str("model endpoints must use http or https")
            }
            EndpointIdentityError::MissingHost => {
                formatter.write_str("model endpoint is missing a host")
            }
            EndpointIdentityError::UserInfoNotAllowed => {
                formatter.write_str("model endpoint userinfo is not allowed")
            }
            EndpointIdentityError::QueryNotAllowed => {
                formatter.write_str("model endpoint query parameters are not allowed")
            }
            EndpointIdentityError::FragmentNotAllowed => {
                formatter.write_str("model endpoint fragments are not allowed")
            }
            EndpointIdentityError::InsecureRemoteTransport => {
                formatter.write_str("remote model endpoints must use https")
            }
        }
    }
}

impl std::error::Error for EndpointIdentityError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(provider: Provider, raw: &str) -> EndpointIdentity {
        EndpointIdentity::parse(provider, Some(raw)).unwrap()
    }

    #[test]
    fn endpoint_normalization_collapses_equivalent_urls() {
        let normalized = endpoint(
            Provider::OpenAiResponses,
            "HTTPS://API.OPENAI.COM:443/a/../v1",
        );
        let default = EndpointIdentity::parse(Provider::OpenAiResponses, None).unwrap();

        assert_eq!(normalized, default);
        assert_eq!(normalized.base_url(), "https://api.openai.com/v1/");
        assert_eq!(normalized.host(), "api.openai.com");
        assert_eq!(normalized.effective_port(), 443);
        assert_eq!(normalized.api_base_path(), "/v1/");
        assert_eq!(
            normalized.normalized_identity(),
            "https://api.openai.com:443/v1/"
        );
        assert_eq!(
            EndpointIdentity::parse(Provider::OpenAiResponses, Some("   ")).unwrap(),
            default
        );
    }

    #[test]
    fn port_path_and_wire_format_are_identity_boundaries() {
        let chat = endpoint(Provider::OpenAiChat, "https://example.com/v1");
        let responses = endpoint(Provider::OpenAiResponses, "https://example.com/v1/");
        let other_port = endpoint(Provider::OpenAiChat, "https://example.com:8443/v1/");
        let other_path = endpoint(Provider::OpenAiChat, "https://example.com/api/");

        assert_ne!(chat, responses);
        assert_ne!(chat, other_port);
        assert_ne!(chat, other_path);

        let target = other_port.credential_target().to_string();
        for component in [
            "markturbo",
            "model-credential:v2",
            "wire=openai-chat",
            "host=example.com",
            "identity-sha256=",
        ] {
            assert!(
                target.contains(component),
                "missing {component} in {target}"
            );
        }
        assert_eq!(target, target.to_ascii_lowercase());
    }

    #[test]
    fn credential_targets_do_not_collide_under_windows_case_insensitive_matching() {
        let upper = endpoint(Provider::OpenAiChat, "https://example.com/Foo/");
        let lower = endpoint(Provider::OpenAiChat, "https://example.com/foo/");

        assert_ne!(upper, lower);
        assert_ne!(
            upper.credential_target().as_str().to_ascii_lowercase(),
            lower.credential_target().as_str().to_ascii_lowercase()
        );
    }

    #[test]
    fn invalid_endpoint_matrix_fails_closed_without_echoing_input() {
        let cases = [
            (
                "not a url",
                EndpointIdentityError::InvalidUrl(url::ParseError::RelativeUrlWithoutBase),
            ),
            (
                "ftp://example.com/v1/",
                EndpointIdentityError::UnsupportedScheme,
            ),
            (
                "https://user:sentinel-secret@example.com/v1/",
                EndpointIdentityError::UserInfoNotAllowed,
            ),
            (
                "https://@example.com/v1/",
                EndpointIdentityError::UserInfoNotAllowed,
            ),
            (
                "https://example.com/v1/?mode=test",
                EndpointIdentityError::QueryNotAllowed,
            ),
            (
                "https://example.com/v1/#part",
                EndpointIdentityError::FragmentNotAllowed,
            ),
            (
                "http://example.com/v1/",
                EndpointIdentityError::InsecureRemoteTransport,
            ),
            (
                "http://localhost/v1/",
                EndpointIdentityError::InsecureRemoteTransport,
            ),
            (
                "http://0.0.0.0/v1/",
                EndpointIdentityError::InsecureRemoteTransport,
            ),
            (
                "http://[::]/v1/",
                EndpointIdentityError::InsecureRemoteTransport,
            ),
        ];

        for (raw, expected) in cases {
            let error = EndpointIdentity::parse(Provider::OpenAiChat, Some(raw)).unwrap_err();
            assert_eq!(
                std::mem::discriminant(&error),
                std::mem::discriminant(&expected)
            );
            assert!(!format!("{error:?} {error}").contains("sentinel-secret"));
        }
    }

    #[test]
    fn credential_shaped_endpoint_input_is_never_safe_to_persist() {
        for raw in [
            "https://user:sentinel-secret@example.com/v1/",
            "https://example.com/v1/?api_key=sentinel-secret",
            "https://example.com/v1/#sentinel-secret",
            "user:sentinel-secret@example.com",
        ] {
            assert!(!endpoint_input_is_safe_to_persist(raw), "accepted {raw:?}");
        }
        for raw in [
            "",
            "https://example.com/v1/",
            "https://example.com/path@version",
            "http://127.0.0.1:8080/v1/",
        ] {
            assert!(endpoint_input_is_safe_to_persist(raw), "rejected {raw:?}");
        }
    }

    #[test]
    fn loopback_http_is_local_unencrypted_and_proxy_free() {
        for raw in [
            "http://127.0.0.1:8080/v1/",
            "http://127.1/v1/",
            "http://[::1]/v1/",
        ] {
            let endpoint = endpoint(Provider::OpenAiChat, raw);
            assert_eq!(endpoint.location(), EndpointLocation::Local);
            assert_eq!(
                endpoint.transport().encryption(),
                TransportEncryption::Unencrypted
            );
            assert_eq!(endpoint.transport().proxy(), ProxyDisclosure::Disabled);
            assert!(!endpoint.transport().is_encrypted());
            assert!(!endpoint.transport().uses_proxy());
        }

        let secure_local = endpoint(Provider::OpenAiChat, "https://127.0.0.1/v1/");
        assert_eq!(secure_local.location(), EndpointLocation::Local);
        assert!(secure_local.transport().is_encrypted());
        assert_eq!(secure_local.transport().proxy(), ProxyDisclosure::Disabled);
        assert!(!secure_local.transport().uses_proxy());

        let remote = endpoint(Provider::OpenAiChat, "https://example.com/v1/");
        assert_eq!(remote.location(), EndpointLocation::Remote);
        assert_eq!(
            remote.transport().proxy(),
            ProxyDisclosure::MayUseConfiguredProxy
        );
    }

    #[test]
    fn localhost_requires_https_and_remains_proxy_eligible() {
        let error =
            EndpointIdentity::parse(Provider::OpenAiChat, Some("http://localhost:8080/v1/"))
                .unwrap_err();
        assert!(matches!(
            error,
            EndpointIdentityError::InsecureRemoteTransport
        ));

        let endpoint = endpoint(Provider::OpenAiChat, "https://localhost:8443/v1/");
        assert_eq!(endpoint.location(), EndpointLocation::Remote);
        assert!(endpoint.transport().is_encrypted());
        assert_eq!(
            endpoint.transport().proxy(),
            ProxyDisclosure::MayUseConfiguredProxy
        );
        assert!(endpoint.transport().uses_proxy());
    }

    #[test]
    fn only_the_exact_vendor_default_accepts_ambient_vendor_credentials() {
        for provider in Provider::ALL {
            assert!(
                EndpointIdentity::parse(provider, None)
                    .unwrap()
                    .is_vendor_default()
            );
        }
        assert!(
            !endpoint(Provider::OpenAiChat, "https://api.openai.com:8443/v1/").is_vendor_default()
        );
        assert!(
            !endpoint(Provider::OpenAiResponses, "https://api.openai.com/custom/")
                .is_vendor_default()
        );
        assert!(
            !endpoint(Provider::AnthropicMessages, "https://proxy.example/v1/").is_vendor_default()
        );
    }

    #[test]
    fn model_config_uses_non_secret_provider_defaults() {
        let config = ModelConfig::new(Provider::OpenAiResponses, Some("  "), None).unwrap();
        assert_eq!(config.provider(), Provider::OpenAiResponses);
        assert_eq!(config.model(), "gpt-5");
        assert!(config.endpoint().is_vendor_default());
        assert_eq!(
            Provider::OpenAiResponses.credential_environment_variable(),
            "OPENAI_API_KEY"
        );
    }
}
