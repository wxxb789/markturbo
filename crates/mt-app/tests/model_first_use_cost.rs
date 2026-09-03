#![cfg(feature = "model-transport")]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::time::Instant;

use mt_app::settings::AppSettings;
use mt_app::translate::Provider;

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

#[test]
#[ignore = "Goal 04 model first-use measurement; run through mt.py probe"]
fn first_model_transport_use_cost() {
    let settings = AppSettings {
        translate_provider: Provider::OpenAiChat.key().into(),
        translate_api_key: "measurement-placeholder".into(),
        translate_base_url: local_model_server(2),
        translate_model: "measurement-model".into(),
        ..AppSettings::default()
    };

    let started = Instant::now();
    let service = Provider::OpenAiChat
        .build_with(&settings)
        .expect("the local transport starts");
    service
        .translate(&["hello".into()], "fr")
        .expect("the first local request succeeds");
    let first = started.elapsed();

    let started = Instant::now();
    service
        .translate(&["hello".into()], "fr")
        .expect("the reused local request succeeds");
    let subsequent = started.elapsed();

    eprintln!("model first {first:?} subsequent {subsequent:?}");
}
