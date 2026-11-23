//! Link activations from the web preview: which bridge messages count, and
//! the one system-browser open they cause.

use std::cell::Cell;
use std::time::{Duration, Instant};

use url::Url;

// Goal 19 clause 4: at most one system open per 500 ms.
pub(crate) const OPEN_INTERVAL: Duration = Duration::from_millis(500);

const OPEN_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

pub(crate) fn target(message: &str) -> Option<Url> {
    if message.len() > nsh_core::open_url::MAX_URL_LEN
        || message.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let url = Url::parse(message).ok()?;
    if !OPEN_SCHEMES.contains(&url.scheme()) {
        return None;
    }
    if matches!(url.scheme(), "http" | "https") && url.host_str().is_none_or(str::is_empty) {
        return None;
    }
    Some(url)
}

pub(crate) struct Opener {
    last: Cell<Option<Instant>>,
    open: Box<dyn Fn(&str)>,
}

impl Opener {
    pub(crate) fn new(open: impl Fn(&str) + 'static) -> Self {
        Self {
            last: Cell::new(None),
            open: Box::new(open),
        }
    }

    pub(crate) fn activated(&self, message: &str, now: Instant) -> bool {
        let Some(url) = target(message) else {
            return false;
        };
        if self
            .last
            .get()
            .is_some_and(|last| now.saturating_duration_since(last) < OPEN_INTERVAL)
        {
            return false;
        }
        self.last.set(Some(now));
        (self.open)(url.as_str());
        true
    }
}

thread_local! {
    static PREVIEW_OPENER: Opener = Opener::new(|url| {
        drop(nsh_core::element::open_url(url.to_owned()));
    });
}

pub(crate) fn preview_message(message: &str) {
    PREVIEW_OPENER.with(|opener| opener.activated(message, Instant::now()));
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    fn recorder() -> (Rc<RefCell<Vec<String>>>, Opener) {
        let opened = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&opened);
        (
            opened,
            Opener::new(move |url| sink.borrow_mut().push(url.to_owned())),
        )
    }

    #[test]
    fn target_allows_http_https_and_mailto() {
        for message in [
            "https://example.com/",
            "http://127.0.0.1:8080/a?b=c#d",
            "https://example.com/a%20b",
            "mailto:owner@example.com",
            "MAILTO:owner@example.com",
        ] {
            assert!(target(message).is_some(), "{message}");
        }
    }

    #[test]
    fn target_denies_every_other_scheme_and_relative_reference() {
        for message in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script></script>",
            "custom:thing",
            "tel:+15550100",
            "about:blank",
            "blob:https://example.com/id",
            "docs/intro.md",
            "/absolute/path",
            "//example.com/scheme-relative",
            "#anchor",
            "?query=1",
            "C:/Windows/System32/calc.exe",
            "{\"url\":\"https://example.com/\"}",
            "https://",
            "",
            "not a url",
            "https://example.com/\n",
        ] {
            assert!(target(message).is_none(), "{message}");
        }
    }

    #[test]
    fn one_activation_opens_exactly_once() {
        let (opened, opener) = recorder();
        assert!(opener.activated("https://example.com/link", Instant::now()));
        assert_eq!(opened.borrow().as_slice(), ["https://example.com/link"]);
    }

    #[test]
    fn no_other_message_shape_opens_anything() {
        for message in [
            "file:///etc/passwd",
            "custom:thing",
            "docs/intro.md",
            "{\"url\":\"https://example.com/\"}",
            "",
        ] {
            let (opened, opener) = recorder();
            assert!(
                !opener.activated(message, Instant::now()),
                "{message} opened"
            );
            assert!(opened.borrow().is_empty(), "{message}");
        }
    }

    #[test]
    fn a_burst_opens_once_and_a_later_activation_opens_again() {
        let (opened, opener) = recorder();
        let start = Instant::now();
        assert!(opener.activated("https://example.com/first", start));
        for index in 0..64 {
            assert!(!opener.activated(
                &format!("https://example.com/burst-{index}"),
                start + Duration::from_millis(499),
            ));
        }
        assert!(!opener.activated(
            "https://example.com/at-the-edge",
            start + OPEN_INTERVAL - Duration::from_millis(1),
        ));
        assert!(opener.activated("https://example.com/second", start + OPEN_INTERVAL));
        assert_eq!(
            opened.borrow().as_slice(),
            ["https://example.com/first", "https://example.com/second"]
        );
    }

    #[test]
    fn a_denied_message_does_not_start_the_interval() {
        let (opened, opener) = recorder();
        let start = Instant::now();
        assert!(!opener.activated("file:///etc/passwd", start));
        assert!(opener.activated("https://example.com/link", start + Duration::from_millis(1)));
        assert_eq!(opened.borrow().as_slice(), ["https://example.com/link"]);
    }

    #[test]
    fn the_open_receives_the_parsed_url() {
        let (opened, opener) = recorder();
        assert!(opener.activated("HTTPS://EXAMPLE.com/Path?q=1#f", Instant::now()));
        assert_eq!(
            opened.borrow().as_slice(),
            ["https://example.com/Path?q=1#f"]
        );
    }
}
