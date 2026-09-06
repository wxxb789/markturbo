#![cfg(feature = "model-transport")]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Instant;

use mt_app::credentials::{CredentialError, CredentialVault, Secret, SecureCredentialStore};
use mt_app::model::{ConsentCapability, ConsentDecision, EndpointIdentity};
use mt_app::settings::AppSettings;
use mt_app::translate::{PreparedTranslation, Provider};
use mt_doc::translate::{Scope, TranslationRequest};

struct EmptyStore;

impl SecureCredentialStore for EmptyStore {
    fn is_supported(&self) -> bool {
        true
    }

    fn read(&self, _: &str) -> Result<Option<Secret>, CredentialError> {
        Ok(None)
    }

    fn write(&self, _: &str, _: &Secret) -> Result<(), CredentialError> {
        Ok(())
    }

    fn delete(&self, _: &str) -> Result<(), CredentialError> {
        Ok(())
    }
}

fn local_model_server(requests: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free loopback port");
    let address = listener.local_addr().expect("a bound loopback address");
    std::thread::spawn(move || {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"[\"bonjour\"]"}}]}"#;
        for _ in 0..requests {
            let (stream, _) = listener.accept().expect("a model request");
            let mut reader = BufReader::new(&stream);
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line
                    .to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
                {
                    length = value;
                }
            }
            let mut payload = vec![0; length];
            reader.read_exact(&mut payload).expect("the request body");
            let mut stream = &stream;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("the model response");
            stream.flush().expect("the model response flush");
        }
    });
    format!("http://{address}/v1/")
}

fn run_authorized_request(settings: &AppSettings, vault: &CredentialVault) {
    let document = mt_doc::Document::new(None, "hello".into());
    let request = TranslationRequest::prepare(&document, &Scope::Document);
    let prepared = PreparedTranslation::from_settings(settings, vault)
        .expect("the local translation is prepared");
    let prepared = prepared.bind_request(request);
    let mut consent =
        ConsentCapability::from_decision(prepared.disclosure(), ConsentDecision::Approve);
    let authorization = prepared
        .authorize(&mut consent)
        .expect("the measurement request is authorized once");
    prepared
        .execute(authorization, "fr")
        .expect("the local request succeeds");
}

#[test]
#[ignore = "Goal 04 model first-use measurement; run through mt.py probe"]
fn first_model_transport_use_cost() {
    let mut settings = AppSettings::default();
    settings.model_provider = Provider::OpenAiChat.key().into();
    settings.model_base_url = local_model_server(2);
    settings.model_name = "measurement-model".into();
    let endpoint = EndpointIdentity::parse(Provider::OpenAiChat, Some(&settings.model_base_url))
        .expect("the loopback endpoint is valid");
    let vault = CredentialVault::with_store(Arc::new(EmptyStore));
    vault
        .replace_session(
            endpoint.credential_target().to_string(),
            "measurement-placeholder".into(),
        )
        .expect("the measurement credential stays in session memory");

    let started = Instant::now();
    run_authorized_request(&settings, &vault);
    let first = started.elapsed();

    let started = Instant::now();
    run_authorized_request(&settings, &vault);
    let subsequent = started.elapsed();

    eprintln!("model first {first:?} subsequent {subsequent:?}");
}
