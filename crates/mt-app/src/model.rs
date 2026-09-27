//! Reusable model-provider and outbound-request privacy contracts.
//!
//! This module owns only non-secret configuration and request authorization.
//! Credential storage, filesystem packaging, provider transport, and Review
//! semantics remain separate boundaries.

use std::fmt;
use std::sync::Arc;

use sha2::{Digest as _, Sha256};
use url::{Host, Url};

pub const MODEL_CREDENTIAL_APPLICATION: &str = "io.github.wxxb789.markturbo";

/// A provider wire format, independent of the endpoint vendor or model name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    AnthropicMessages,
    OpenAiChat,
    OpenAiResponses,
}

impl Provider {
    pub const ALL: [Provider; 3] = [
        Provider::AnthropicMessages,
        Provider::OpenAiChat,
        Provider::OpenAiResponses,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "anthropic",
            Provider::OpenAiChat => "openai-chat",
            Provider::OpenAiResponses => "openai-responses",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|provider| provider.key() == key)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "Anthropic Messages",
            Provider::OpenAiChat => "OpenAI Chat Completions",
            Provider::OpenAiResponses => "OpenAI Responses",
        }
    }

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

    pub const fn credential_environment_variable(self) -> &'static str {
        match self {
            Provider::AnthropicMessages => "ANTHROPIC_API_KEY",
            Provider::OpenAiChat | Provider::OpenAiResponses => "OPENAI_API_KEY",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Reusable, non-secret model configuration shared by model operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelConfig {
    provider: Provider,
    model: String,
    endpoint: EndpointIdentity,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointScheme {
    Http,
    Https,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointLocation {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportEncryption {
    Encrypted,
    Unencrypted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProxyDisclosure {
    MayUseConfiguredProxy,
    Disabled,
}

/// User-visible transport facts derived from the validated endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransportDisclosure {
    encryption: TransportEncryption,
    proxy: ProxyDisclosure,
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

/// Canonical endpoint identity used by disclosure, routing, and credentials.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointIdentity {
    application: &'static str,
    provider: Provider,
    scheme: EndpointScheme,
    host: String,
    effective_port: u16,
    api_base_path: String,
    base_url: String,
    location: EndpointLocation,
    transport: TransportDisclosure,
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

/// Whether an endpoint value is complete and safe to persist.
pub fn endpoint_input_is_safe_to_persist(raw: &str) -> bool {
    raw.trim().is_empty() || EndpointIdentity::parse(Provider::OpenAiChat, Some(raw)).is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointIdentityError {
    InvalidUrl(url::ParseError),
    UnsupportedScheme,
    MissingHost,
    UserInfoNotAllowed,
    QueryNotAllowed,
    FragmentNotAllowed,
    InsecureRemoteTransport,
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

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CredentialTarget(String);

impl CredentialTarget {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CredentialTarget")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for CredentialTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelOperation {
    Review,
    Revision,
    Translation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboundScopeKind {
    Selection,
    Block,
    Document,
    DocumentWithEffectiveAgentContext,
    AgentSkillPackage,
}

/// Maximum source bytes accepted from one Agent Skill package file.
///
/// These limits apply to source bytes before provider framing. They are kept
/// here, next to the immutable outbound package model, so callers cannot
/// accidentally implement a second, weaker limit in a UI or transport layer.
pub const AGENT_SKILL_MAX_FILE_BYTES: usize = 512 * 1024;
pub const AGENT_SKILL_MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;

const AGENT_SKILL_SHA256_BYTES: usize = 32;
const AGENT_SKILL_FRAME_LENGTH_BYTES: usize = std::mem::size_of::<u64>();

#[derive(Debug, Clone, PartialEq, Eq)]
enum OutboundScopeDetails {
    Selection {
        byte_size: u64,
    },
    Block {
        byte_size: u64,
    },
    Document {
        byte_size: u64,
    },
    DocumentWithEffectiveAgentContext {
        document_byte_size: u64,
        sources: Vec<String>,
    },
    AgentSkillPackage {
        inventory: AgentSkillInventory,
    },
}

#[derive(Clone)]
struct ScopeBinding(Arc<()>);

impl PartialEq for ScopeBinding {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ScopeBinding {}

/// Exact displayed content scope for one disclosure/request flow.
#[derive(Clone, PartialEq, Eq)]
pub struct OutboundScope {
    binding: ScopeBinding,
    details: OutboundScopeDetails,
}

impl OutboundScope {
    pub fn selection(byte_size: u64) -> Self {
        Self::new(OutboundScopeDetails::Selection { byte_size })
    }

    pub fn document(byte_size: u64) -> Self {
        Self::new(OutboundScopeDetails::Document { byte_size })
    }

    pub fn block(byte_size: u64) -> Self {
        Self::new(OutboundScopeDetails::Block { byte_size })
    }

    pub fn document_with_effective_agent_context<I, S>(
        document_byte_size: u64,
        sources: I,
    ) -> Result<Self, OutboundScopeError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut sources = sources
            .into_iter()
            .map(Into::into)
            .map(|source: String| source.trim().to_owned())
            .collect::<Vec<_>>();
        if sources.iter().any(String::is_empty) {
            return Err(OutboundScopeError::EmptyEffectiveContextSource);
        }
        sources.sort();
        sources.dedup();
        if sources.is_empty() {
            return Err(OutboundScopeError::MissingEffectiveContextSources);
        }
        Ok(Self::new(
            OutboundScopeDetails::DocumentWithEffectiveAgentContext {
                document_byte_size,
                sources,
            },
        ))
    }

    fn agent_skill_package(inventory: AgentSkillInventory) -> Self {
        Self::new(OutboundScopeDetails::AgentSkillPackage { inventory })
    }

    fn new(details: OutboundScopeDetails) -> Self {
        Self {
            binding: ScopeBinding(Arc::new(())),
            details,
        }
    }

    pub const fn kind(&self) -> OutboundScopeKind {
        match self.details {
            OutboundScopeDetails::Selection { .. } => OutboundScopeKind::Selection,
            OutboundScopeDetails::Block { .. } => OutboundScopeKind::Block,
            OutboundScopeDetails::Document { .. } => OutboundScopeKind::Document,
            OutboundScopeDetails::DocumentWithEffectiveAgentContext { .. } => {
                OutboundScopeKind::DocumentWithEffectiveAgentContext
            }
            OutboundScopeDetails::AgentSkillPackage { .. } => OutboundScopeKind::AgentSkillPackage,
        }
    }

    pub const fn primary_content_byte_size(&self) -> u64 {
        match &self.details {
            OutboundScopeDetails::Selection { byte_size }
            | OutboundScopeDetails::Block { byte_size }
            | OutboundScopeDetails::Document { byte_size } => *byte_size,
            OutboundScopeDetails::DocumentWithEffectiveAgentContext {
                document_byte_size, ..
            } => *document_byte_size,
            OutboundScopeDetails::AgentSkillPackage { inventory } => inventory.total_byte_size(),
        }
    }

    pub fn effective_context_sources(&self) -> &[String] {
        match &self.details {
            OutboundScopeDetails::DocumentWithEffectiveAgentContext { sources, .. } => sources,
            _ => &[],
        }
    }

    pub fn agent_skill_inventory(&self) -> Option<&AgentSkillInventory> {
        match &self.details {
            OutboundScopeDetails::AgentSkillPackage { inventory } => Some(inventory),
            _ => None,
        }
    }

    /// Goal 06 Review is document-only. Effective Agent Context is introduced
    /// by Goal 08 and may remain a valid scope for other operations.
    pub const fn permits_review(&self) -> bool {
        !matches!(
            self.details,
            OutboundScopeDetails::DocumentWithEffectiveAgentContext { .. }
        )
    }

    /// Goal 07 Revision may disclose only a document, selection, or frozen
    /// Agent Skill package. Blocks and Effective Agent Context are separate
    /// flows and cannot inherit Revision consent.
    pub const fn permits_revision(&self) -> bool {
        matches!(
            self.details,
            OutboundScopeDetails::Selection { .. }
                | OutboundScopeDetails::Document { .. }
                | OutboundScopeDetails::AgentSkillPackage { .. }
        )
    }
}

impl fmt::Debug for OutboundScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutboundScope")
            .field("details", &self.details)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundScopeError {
    MissingEffectiveContextSources,
    EmptyEffectiveContextSource,
}

impl fmt::Display for OutboundScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutboundScopeError::MissingEffectiveContextSources => {
                formatter.write_str("effective-context scope must name at least one source")
            }
            OutboundScopeError::EmptyEffectiveContextSource => {
                formatter.write_str("effective-context source names cannot be empty")
            }
        }
    }
}

impl std::error::Error for OutboundScopeError {}

/// How an Agent Skill file is represented at the provider boundary.
///
/// Invalid UTF-8 is metadata-only by default. A caller that explicitly chose
/// to disclose those bytes may use `ExplicitRaw`; that distinction is retained
/// in the frozen request so an adapter cannot mistake an omitted payload for an
/// empty file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentSkillContentKind {
    Utf8Text,
    MetadataOnly,
    ExplicitRaw,
}

impl AgentSkillContentKind {
    pub const fn includes_raw_content(self) -> bool {
        matches!(self, Self::Utf8Text | Self::ExplicitRaw)
    }

    pub const fn is_metadata_only(self) -> bool {
        matches!(self, Self::MetadataOnly)
    }
}

/// One file disclosed as part of an outbound Agent Skill package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSkillFile {
    normalized_relative_path: String,
    byte_size: u64,
    sha256: [u8; AGENT_SKILL_SHA256_BYTES],
    inclusion_reason: String,
}

impl AgentSkillFile {
    fn new_with_sha256(
        relative_path: impl AsRef<str>,
        byte_size: u64,
        sha256: [u8; AGENT_SKILL_SHA256_BYTES],
        inclusion_reason: impl Into<String>,
    ) -> Result<Self, AgentSkillInventoryError> {
        let inclusion_reason = inclusion_reason.into().trim().to_owned();
        if inclusion_reason.is_empty() {
            return Err(AgentSkillInventoryError::EmptyInclusionReason);
        }
        validate_file_size(byte_size)?;
        Ok(Self {
            normalized_relative_path: normalize_relative_path(relative_path.as_ref())?,
            byte_size,
            sha256,
            inclusion_reason,
        })
    }

    pub fn normalized_relative_path(&self) -> &str {
        &self.normalized_relative_path
    }

    /// The canonical UTF-8 bytes used for deterministic package ordering.
    pub fn normalized_relative_path_bytes(&self) -> &[u8] {
        self.normalized_relative_path.as_bytes()
    }

    pub const fn byte_size(&self) -> u64 {
        self.byte_size
    }

    /// SHA-256 of the complete source bytes, even when raw content is omitted.
    pub const fn sha256(&self) -> &[u8; AGENT_SKILL_SHA256_BYTES] {
        &self.sha256
    }

    pub fn sha256_hex(&self) -> String {
        self.sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub fn inclusion_reason(&self) -> &str {
        &self.inclusion_reason
    }
}

/// The exact file inventory disclosed for an Agent Skill request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSkillInventory {
    files: Vec<AgentSkillFile>,
    omissions: Vec<AgentSkillOmission>,
    total_byte_size: u64,
}

impl AgentSkillInventory {
    fn new(
        mut files: Vec<AgentSkillFile>,
        mut omissions: Vec<AgentSkillOmission>,
    ) -> Result<Self, AgentSkillInventoryError> {
        files.sort_by(compare_normalized_paths);
        omissions.sort_by(compare_normalized_paths);
        if files
            .windows(2)
            .any(|files| files[0].normalized_relative_path == files[1].normalized_relative_path)
            || omissions.windows(2).any(|omissions| {
                omissions[0].normalized_relative_path == omissions[1].normalized_relative_path
            })
            || files.iter().any(|file| {
                omissions
                    .binary_search_by(|omission| compare_normalized_paths(omission, file))
                    .is_ok()
            })
        {
            return Err(AgentSkillInventoryError::DuplicatePath);
        }
        let total_byte_size = files.iter().try_fold(0_u64, |total, file| {
            total
                .checked_add(file.byte_size)
                .ok_or(AgentSkillInventoryError::ByteSizeOverflow)
        })?;
        validate_total_size(total_byte_size)?;
        Ok(Self {
            files,
            omissions,
            total_byte_size,
        })
    }

    pub fn files(&self) -> &[AgentSkillFile] {
        &self.files
    }

    pub const fn total_byte_size(&self) -> u64 {
        self.total_byte_size
    }

    pub fn omissions(&self) -> &[AgentSkillOmission] {
        &self.omissions
    }

    pub const fn is_partial(&self) -> bool {
        !self.omissions.is_empty()
    }

    /// Whether the supplied immutable payload exposes exactly this inventory.
    ///
    /// Omitted files are intentionally absent from the payload; included files
    /// must match path, byte size, digest, and disclosure reason in the same
    /// deterministic order.
    pub fn matches_payload(&self, entries: &[AgentSkillRequestEntry]) -> bool {
        self.files.len() == entries.len()
            && self
                .files
                .iter()
                .zip(entries)
                .all(|(file, entry)| file == entry.file())
    }
}

/// One normally in-scope supporting file deliberately omitted from a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSkillOmission {
    normalized_relative_path: String,
    reason: String,
}

impl AgentSkillOmission {
    pub fn new(
        relative_path: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<Self, AgentSkillInventoryError> {
        let reason = reason.into().trim().to_owned();
        if reason.is_empty() {
            return Err(AgentSkillInventoryError::EmptyOmissionReason);
        }
        Ok(Self {
            normalized_relative_path: normalize_relative_path(relative_path.as_ref())?,
            reason,
        })
    }

    pub fn normalized_relative_path(&self) -> &str {
        &self.normalized_relative_path
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// One immutable payload entry and the disclosure metadata derived from it.
pub struct AgentSkillRequestEntry {
    file: AgentSkillFile,
    bytes: Option<Box<[u8]>>,
    content_kind: AgentSkillContentKind,
}

impl AgentSkillRequestEntry {
    /// Construct an entry with explicitly selected raw content.
    ///
    /// This is the original Goal 05A constructor and remains an explicit raw
    /// disclosure path for compatibility. New filesystem callers should use
    /// [`Self::from_source_bytes`], which omits invalid UTF-8 by default.
    pub fn new(
        relative_path: impl AsRef<str>,
        inclusion_reason: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<Self, AgentSkillInventoryError> {
        Self::from_bytes_with_kind(
            relative_path,
            inclusion_reason,
            bytes.into(),
            AgentSkillContentKind::ExplicitRaw,
        )
    }

    /// Build a package entry from source bytes using the Goal 06 disclosure
    /// policy: valid UTF-8 is sent, while binary/non-UTF-8 bytes become
    /// metadata-only unless the caller explicitly uses [`Self::new`].
    pub fn from_source_bytes(
        relative_path: impl AsRef<str>,
        inclusion_reason: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<Self, AgentSkillInventoryError> {
        let bytes = bytes.into();
        let kind = if std::str::from_utf8(&bytes).is_ok() {
            AgentSkillContentKind::Utf8Text
        } else {
            AgentSkillContentKind::MetadataOnly
        };
        Self::from_bytes_with_kind(relative_path, inclusion_reason, bytes, kind)
    }

    /// Construct metadata without retaining or disclosing the raw source.
    pub fn metadata_only(
        relative_path: impl AsRef<str>,
        inclusion_reason: impl Into<String>,
        byte_size: u64,
        sha256: [u8; AGENT_SKILL_SHA256_BYTES],
    ) -> Result<Self, AgentSkillInventoryError> {
        Ok(Self {
            file: AgentSkillFile::new_with_sha256(
                relative_path,
                byte_size,
                sha256,
                inclusion_reason,
            )?,
            bytes: None,
            content_kind: AgentSkillContentKind::MetadataOnly,
        })
    }

    fn from_bytes_with_kind(
        relative_path: impl AsRef<str>,
        inclusion_reason: impl Into<String>,
        bytes: Vec<u8>,
        content_kind: AgentSkillContentKind,
    ) -> Result<Self, AgentSkillInventoryError> {
        let byte_size =
            u64::try_from(bytes.len()).map_err(|_| AgentSkillInventoryError::ByteSizeOverflow)?;
        let sha256 = sha256_digest(&bytes);
        let file =
            AgentSkillFile::new_with_sha256(relative_path, byte_size, sha256, inclusion_reason)?;
        let bytes = if content_kind.includes_raw_content() {
            Some(bytes.into_boxed_slice())
        } else {
            None
        };
        Ok(Self {
            file,
            bytes,
            content_kind,
        })
    }

    pub fn file(&self) -> &AgentSkillFile {
        &self.file
    }

    pub const fn content_kind(&self) -> AgentSkillContentKind {
        self.content_kind
    }

    pub const fn is_metadata_only(&self) -> bool {
        self.content_kind.is_metadata_only()
    }

    pub const fn sha256(&self) -> &[u8; AGENT_SKILL_SHA256_BYTES] {
        self.file.sha256()
    }

    pub fn sha256_hex(&self) -> String {
        self.file.sha256_hex()
    }

    /// The raw content exposed to an adapter, if explicitly included.
    pub fn payload_bytes(&self) -> Option<&[u8]> {
        self.bytes.as_deref()
    }

    /// Compatibility view for the pre-Review raw payload API.
    /// Metadata-only entries intentionally return an empty slice; callers that
    /// must distinguish omission from an empty file use [`Self::payload_bytes`].
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_deref().unwrap_or_default()
    }

    /// A deterministic frame that covers every disclosed inventory entry.
    /// Metadata-only binaries contribute their path, byte size, and SHA-256
    /// without ever retaining or exposing their raw source bytes.
    pub fn length_delimited_frame(&self) -> Vec<u8> {
        match self.payload_bytes() {
            Some(bytes) => {
                encode_length_delimited_frame(&self.file.normalized_relative_path, bytes)
                    .expect("the entry path was normalized during construction")
            }
            None => encode_length_delimited_frame(
                &self.file.normalized_relative_path,
                &mt_doc::review::metadata_only_source_content(
                    self.file.byte_size(),
                    &self.file.sha256_hex(),
                ),
            )
            .expect("the entry path was normalized during construction"),
        }
    }
}

/// The only Agent Skill payload view exposed to a provider adapter.
pub struct AgentSkillProviderRequest<'a> {
    entries: &'a [AgentSkillRequestEntry],
}

impl AgentSkillProviderRequest<'_> {
    pub fn entries(&self) -> &[AgentSkillRequestEntry] {
        self.entries
    }

    pub fn framed_payload(&self) -> Vec<u8> {
        self.entries
            .iter()
            .flat_map(AgentSkillRequestEntry::length_delimited_frame)
            .collect()
    }

    pub fn raw_content_bytes(&self) -> u64 {
        self.entries
            .iter()
            .filter_map(AgentSkillRequestEntry::payload_bytes)
            .map(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .fold(0, u64::saturating_add)
    }
}

/// Evidence that an immutable package inventory and the provider payload have
/// the same included files. The proof carries counts and raw-byte accounting so
/// request-inspection tests can report what crossed the adapter boundary
/// without exposing source text in diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentSkillPayloadProof {
    inventory_file_count: usize,
    payload_entry_count: usize,
    metadata_only_entry_count: usize,
    raw_content_bytes: u64,
    framed_payload_bytes: u64,
    exact: bool,
}

impl AgentSkillPayloadProof {
    pub const fn inventory_file_count(self) -> usize {
        self.inventory_file_count
    }

    pub const fn payload_entry_count(self) -> usize {
        self.payload_entry_count
    }

    pub const fn metadata_only_entry_count(self) -> usize {
        self.metadata_only_entry_count
    }

    pub const fn raw_content_bytes(self) -> u64 {
        self.raw_content_bytes
    }

    pub const fn framed_payload_bytes(self) -> u64 {
        self.framed_payload_bytes
    }

    pub const fn is_exact(self) -> bool {
        self.exact
    }
}

/// Provider boundary for an already frozen Agent Skill package.
pub trait AgentSkillProviderAdapter {
    type Error;

    fn endpoint(&self) -> &EndpointIdentity;
    fn send(&mut self, request: AgentSkillProviderRequest<'_>) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum AgentSkillSendError<E> {
    AuthorizationMismatch,
    Adapter(E),
}

impl fmt::Debug for AgentSkillRequestEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentSkillRequestEntry")
            .field("file", &self.file)
            .field("payload", &"<redacted>")
            .finish()
    }
}

/// Exact Agent Skill payload paired with its non-divergent disclosure scope.
pub struct AgentSkillRequest {
    entries: Vec<AgentSkillRequestEntry>,
    scope: OutboundScope,
}

impl AgentSkillRequest {
    pub fn new(
        mut entries: Vec<AgentSkillRequestEntry>,
        omissions: Vec<AgentSkillOmission>,
    ) -> Result<Self, AgentSkillInventoryError> {
        entries.sort_by(|left, right| compare_normalized_paths(&left.file, &right.file));
        let inventory = AgentSkillInventory::new(
            entries.iter().map(|entry| entry.file.clone()).collect(),
            omissions,
        )?;
        let request = Self {
            entries,
            scope: OutboundScope::agent_skill_package(inventory),
        };
        request.verify_inventory_payload()?;
        Ok(request)
    }

    pub fn payload_entries(&self) -> &[AgentSkillRequestEntry] {
        &self.entries
    }

    pub fn inventory(&self) -> &AgentSkillInventory {
        self.scope
            .agent_skill_inventory()
            .expect("an Agent Skill request always owns an Agent Skill scope")
    }

    pub fn outbound_scope(&self) -> OutboundScope {
        self.scope.clone()
    }

    pub fn framed_payload(&self) -> Vec<u8> {
        AgentSkillProviderRequest {
            entries: &self.entries,
        }
        .framed_payload()
    }

    pub fn metadata_only_entries(&self) -> impl Iterator<Item = &AgentSkillRequestEntry> {
        self.entries.iter().filter(|entry| entry.is_metadata_only())
    }

    pub fn inventory_payload_matches(&self) -> bool {
        self.inventory().matches_payload(&self.entries)
    }

    pub fn payload_matches_inventory(&self) -> bool {
        self.inventory_payload_matches()
    }

    pub fn verify_inventory_payload(&self) -> Result<(), AgentSkillInventoryError> {
        if self.inventory_payload_matches() {
            Ok(())
        } else {
            Err(AgentSkillInventoryError::InventoryPayloadMismatch)
        }
    }

    pub fn payload_proof(&self) -> AgentSkillPayloadProof {
        let provider_request = AgentSkillProviderRequest {
            entries: &self.entries,
        };
        let framed_payload_bytes = self
            .entries
            .iter()
            .map(AgentSkillRequestEntry::length_delimited_frame)
            .map(|frame| u64::try_from(frame.len()).unwrap_or(u64::MAX))
            .fold(0, u64::saturating_add);
        AgentSkillPayloadProof {
            inventory_file_count: self.inventory().files().len(),
            payload_entry_count: self.entries.len(),
            metadata_only_entry_count: self
                .entries
                .iter()
                .filter(|entry| entry.is_metadata_only())
                .count(),
            raw_content_bytes: provider_request.raw_content_bytes(),
            framed_payload_bytes,
            exact: self.inventory_payload_matches(),
        }
    }

    pub fn exact_inventory_payload_proof(&self) -> AgentSkillPayloadProof {
        self.payload_proof()
    }

    pub fn disclosure(
        &self,
        operation: ModelOperation,
        endpoint: EndpointIdentity,
    ) -> ModelRequestDisclosure {
        ModelRequestDisclosure::new(operation, endpoint, self.outbound_scope())
    }

    /// Build a complete Revision disclosure for this exact frozen Agent Skill
    /// inventory. The generic provider adapter below intentionally remains a
    /// Review-only path; Revision transport has its own provider boundary.
    pub fn revision_disclosure(
        &self,
        endpoint: EndpointIdentity,
        binding: RevisionRequestBinding,
        details: RevisionDisclosureDetails,
    ) -> ModelRequestDisclosure {
        ModelRequestDisclosure::revision_with_details(
            endpoint,
            self.outbound_scope(),
            binding,
            details,
        )
    }

    pub fn send_with<A>(
        &self,
        disclosure: &ModelRequestDisclosure,
        authorization: RequestAuthorization,
        adapter: &mut A,
    ) -> Result<(), AgentSkillSendError<A::Error>>
    where
        A: AgentSkillProviderAdapter,
    {
        if !self.inventory_payload_matches()
            || disclosure.operation() != ModelOperation::Review
            || disclosure.scope() != &self.scope
            || adapter.endpoint() != disclosure.endpoint()
            || !authorization.matches(disclosure)
        {
            return Err(AgentSkillSendError::AuthorizationMismatch);
        }
        adapter
            .send(AgentSkillProviderRequest {
                entries: &self.entries,
            })
            .map_err(AgentSkillSendError::Adapter)
    }
}

impl fmt::Debug for AgentSkillRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentSkillRequest")
            .field("inventory", self.inventory())
            .finish_non_exhaustive()
    }
}

fn normalize_relative_path(raw: &str) -> Result<String, AgentSkillInventoryError> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(AgentSkillInventoryError::InvalidRelativePath);
    }
    let path = raw.replace('\\', "/");
    if path.starts_with('/')
        || path
            .as_bytes()
            .get(1)
            .is_some_and(|character| *character == b':')
    {
        return Err(AgentSkillInventoryError::InvalidRelativePath);
    }

    let mut normalized = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(AgentSkillInventoryError::InvalidRelativePath),
            component => normalized.push(component),
        }
    }
    if normalized.is_empty() {
        return Err(AgentSkillInventoryError::InvalidRelativePath);
    }
    Ok(normalized.join("/"))
}

trait NormalizedRelativePath {
    fn normalized_relative_path(&self) -> &str;
}

impl NormalizedRelativePath for AgentSkillFile {
    fn normalized_relative_path(&self) -> &str {
        &self.normalized_relative_path
    }
}

impl NormalizedRelativePath for AgentSkillOmission {
    fn normalized_relative_path(&self) -> &str {
        &self.normalized_relative_path
    }
}

fn compare_normalized_paths(
    left: &impl NormalizedRelativePath,
    right: &impl NormalizedRelativePath,
) -> std::cmp::Ordering {
    left.normalized_relative_path()
        .as_bytes()
        .cmp(right.normalized_relative_path().as_bytes())
}

fn validate_file_size(byte_size: u64) -> Result<(), AgentSkillInventoryError> {
    if byte_size > AGENT_SKILL_MAX_FILE_BYTES as u64 {
        Err(AgentSkillInventoryError::FileTooLarge)
    } else {
        Ok(())
    }
}

fn validate_total_size(byte_size: u64) -> Result<(), AgentSkillInventoryError> {
    if byte_size > AGENT_SKILL_MAX_TOTAL_BYTES as u64 {
        Err(AgentSkillInventoryError::AggregateTooLarge)
    } else {
        Ok(())
    }
}

fn sha256_digest(bytes: &[u8]) -> [u8; AGENT_SKILL_SHA256_BYTES] {
    let digest = Sha256::digest(bytes);
    let mut result = [0; AGENT_SKILL_SHA256_BYTES];
    result.copy_from_slice(&digest);
    result
}

/// Encode one source frame as:
///
/// `u64-be path-length | path UTF-8 bytes | u64-be content-length | raw bytes`
///
/// Lengths are byte lengths, not character or line counts. Normalizing the
/// path here makes the helper safe to use independently of package discovery.
pub fn encode_length_delimited_frame(
    relative_path: impl AsRef<str>,
    content: &[u8],
) -> Result<Vec<u8>, AgentSkillInventoryError> {
    let path = normalize_relative_path(relative_path.as_ref())?;
    let path_bytes = path.as_bytes();
    let path_length =
        u64::try_from(path_bytes.len()).map_err(|_| AgentSkillInventoryError::ByteSizeOverflow)?;
    let content_length =
        u64::try_from(content.len()).map_err(|_| AgentSkillInventoryError::ByteSizeOverflow)?;
    validate_file_size(content_length)?;
    let capacity = AGENT_SKILL_FRAME_LENGTH_BYTES
        .checked_add(path_bytes.len())
        .and_then(|capacity| capacity.checked_add(AGENT_SKILL_FRAME_LENGTH_BYTES))
        .and_then(|capacity| capacity.checked_add(content.len()))
        .ok_or(AgentSkillInventoryError::ByteSizeOverflow)?;
    let mut frame = Vec::with_capacity(capacity);
    frame.extend_from_slice(&path_length.to_be_bytes());
    frame.extend_from_slice(path_bytes);
    frame.extend_from_slice(&content_length.to_be_bytes());
    frame.extend_from_slice(content);
    Ok(frame)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSkillInventoryError {
    InvalidRelativePath,
    EmptyInclusionReason,
    EmptyOmissionReason,
    DuplicatePath,
    ByteSizeOverflow,
    FileTooLarge,
    AggregateTooLarge,
    NonUtf8Text,
    InventoryPayloadMismatch,
}

impl fmt::Display for AgentSkillInventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentSkillInventoryError::InvalidRelativePath => {
                formatter.write_str("Agent Skill paths must be normalized relative paths")
            }
            AgentSkillInventoryError::EmptyInclusionReason => {
                formatter.write_str("Agent Skill files require an inclusion reason")
            }
            AgentSkillInventoryError::EmptyOmissionReason => {
                formatter.write_str("omitted Agent Skill files require a reason")
            }
            AgentSkillInventoryError::DuplicatePath => {
                formatter.write_str("Agent Skill inventory paths must be unique")
            }
            AgentSkillInventoryError::ByteSizeOverflow => {
                formatter.write_str("Agent Skill inventory byte size overflowed")
            }
            AgentSkillInventoryError::FileTooLarge => {
                write!(
                    formatter,
                    "Agent Skill file exceeds the {}-byte source limit",
                    AGENT_SKILL_MAX_FILE_BYTES
                )
            }
            AgentSkillInventoryError::AggregateTooLarge => {
                write!(
                    formatter,
                    "Agent Skill package exceeds the {}-byte aggregate source limit",
                    AGENT_SKILL_MAX_TOTAL_BYTES
                )
            }
            AgentSkillInventoryError::NonUtf8Text => {
                formatter.write_str("Agent Skill text content is not valid UTF-8")
            }
            AgentSkillInventoryError::InventoryPayloadMismatch => {
                formatter.write_str("Agent Skill inventory and provider payload do not match")
            }
        }
    }
}

impl std::error::Error for AgentSkillInventoryError {}

/// Immutable identity for one Revision request.
///
/// Every field comes from the reviewed source and the answers currently shown
/// to the user. Keeping these values together prevents a caller from binding
/// only the two model-context digests while silently changing the source or
/// artifact lens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RevisionRequestBinding {
    source_sha256: [u8; 32],
    source_revision: u64,
    source_generation: u64,
    artifact_lens_digest: [u8; 32],
    review_context_digest: [u8; 32],
    answers_digest: [u8; 32],
}

impl RevisionRequestBinding {
    pub const fn new(
        source_sha256: [u8; 32],
        source_revision: u64,
        source_generation: u64,
        artifact_lens_digest: [u8; 32],
        review_context_digest: [u8; 32],
        answers_digest: [u8; 32],
    ) -> Self {
        Self {
            source_sha256,
            source_revision,
            source_generation,
            artifact_lens_digest,
            review_context_digest,
            answers_digest,
        }
    }

    pub const fn source_sha256(&self) -> &[u8; 32] {
        &self.source_sha256
    }

    pub const fn source_revision(&self) -> u64 {
        self.source_revision
    }

    pub const fn source_generation(&self) -> u64 {
        self.source_generation
    }

    pub const fn artifact_lens_digest(&self) -> &[u8; 32] {
        &self.artifact_lens_digest
    }

    pub const fn review_context_digest(&self) -> &[u8; 32] {
        &self.review_context_digest
    }

    pub const fn answers_digest(&self) -> &[u8; 32] {
        &self.answers_digest
    }
}

/// Everything the user must inspect before one model request can be authorized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionDisclosureDetails {
    review_context_bytes: u64,
    answers_bytes: u64,
    answer_count: u32,
}

impl RevisionDisclosureDetails {
    pub const fn new(review_context_bytes: u64, answers_bytes: u64, answer_count: u32) -> Self {
        Self {
            review_context_bytes,
            answers_bytes,
            answer_count,
        }
    }

    pub const fn review_context_bytes(self) -> u64 {
        self.review_context_bytes
    }

    pub const fn answers_bytes(self) -> u64 {
        self.answers_bytes
    }

    pub const fn answer_count(self) -> u32 {
        self.answer_count
    }
}

/// Everything the user must inspect before one model request can be authorized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequestDisclosure {
    operation: ModelOperation,
    endpoint: EndpointIdentity,
    scope: OutboundScope,
    revision_binding: Option<RevisionRequestBinding>,
    revision_details: Option<RevisionDisclosureDetails>,
}

impl ModelRequestDisclosure {
    pub const fn new(
        operation: ModelOperation,
        endpoint: EndpointIdentity,
        scope: OutboundScope,
    ) -> Self {
        Self {
            operation,
            endpoint,
            scope,
            revision_binding: None,
            revision_details: None,
        }
    }

    /// Create an incomplete Revision disclosure.
    ///
    /// This compatibility constructor deliberately omits the displayed Review
    /// context and answer sizes. Such a disclosure is never authorizable;
    /// callers must use [`Self::revision_with_details`] for a real request.
    pub fn revision(
        endpoint: EndpointIdentity,
        scope: OutboundScope,
        binding: RevisionRequestBinding,
    ) -> Self {
        Self {
            operation: ModelOperation::Revision,
            endpoint,
            scope,
            revision_binding: Some(binding),
            revision_details: None,
        }
    }

    pub fn revision_with_details(
        endpoint: EndpointIdentity,
        scope: OutboundScope,
        binding: RevisionRequestBinding,
        details: RevisionDisclosureDetails,
    ) -> Self {
        Self {
            operation: ModelOperation::Revision,
            endpoint,
            scope,
            revision_binding: Some(binding),
            revision_details: Some(details),
        }
    }

    /// Fallible constructor for Review and Translation callers. Revision must
    /// use [`Self::revision_with_details`] because consent requires its
    /// complete typed binding and disclosure details.
    pub fn try_new(
        operation: ModelOperation,
        endpoint: EndpointIdentity,
        scope: OutboundScope,
    ) -> Result<Self, ModelRequestDisclosureError> {
        if let Some(error) = Self::validation_error(operation, &scope) {
            return Err(error);
        }
        if operation == ModelOperation::Revision {
            return Err(ModelRequestDisclosureError::RevisionMissingDigests);
        }
        Ok(Self::new(operation, endpoint, scope))
    }

    pub const fn operation(&self) -> ModelOperation {
        self.operation
    }

    pub fn endpoint(&self) -> &EndpointIdentity {
        &self.endpoint
    }

    pub fn scope(&self) -> &OutboundScope {
        &self.scope
    }

    pub fn revision_binding(&self) -> Option<&RevisionRequestBinding> {
        self.revision_binding.as_ref()
    }

    pub const fn revision_details(&self) -> Option<RevisionDisclosureDetails> {
        self.revision_details
    }

    pub fn review_context_digest(&self) -> Option<&[u8; 32]> {
        self.revision_binding
            .as_ref()
            .map(RevisionRequestBinding::review_context_digest)
    }

    pub fn answers_digest(&self) -> Option<&[u8; 32]> {
        self.revision_binding
            .as_ref()
            .map(RevisionRequestBinding::answers_digest)
    }

    pub const fn is_review_scope_allowed(&self) -> bool {
        match self.operation {
            ModelOperation::Review => self.scope.permits_review(),
            ModelOperation::Revision => false,
            ModelOperation::Translation => true,
        }
    }

    pub const fn is_revision_scope_allowed(&self) -> bool {
        matches!(self.operation, ModelOperation::Revision)
            && self.scope.permits_revision()
            && self.scope.permits_review()
            && self.revision_binding.is_some()
            && self.revision_details.is_some()
    }

    pub const fn is_scope_allowed(&self) -> bool {
        match self.operation {
            ModelOperation::Review => self.scope.permits_review(),
            ModelOperation::Revision => self.is_revision_scope_allowed(),
            ModelOperation::Translation => true,
        }
    }

    pub const fn protocol_framing_crosses_boundary(&self) -> bool {
        true
    }

    pub const fn disclosed_source_content_crosses_boundary(&self) -> bool {
        true
    }

    fn binding(&self) -> RequestBinding {
        RequestBinding {
            operation: self.operation,
            endpoint: self.endpoint.clone(),
            scope: self.scope.binding.clone(),
            revision_binding: self.revision_binding,
            revision_details: self.revision_details,
        }
    }

    fn validation_error(
        operation: ModelOperation,
        scope: &OutboundScope,
    ) -> Option<ModelRequestDisclosureError> {
        match operation {
            ModelOperation::Review if !scope.permits_review() => {
                Some(ModelRequestDisclosureError::ReviewEffectiveAgentContext)
            }
            ModelOperation::Revision if !scope.permits_review() => {
                Some(ModelRequestDisclosureError::RevisionEffectiveAgentContext)
            }
            ModelOperation::Revision if !scope.permits_revision() => {
                Some(ModelRequestDisclosureError::RevisionScopeNotAllowed)
            }
            ModelOperation::Translation | ModelOperation::Review | ModelOperation::Revision => None,
        }
    }

    fn rejection_error(&self) -> Option<ModelRequestDisclosureError> {
        match self.operation {
            ModelOperation::Review if !self.scope.permits_review() => {
                Some(ModelRequestDisclosureError::ReviewEffectiveAgentContext)
            }
            ModelOperation::Revision if !self.scope.permits_review() => {
                Some(ModelRequestDisclosureError::RevisionEffectiveAgentContext)
            }
            ModelOperation::Revision if !self.scope.permits_revision() => {
                Some(ModelRequestDisclosureError::RevisionScopeNotAllowed)
            }
            ModelOperation::Revision if self.revision_binding.is_none() => {
                Some(ModelRequestDisclosureError::RevisionMissingDigests)
            }
            ModelOperation::Revision if self.revision_details.is_none() => {
                Some(ModelRequestDisclosureError::RevisionMissingDetails)
            }
            ModelOperation::Review | ModelOperation::Revision | ModelOperation::Translation => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRequestDisclosureError {
    ReviewEffectiveAgentContext,
    RevisionEffectiveAgentContext,
    RevisionScopeNotAllowed,
    RevisionMissingDigests,
    RevisionMissingDetails,
}

impl fmt::Display for ModelRequestDisclosureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReviewEffectiveAgentContext => formatter.write_str(
                "Review cannot resolve or include Effective Agent Context before Goal 08",
            ),
            Self::RevisionEffectiveAgentContext => formatter.write_str(
                "Revision cannot resolve or include Effective Agent Context before Goal 08",
            ),
            Self::RevisionScopeNotAllowed => formatter.write_str(
                "Revision scope must be a document, selection, or frozen Agent Skill package",
            ),
            Self::RevisionMissingDigests => formatter
                .write_str("Revision consent requires a complete source and Review binding"),
            Self::RevisionMissingDetails => formatter.write_str(
                "Revision consent requires displayed Review context and answer disclosure details",
            ),
        }
    }
}

impl std::error::Error for ModelRequestDisclosureError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentDecision {
    Approve,
    Cancel,
}

#[derive(Clone, PartialEq, Eq)]
struct RequestBinding {
    operation: ModelOperation,
    endpoint: EndpointIdentity,
    scope: ScopeBinding,
    revision_binding: Option<RevisionRequestBinding>,
    revision_details: Option<RevisionDisclosureDetails>,
}

impl RequestBinding {
    fn matches(&self, request: &ModelRequestDisclosure) -> bool {
        self.operation == request.operation
            && self.endpoint == request.endpoint
            && self.scope == request.scope.binding
            && self.revision_binding == request.revision_binding
            && self.revision_details == request.revision_details
    }
}

enum ConsentState {
    Pending(Box<RequestBinding>),
    Cancelled,
    Rejected(ModelRequestDisclosureError),
    Consumed,
}

/// A non-cloneable, one-shot grant bound to one displayed request disclosure.
pub struct ConsentCapability {
    state: ConsentState,
}

impl ConsentCapability {
    pub fn from_decision(disclosure: &ModelRequestDisclosure, decision: ConsentDecision) -> Self {
        let state = if let Some(error) = disclosure.rejection_error() {
            ConsentState::Rejected(error)
        } else {
            match decision {
                ConsentDecision::Approve => ConsentState::Pending(Box::new(disclosure.binding())),
                ConsentDecision::Cancel => ConsentState::Cancelled,
            }
        };
        Self { state }
    }

    /// Authorize exactly one request. Any attempt consumes the capability.
    pub fn authorize(
        &mut self,
        request: &ModelRequestDisclosure,
    ) -> Result<RequestAuthorization, ConsentError> {
        match std::mem::replace(&mut self.state, ConsentState::Consumed) {
            ConsentState::Pending(binding) if binding.matches(request) => {
                Ok(RequestAuthorization { binding: *binding })
            }
            ConsentState::Pending(_) => Err(ConsentError::Mismatch),
            ConsentState::Cancelled => Err(ConsentError::Cancelled),
            ConsentState::Rejected(error) => Err(ConsentError::Rejected(error)),
            ConsentState::Consumed => Err(ConsentError::Consumed),
        }
    }
}

impl fmt::Debug for ConsentCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match self.state {
            ConsentState::Pending(_) => "pending",
            ConsentState::Cancelled => "cancelled",
            ConsentState::Rejected(_) => "rejected",
            ConsentState::Consumed => "consumed",
        };
        formatter
            .debug_struct("ConsentCapability")
            .field("state", &state)
            .finish_non_exhaustive()
    }
}

/// Proof that the matching disclosure was approved and consumed once.
pub struct RequestAuthorization {
    binding: RequestBinding,
}

impl RequestAuthorization {
    pub fn matches(&self, request: &ModelRequestDisclosure) -> bool {
        self.binding.matches(request)
    }
}

impl fmt::Debug for RequestAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RequestAuthorization { .. }")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentError {
    Cancelled,
    Mismatch,
    Rejected(ModelRequestDisclosureError),
    Consumed,
}

impl fmt::Display for ConsentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsentError::Cancelled => formatter.write_str("model request consent was cancelled"),
            ConsentError::Mismatch => {
                formatter.write_str("model request does not match the approved disclosure")
            }
            ConsentError::Rejected(error) => error.fmt(formatter),
            ConsentError::Consumed => {
                formatter.write_str("model request consent was already consumed")
            }
        }
    }
}

impl std::error::Error for ConsentError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(provider: Provider, raw: &str) -> EndpointIdentity {
        EndpointIdentity::parse(provider, Some(raw)).unwrap()
    }

    fn revision_disclosure(
        endpoint: EndpointIdentity,
        source_scope: OutboundScope,
        review_context_digest: [u8; 32],
        answers_digest: [u8; 32],
    ) -> ModelRequestDisclosure {
        ModelRequestDisclosure::revision_with_details(
            endpoint,
            source_scope,
            RevisionRequestBinding::new(
                [9; 32],
                7,
                11,
                [8; 32],
                review_context_digest,
                answers_digest,
            ),
            RevisionDisclosureDetails::new(128, 64, 2),
        )
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

    #[test]
    fn consent_is_bound_to_operation_endpoint_and_scope_then_consumed() {
        let default_endpoint = EndpointIdentity::parse(Provider::OpenAiResponses, None).unwrap();
        let scope = OutboundScope::selection(8);
        let disclosure = ModelRequestDisclosure::new(
            ModelOperation::Review,
            default_endpoint.clone(),
            scope.clone(),
        );
        let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);

        let authorization = consent.authorize(&disclosure).unwrap();
        assert!(authorization.matches(&disclosure));
        assert!(matches!(
            consent.authorize(&disclosure),
            Err(ConsentError::Consumed)
        ));

        let mismatches = [
            ModelRequestDisclosure::new(
                ModelOperation::Translation,
                default_endpoint.clone(),
                scope.clone(),
            ),
            ModelRequestDisclosure::new(
                ModelOperation::Review,
                endpoint(Provider::OpenAiResponses, "https://proxy.example/v1/"),
                scope.clone(),
            ),
            ModelRequestDisclosure::new(
                ModelOperation::Review,
                default_endpoint,
                OutboundScope::selection(8),
            ),
        ];
        for mismatch in mismatches {
            let mut consent =
                ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
            assert!(matches!(
                consent.authorize(&mismatch),
                Err(ConsentError::Mismatch)
            ));
            assert!(matches!(
                consent.authorize(&disclosure),
                Err(ConsentError::Consumed)
            ));
        }
    }

    #[test]
    fn cancellation_never_authorizes_a_request() {
        let disclosure = ModelRequestDisclosure::new(
            ModelOperation::Translation,
            EndpointIdentity::parse(Provider::AnthropicMessages, None).unwrap(),
            OutboundScope::document(42),
        );
        let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Cancel);
        assert!(matches!(
            consent.authorize(&disclosure),
            Err(ConsentError::Cancelled)
        ));
        assert!(matches!(
            consent.authorize(&disclosure),
            Err(ConsentError::Consumed)
        ));
    }

    #[test]
    fn effective_context_scope_names_sources_and_has_a_distinct_binding() {
        let scope = OutboundScope::document_with_effective_agent_context(
            100,
            [
                " workspace AGENTS.md ",
                "global AGENTS.md",
                "workspace AGENTS.md",
            ],
        )
        .unwrap();
        assert_eq!(
            scope.kind(),
            OutboundScopeKind::DocumentWithEffectiveAgentContext
        );
        assert_eq!(
            scope.effective_context_sources(),
            &["global AGENTS.md", "workspace AGENTS.md"]
        );
        assert_ne!(scope, OutboundScope::document(100));
    }

    #[test]
    fn agent_skill_request_keeps_disclosure_and_payload_exactly_aligned() {
        struct RecordingProviderAdapter {
            endpoint: EndpointIdentity,
            entries: Vec<(String, u64, String, Vec<u8>)>,
        }

        impl AgentSkillProviderAdapter for RecordingProviderAdapter {
            type Error = std::convert::Infallible;

            fn endpoint(&self) -> &EndpointIdentity {
                &self.endpoint
            }

            fn send(&mut self, request: AgentSkillProviderRequest<'_>) -> Result<(), Self::Error> {
                self.entries = request
                    .entries()
                    .iter()
                    .map(|entry| {
                        (
                            entry.file().normalized_relative_path().to_owned(),
                            entry.file().byte_size(),
                            entry.file().inclusion_reason().to_owned(),
                            entry.bytes().to_vec(),
                        )
                    })
                    .collect();
                Ok(())
            }
        }

        let request = AgentSkillRequest::new(
            vec![
                AgentSkillRequestEntry::new(
                    "references\\guide.md",
                    "referenced support",
                    b"guide bytes".to_vec(),
                )
                .unwrap(),
                AgentSkillRequestEntry::new(
                    ".//SKILL.md",
                    "entrypoint",
                    b"sentinel source body".to_vec(),
                )
                .unwrap(),
            ],
            vec![AgentSkillOmission::new("references/private.md", "excluded by policy").unwrap()],
        )
        .unwrap();
        let inventory = request.inventory();
        assert_eq!(
            inventory.total_byte_size(),
            u64::try_from(b"guide bytes".len() + b"sentinel source body".len()).unwrap()
        );
        assert!(inventory.is_partial());
        assert_eq!(inventory.omissions().len(), 1);
        assert_eq!(
            inventory.omissions()[0].normalized_relative_path(),
            "references/private.md"
        );
        assert_eq!(inventory.omissions()[0].reason(), "excluded by policy");
        assert_eq!(inventory.files()[0].normalized_relative_path(), "SKILL.md");
        assert_eq!(
            inventory.files()[1].normalized_relative_path(),
            "references/guide.md"
        );
        assert_eq!(
            inventory.files()[1].inclusion_reason(),
            "referenced support"
        );

        let payload = request.payload_entries();
        assert_eq!(payload.len(), inventory.files().len());
        for (entry, disclosed) in payload.iter().zip(inventory.files()) {
            assert_eq!(entry.file(), disclosed);
            assert_eq!(
                u64::try_from(entry.bytes().len()).unwrap(),
                disclosed.byte_size()
            );
        }
        assert_eq!(payload[0].bytes(), b"sentinel source body");
        assert_eq!(payload[1].bytes(), b"guide bytes");
        assert!(
            payload
                .iter()
                .all(|entry| entry.file().normalized_relative_path() != "private.txt")
        );
        assert!(!format!("{request:?}").contains("sentinel source body"));

        let disclosure = request.disclosure(
            ModelOperation::Review,
            EndpointIdentity::parse(Provider::OpenAiResponses, None).unwrap(),
        );
        assert!(disclosure.protocol_framing_crosses_boundary());
        assert!(disclosure.disclosed_source_content_crosses_boundary());
        assert_eq!(disclosure.scope(), &request.outbound_scope());
        let disclosed = disclosure.scope().agent_skill_inventory().unwrap();
        assert!(disclosed.is_partial());
        assert_eq!(disclosed.files().len(), 2);

        let mut adapter = RecordingProviderAdapter {
            endpoint: disclosure.endpoint().clone(),
            entries: Vec::new(),
        };
        let mismatched_disclosure = ModelRequestDisclosure::new(
            ModelOperation::Review,
            disclosure.endpoint().clone(),
            OutboundScope::document(1),
        );
        let mut mismatched_consent =
            ConsentCapability::from_decision(&mismatched_disclosure, ConsentDecision::Approve);
        let mismatched_authorization = mismatched_consent
            .authorize(&mismatched_disclosure)
            .unwrap();
        assert_eq!(
            request.send_with(&disclosure, mismatched_authorization, &mut adapter,),
            Err(AgentSkillSendError::AuthorizationMismatch)
        );
        assert!(adapter.entries.is_empty());

        let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
        let authorization = consent.authorize(&disclosure).unwrap();
        let mut wrong_endpoint_adapter = RecordingProviderAdapter {
            endpoint: EndpointIdentity::parse(Provider::AnthropicMessages, None).unwrap(),
            entries: Vec::new(),
        };
        assert_eq!(
            request.send_with(&disclosure, authorization, &mut wrong_endpoint_adapter,),
            Err(AgentSkillSendError::AuthorizationMismatch)
        );
        assert!(wrong_endpoint_adapter.entries.is_empty());

        let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
        let authorization = consent.authorize(&disclosure).unwrap();
        request
            .send_with(&disclosure, authorization, &mut adapter)
            .unwrap();
        assert_eq!(adapter.entries.len(), disclosed.files().len());
        for (sent, file) in adapter.entries.iter().zip(disclosed.files()) {
            assert_eq!(sent.0, file.normalized_relative_path());
            assert_eq!(sent.1, file.byte_size());
            assert_eq!(sent.2, file.inclusion_reason());
        }
        assert_eq!(adapter.entries[0].3, b"sentinel source body");
        assert_eq!(adapter.entries[1].3, b"guide bytes");
        assert!(adapter.entries.iter().all(|entry| entry.0 != "private.txt"));
    }

    #[test]
    fn agent_skill_request_rejects_unsafe_or_ambiguous_paths() {
        for raw in ["", "../secret", "C:\\secret", "/absolute", "foo/../../bar"] {
            assert!(matches!(
                AgentSkillRequestEntry::new(raw, "support", vec![1]),
                Err(AgentSkillInventoryError::InvalidRelativePath)
            ));
        }
        let duplicate = AgentSkillRequest::new(
            vec![
                AgentSkillRequestEntry::new("./references/guide.md", "support", vec![1]).unwrap(),
                AgentSkillRequestEntry::new("references\\guide.md", "support", vec![2]).unwrap(),
            ],
            Vec::new(),
        );
        assert!(matches!(
            duplicate,
            Err(AgentSkillInventoryError::DuplicatePath)
        ));

        let included_and_omitted = AgentSkillRequest::new(
            vec![AgentSkillRequestEntry::new("SKILL.md", "entrypoint", vec![1]).unwrap()],
            vec![AgentSkillOmission::new("./SKILL.md", "excluded").unwrap()],
        );
        assert!(matches!(
            included_and_omitted,
            Err(AgentSkillInventoryError::DuplicatePath)
        ));
    }

    #[test]
    fn source_bytes_default_to_metadata_for_non_utf8_and_raw_requires_opt_in() {
        let binary = AgentSkillRequestEntry::from_source_bytes(
            "assets\\blob.bin",
            "support asset",
            vec![0xff, 0x00, 0x80],
        )
        .unwrap();
        assert!(binary.is_metadata_only());
        assert!(binary.payload_bytes().is_none());
        assert!(binary.bytes().is_empty());
        assert_eq!(binary.file().normalized_relative_path(), "assets/blob.bin");
        assert_eq!(binary.file().byte_size(), 3);
        assert_eq!(
            binary.file().sha256_hex(),
            "ef192b7af54e943f206ab27075ec1805384c972c9959fc5820f1fa7d5268fcef"
        );

        let explicit = AgentSkillRequestEntry::new(
            "assets/blob.bin",
            "explicitly selected binary",
            vec![0xff, 0x00, 0x80],
        )
        .unwrap();
        assert_eq!(explicit.content_kind(), AgentSkillContentKind::ExplicitRaw);
        assert_eq!(explicit.payload_bytes(), Some(&[0xff, 0x00, 0x80][..]));

        let text = AgentSkillRequestEntry::from_source_bytes(
            "docs/readme.md",
            "support text",
            "中文 text".as_bytes().to_vec(),
        )
        .unwrap();
        assert_eq!(text.content_kind(), AgentSkillContentKind::Utf8Text);
        assert_eq!(text.payload_bytes(), Some("中文 text".as_bytes()));
    }

    #[test]
    fn skill_source_limits_apply_per_file_and_in_aggregate() {
        let at_file_limit = AgentSkillRequestEntry::from_source_bytes(
            "at-limit.md",
            "support",
            vec![b'x'; AGENT_SKILL_MAX_FILE_BYTES],
        )
        .unwrap();
        assert_eq!(
            at_file_limit.file().byte_size(),
            AGENT_SKILL_MAX_FILE_BYTES as u64
        );

        assert!(matches!(
            AgentSkillRequestEntry::from_source_bytes(
                "too-large.md",
                "support",
                vec![b'x'; AGENT_SKILL_MAX_FILE_BYTES + 1],
            ),
            Err(AgentSkillInventoryError::FileTooLarge)
        ));

        let entries = (0..8)
            .map(|index| {
                AgentSkillRequestEntry::from_source_bytes(
                    format!("part-{index}.md"),
                    "support",
                    vec![b'x'; AGENT_SKILL_MAX_FILE_BYTES],
                )
                .unwrap()
            })
            .collect();
        let request = AgentSkillRequest::new(entries, Vec::new()).unwrap();
        assert_eq!(
            request.inventory().total_byte_size(),
            AGENT_SKILL_MAX_TOTAL_BYTES as u64
        );

        let entries = (0..9)
            .map(|index| {
                AgentSkillRequestEntry::from_source_bytes(
                    format!("part-{index}.md"),
                    "support",
                    vec![b'x'; AGENT_SKILL_MAX_FILE_BYTES],
                )
                .unwrap()
            })
            .collect();
        assert!(matches!(
            AgentSkillRequest::new(entries, Vec::new()),
            Err(AgentSkillInventoryError::AggregateTooLarge)
        ));
    }

    #[test]
    fn package_order_is_normalized_and_compared_by_utf8_path_bytes() {
        let request = AgentSkillRequest::new(
            vec![
                AgentSkillRequestEntry::new("z\\b.md", "support", b"z".to_vec()).unwrap(),
                AgentSkillRequestEntry::new("./a\\é.md", "support", b"unicode".to_vec()).unwrap(),
                AgentSkillRequestEntry::new("a/z.md", "support", b"ascii".to_vec()).unwrap(),
            ],
            Vec::new(),
        )
        .unwrap();
        let paths = request
            .inventory()
            .files()
            .iter()
            .map(AgentSkillFile::normalized_relative_path)
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["a/z.md", "a/é.md", "z/b.md"]);
        assert!(
            request
                .inventory()
                .files()
                .windows(2)
                .all(|pair| pair[0].normalized_relative_path_bytes()
                    <= pair[1].normalized_relative_path_bytes())
        );
    }

    #[test]
    fn source_frame_is_length_delimited_and_metadata_entries_are_hashed_without_raw_bytes() {
        let entry = AgentSkillRequestEntry::new(
            "./refs\\guide.bin",
            "explicit support",
            vec![0xff, 0x00, 0x01],
        )
        .unwrap();
        let frame = entry.length_delimited_frame();
        let path_length = u64::from_be_bytes(frame[..8].try_into().unwrap()) as usize;
        assert_eq!(&frame[8..8 + path_length], b"refs/guide.bin");
        let content_length_offset = 8 + path_length;
        let content_length = u64::from_be_bytes(
            frame[content_length_offset..content_length_offset + 8]
                .try_into()
                .unwrap(),
        ) as usize;
        assert_eq!(content_length, 3);
        assert_eq!(
            &frame[content_length_offset + 8..content_length_offset + 8 + content_length],
            &[0xff, 0x00, 0x01]
        );

        let metadata = AgentSkillRequestEntry::from_source_bytes(
            "refs/blob.bin",
            "binary support",
            vec![0xff, 0x00, 0x01],
        )
        .unwrap();
        let metadata_frame = metadata.length_delimited_frame();
        let metadata_path_length =
            u64::from_be_bytes(metadata_frame[..8].try_into().unwrap()) as usize;
        assert_eq!(
            &metadata_frame[8..8 + metadata_path_length],
            b"refs/blob.bin"
        );
        let metadata_content_offset = 8 + metadata_path_length;
        let metadata_content_length = u64::from_be_bytes(
            metadata_frame[metadata_content_offset..metadata_content_offset + 8]
                .try_into()
                .unwrap(),
        ) as usize;
        assert_eq!(
            metadata_content_length,
            b"markturbo-agent-skill-metadata-v1\0".len() + 8 + 64
        );
        assert_ne!(
            &metadata_frame[metadata_content_offset + 8..],
            &[0xff, 0x00, 0x01]
        );
        let changed_metadata = AgentSkillRequestEntry::from_source_bytes(
            "refs/blob.bin",
            "binary support",
            vec![0xfe, 0x00, 0x01],
        )
        .unwrap();
        assert_ne!(metadata_frame, changed_metadata.length_delimited_frame());

        let raw_frame_length = entry.length_delimited_frame().len();
        let request = AgentSkillRequest::new(vec![entry, metadata], Vec::new()).unwrap();
        let proof = request.payload_proof();
        assert!(proof.is_exact());
        assert_eq!(proof.inventory_file_count(), 2);
        assert_eq!(proof.payload_entry_count(), 2);
        assert_eq!(proof.metadata_only_entry_count(), 1);
        assert_eq!(proof.raw_content_bytes(), 3);
        assert_eq!(
            proof.framed_payload_bytes(),
            (raw_frame_length + metadata_frame.len()) as u64
        );
        assert_eq!(
            request.framed_payload().len(),
            proof.framed_payload_bytes() as usize
        );
    }

    #[test]
    fn review_cannot_authorize_effective_agent_context() {
        let scope = OutboundScope::document_with_effective_agent_context(4, ["AGENTS.md"]).unwrap();
        let endpoint = EndpointIdentity::parse(Provider::OpenAiResponses, None).unwrap();
        assert_eq!(
            ModelRequestDisclosure::try_new(
                ModelOperation::Review,
                endpoint.clone(),
                scope.clone()
            ),
            Err(ModelRequestDisclosureError::ReviewEffectiveAgentContext)
        );

        let disclosure = ModelRequestDisclosure::new(ModelOperation::Review, endpoint, scope);
        assert!(!disclosure.is_review_scope_allowed());
        let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
        assert!(matches!(
            consent.authorize(&disclosure),
            Err(ConsentError::Rejected(
                ModelRequestDisclosureError::ReviewEffectiveAgentContext
            ))
        ));
        assert!(matches!(
            consent.authorize(&disclosure),
            Err(ConsentError::Consumed)
        ));

        let translation = ModelRequestDisclosure::new(
            ModelOperation::Translation,
            disclosure.endpoint().clone(),
            disclosure.scope().clone(),
        );
        let mut translation_consent =
            ConsentCapability::from_decision(&translation, ConsentDecision::Approve);
        assert!(translation_consent.authorize(&translation).is_ok());
    }

    #[test]
    fn review_consent_cannot_authorize_revision() {
        let endpoint = endpoint(Provider::OpenAiResponses, "https://api.openai.com/v1/");
        let source_scope = OutboundScope::document(128);
        let review = ModelRequestDisclosure::new(
            ModelOperation::Review,
            endpoint.clone(),
            source_scope.clone(),
        );
        let revision = revision_disclosure(endpoint, source_scope, [1; 32], [2; 32]);

        let mut consent = ConsentCapability::from_decision(&review, ConsentDecision::Approve);
        assert!(matches!(
            consent.authorize(&revision),
            Err(ConsentError::Mismatch)
        ));
        assert!(matches!(
            consent.authorize(&review),
            Err(ConsentError::Consumed)
        ));
    }

    #[test]
    fn revision_consent_is_fresh_and_one_shot() {
        let base_endpoint = endpoint(Provider::OpenAiResponses, "https://api.openai.com/v1/");
        let revision = revision_disclosure(
            base_endpoint,
            OutboundScope::document(128),
            [1; 32],
            [2; 32],
        );

        let mut consent = ConsentCapability::from_decision(&revision, ConsentDecision::Approve);
        assert!(consent.authorize(&revision).is_ok());
        assert!(matches!(
            consent.authorize(&revision),
            Err(ConsentError::Consumed)
        ));

        let selection = revision_disclosure(
            endpoint(Provider::OpenAiResponses, "https://api.openai.com/v1/"),
            OutboundScope::selection(128),
            [1; 32],
            [2; 32],
        );
        let mut selection_consent =
            ConsentCapability::from_decision(&selection, ConsentDecision::Approve);
        assert!(selection_consent.authorize(&selection).is_ok());
    }

    #[test]
    fn revision_without_complete_binding_is_rejected() {
        let disclosure = ModelRequestDisclosure::new(
            ModelOperation::Revision,
            endpoint(Provider::OpenAiResponses, "https://api.openai.com/v1/"),
            OutboundScope::document(128),
        );

        let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
        assert!(matches!(
            consent.authorize(&disclosure),
            Err(ConsentError::Rejected(
                ModelRequestDisclosureError::RevisionMissingDigests
            ))
        ));
        assert!(matches!(
            consent.authorize(&disclosure),
            Err(ConsentError::Consumed)
        ));
    }

    #[test]
    fn revision_without_disclosure_details_is_rejected() {
        let disclosure = ModelRequestDisclosure::revision(
            endpoint(Provider::OpenAiResponses, "https://api.openai.com/v1/"),
            OutboundScope::document(128),
            RevisionRequestBinding::new([1; 32], 7, 11, [2; 32], [3; 32], [4; 32]),
        );

        assert!(!disclosure.is_revision_scope_allowed());
        let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
        assert!(matches!(
            consent.authorize(&disclosure),
            Err(ConsentError::Rejected(
                ModelRequestDisclosureError::RevisionMissingDetails
            ))
        ));
        assert!(matches!(
            consent.authorize(&disclosure),
            Err(ConsentError::Consumed)
        ));
    }

    #[test]
    fn revision_consent_rejects_endpoint_scope_and_digest_changes() {
        let base_endpoint = endpoint(Provider::OpenAiResponses, "https://api.openai.com/v1/");
        let source_scope = OutboundScope::document(128);
        let expected = revision_disclosure(
            base_endpoint.clone(),
            source_scope.clone(),
            [1; 32],
            [2; 32],
        );
        let mismatches = [
            revision_disclosure(
                endpoint(Provider::OpenAiResponses, "https://proxy.example/v1/"),
                source_scope.clone(),
                [1; 32],
                [2; 32],
            ),
            revision_disclosure(
                base_endpoint.clone(),
                OutboundScope::selection(128),
                [1; 32],
                [2; 32],
            ),
            revision_disclosure(
                base_endpoint.clone(),
                source_scope.clone(),
                [3; 32],
                [2; 32],
            ),
            revision_disclosure(base_endpoint, source_scope, [1; 32], [4; 32]),
        ];

        for mismatch in mismatches {
            let mut consent = ConsentCapability::from_decision(&expected, ConsentDecision::Approve);
            assert!(matches!(
                consent.authorize(&mismatch),
                Err(ConsentError::Mismatch)
            ));
            assert!(matches!(
                consent.authorize(&expected),
                Err(ConsentError::Consumed)
            ));
        }
    }

    #[test]
    fn revision_consent_rejects_source_snapshot_and_lens_binding_changes() {
        let endpoint = endpoint(Provider::OpenAiResponses, "https://api.openai.com/v1/");
        let scope = OutboundScope::document(128);
        let expected_binding =
            RevisionRequestBinding::new([1; 32], 7, 11, [4; 32], [2; 32], [3; 32]);
        let details = RevisionDisclosureDetails::new(128, 64, 2);
        let expected = ModelRequestDisclosure::revision_with_details(
            endpoint.clone(),
            scope.clone(),
            expected_binding,
            details,
        );
        let mismatches = [
            RevisionRequestBinding::new([9; 32], 7, 11, [4; 32], [2; 32], [3; 32]),
            RevisionRequestBinding::new([1; 32], 8, 11, [4; 32], [2; 32], [3; 32]),
            RevisionRequestBinding::new([1; 32], 7, 12, [4; 32], [2; 32], [3; 32]),
            RevisionRequestBinding::new([1; 32], 7, 11, [5; 32], [2; 32], [3; 32]),
            RevisionRequestBinding::new([1; 32], 7, 11, [4; 32], [6; 32], [3; 32]),
            RevisionRequestBinding::new([1; 32], 7, 11, [4; 32], [2; 32], [7; 32]),
        ];

        for binding in mismatches {
            let changed = ModelRequestDisclosure::revision_with_details(
                endpoint.clone(),
                scope.clone(),
                binding,
                details,
            );
            let mut consent = ConsentCapability::from_decision(&expected, ConsentDecision::Approve);
            assert!(matches!(
                consent.authorize(&changed),
                Err(ConsentError::Mismatch)
            ));
            assert!(matches!(
                consent.authorize(&expected),
                Err(ConsentError::Consumed)
            ));
        }
    }

    #[test]
    fn revision_consent_rejects_block_scope() {
        let endpoint = endpoint(Provider::OpenAiResponses, "https://api.openai.com/v1/");
        let revision = revision_disclosure(
            endpoint.clone(),
            OutboundScope::block(128),
            [1; 32],
            [2; 32],
        );
        assert_eq!(
            ModelRequestDisclosure::try_new(
                ModelOperation::Revision,
                endpoint,
                OutboundScope::block(128),
            ),
            Err(ModelRequestDisclosureError::RevisionScopeNotAllowed)
        );

        let mut consent = ConsentCapability::from_decision(&revision, ConsentDecision::Approve);
        assert!(matches!(
            consent.authorize(&revision),
            Err(ConsentError::Rejected(
                ModelRequestDisclosureError::RevisionScopeNotAllowed
            ))
        ));
        assert!(matches!(
            consent.authorize(&revision),
            Err(ConsentError::Consumed)
        ));
    }

    #[test]
    fn revision_consent_cannot_expand_to_effective_agent_context() {
        let endpoint = endpoint(Provider::OpenAiResponses, "https://api.openai.com/v1/");
        let scope =
            OutboundScope::document_with_effective_agent_context(128, ["workspace/AGENTS.md"])
                .unwrap();
        let revision = revision_disclosure(endpoint, scope, [1; 32], [2; 32]);

        assert!(
            !revision.is_revision_scope_allowed(),
            "Effective Agent Context must never be a Revision scope"
        );
        let mut consent = ConsentCapability::from_decision(&revision, ConsentDecision::Approve);
        assert!(matches!(
            consent.authorize(&revision),
            Err(ConsentError::Rejected(
                ModelRequestDisclosureError::RevisionEffectiveAgentContext
            ))
        ));
        assert!(matches!(
            consent.authorize(&revision),
            Err(ConsentError::Consumed)
        ));
    }

    #[test]
    fn revision_consent_does_not_mutate_model_settings_or_credential_identity() {
        let mut settings = crate::settings::AppSettings::default();
        settings.model_provider = Provider::OpenAiResponses.key().to_owned();
        settings.model_name = "revision-test-model".to_owned();
        settings.model_base_url = "https://proxy.example/v1/".to_owned();
        let settings_before = settings.clone();
        let config = ModelConfig::new(
            Provider::OpenAiResponses,
            Some(settings.model_name.as_str()),
            Some(settings.model_base_url.as_str()),
        )
        .unwrap();
        let config_before = config.clone();
        let endpoint = config.endpoint().clone();
        let credential_target_before = endpoint.credential_target();
        let revision =
            revision_disclosure(endpoint, OutboundScope::document(128), [1; 32], [2; 32]);

        let mut consent = ConsentCapability::from_decision(&revision, ConsentDecision::Approve);
        let authorization = consent.authorize(&revision).unwrap();
        assert!(authorization.matches(&revision));
        assert_eq!(settings, settings_before);
        assert_eq!(config, config_before);
        assert_eq!(
            config.endpoint().credential_target(),
            credential_target_before
        );
    }

    #[test]
    fn agent_skill_revision_requires_the_frozen_inventory_scope() {
        struct RecordingProviderAdapter {
            endpoint: EndpointIdentity,
            send_count: usize,
        }

        impl AgentSkillProviderAdapter for RecordingProviderAdapter {
            type Error = std::convert::Infallible;

            fn endpoint(&self) -> &EndpointIdentity {
                &self.endpoint
            }

            fn send(&mut self, _request: AgentSkillProviderRequest<'_>) -> Result<(), Self::Error> {
                self.send_count += 1;
                Ok(())
            }
        }

        let request = AgentSkillRequest::new(
            vec![
                AgentSkillRequestEntry::new(
                    "SKILL.md",
                    "entrypoint",
                    b"frozen skill source".to_vec(),
                )
                .unwrap(),
            ],
            Vec::new(),
        )
        .unwrap();
        let endpoint = endpoint(Provider::OpenAiResponses, "https://api.openai.com/v1/");
        let generic_revision = request.disclosure(ModelOperation::Revision, endpoint.clone());
        let mut generic_consent =
            ConsentCapability::from_decision(&generic_revision, ConsentDecision::Approve);
        assert!(matches!(
            generic_consent.authorize(&generic_revision),
            Err(ConsentError::Rejected(
                ModelRequestDisclosureError::RevisionMissingDigests
            ))
        ));
        let binding = RevisionRequestBinding::new([3; 32], 7, 11, [4; 32], [1; 32], [2; 32]);
        let inventory_scope = request.outbound_scope();
        let details = RevisionDisclosureDetails::new(0, 0, 0);
        let inventory_disclosure = request.revision_disclosure(endpoint.clone(), binding, details);
        assert_eq!(inventory_disclosure.scope(), &inventory_scope);
        assert_eq!(inventory_disclosure.revision_details(), Some(details));
        let mut consent =
            ConsentCapability::from_decision(&inventory_disclosure, ConsentDecision::Approve);
        let authorization = consent.authorize(&inventory_disclosure).unwrap();
        let mut adapter = RecordingProviderAdapter {
            endpoint: endpoint.clone(),
            send_count: 0,
        };
        assert_eq!(
            request.send_with(&inventory_disclosure, authorization, &mut adapter),
            Err(AgentSkillSendError::AuthorizationMismatch)
        );
        assert_eq!(adapter.send_count, 0);

        let broad_disclosure = ModelRequestDisclosure::revision_with_details(
            endpoint,
            OutboundScope::document(request.inventory().total_byte_size()),
            binding,
            RevisionDisclosureDetails::new(0, 0, 0),
        );
        let mut broad_consent =
            ConsentCapability::from_decision(&broad_disclosure, ConsentDecision::Approve);
        let broad_authorization = broad_consent.authorize(&broad_disclosure).unwrap();
        assert_eq!(
            request.send_with(&broad_disclosure, broad_authorization, &mut adapter),
            Err(AgentSkillSendError::AuthorizationMismatch)
        );
        assert_eq!(adapter.send_count, 0);
    }
}
