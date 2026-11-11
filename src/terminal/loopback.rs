#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LoopbackUrl {
    pub scheme: String,
    pub host: LoopbackHost,
    pub port: u16,
    suffix: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LoopbackHost {
    Localhost,
    Ipv4,
    Ipv6,
}

impl LoopbackUrl {
    pub fn normalized(&self) -> String {
        format!(
            "{}://{}:{}{}",
            self.scheme,
            self.host.url_host(),
            self.port,
            self.suffix
        )
    }

    pub fn forwarded_url(&self, local_port: u16) -> String {
        format!("{}://127.0.0.1:{}{}", self.scheme, local_port, self.suffix)
    }

    pub fn forward_host(&self) -> &'static str {
        self.host.forward_host()
    }
}

impl LoopbackHost {
    fn url_host(self) -> &'static str {
        match self {
            LoopbackHost::Localhost => "localhost",
            LoopbackHost::Ipv4 => "127.0.0.1",
            LoopbackHost::Ipv6 => "[::1]",
        }
    }

    fn forward_host(self) -> &'static str {
        match self {
            LoopbackHost::Localhost => "localhost",
            LoopbackHost::Ipv4 => "127.0.0.1",
            LoopbackHost::Ipv6 => "::1",
        }
    }
}

pub(super) fn parse_loopback_url(input: &str) -> Option<LoopbackUrl> {
    if input.is_empty() || input.chars().any(char::is_whitespace) {
        return None;
    }
    let (scheme, rest) = if let Some((scheme, rest)) = input.split_once("://") {
        if !matches!(scheme, "http" | "https") {
            return None;
        }
        (scheme, rest)
    } else {
        ("http", input)
    };

    let (host, after_host) = if let Some(rest) = rest.strip_prefix("[::1]") {
        (LoopbackHost::Ipv6, rest)
    } else if let Some(rest) = strip_prefix_ignore_ascii_case(rest, "localhost") {
        (LoopbackHost::Localhost, rest)
    } else if let Some(rest) = rest.strip_prefix("127.0.0.1") {
        (LoopbackHost::Ipv4, rest)
    } else {
        return None;
    };

    let after_colon = after_host.strip_prefix(':')?;
    let digits = after_colon
        .as_bytes()
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }
    let port = after_colon[..digits].parse::<u16>().ok()?;
    let suffix = &after_colon[digits..];
    if !suffix.is_empty() && !matches!(suffix.as_bytes()[0], b'/' | b'?' | b'#') {
        return None;
    }

    Some(LoopbackUrl {
        scheme: scheme.to_string(),
        host,
        port,
        suffix: suffix.to_string(),
    })
}

pub(super) fn loopback_url_span_at(text: &str, col: usize) -> Option<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    if col >= chars.len() || chars[col].is_whitespace() {
        return None;
    }

    let mut start = col;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && !chars[end + 1].is_whitespace() {
        end += 1;
    }

    let (start, end, token) = trim_wrapped_token(start, end, &chars);
    let parsed = parse_loopback_url(&token)?;
    (start..=end)
        .contains(&col)
        .then(|| (start, end, parsed.normalized()))
}

/// Strips wrapping punctuation off the inclusive span `start..=end`.
///
/// Works on a half-open `start..stop` so the trailing scan cannot get stuck:
/// with an inclusive `end` and a saturating decrement, a token at column 0
/// made only of trailing characters (`>>`, `}`, `});`) left `end` pinned at 0
/// with `start <= end` forever true — a spin on the UI thread every time the
/// pointer hovered such a line.
fn trim_wrapped_token(start: usize, end: usize, chars: &[char]) -> (usize, usize, String) {
    let mut start = start;
    let mut stop = end + 1;
    while start < stop && matches!(chars[start], '(' | '[' | '<' | '\'' | '"' | '{') {
        start += 1;
    }
    while start < stop
        && matches!(
            chars[stop - 1],
            ')' | ']' | '>' | '\'' | '"' | '}' | ',' | ';'
        )
    {
        stop -= 1;
    }
    let token = chars[start..stop].iter().collect();
    (start, stop.saturating_sub(1), token)
}

fn strip_prefix_ignore_ascii_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        .then(|| &s[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scheme_loopback_urls() {
        let url = parse_loopback_url("https://localhost:3000/a?b=1#c").unwrap();
        assert_eq!(url.host, LoopbackHost::Localhost);
        assert_eq!(url.port, 3000);
        assert_eq!(url.normalized(), "https://localhost:3000/a?b=1#c");
        assert_eq!(url.forwarded_url(49152), "https://127.0.0.1:49152/a?b=1#c");

        assert_eq!(
            parse_loopback_url("http://127.0.0.1:8080")
                .unwrap()
                .forward_host(),
            "127.0.0.1"
        );
        assert_eq!(
            parse_loopback_url("http://localhost:8080")
                .unwrap()
                .forward_host(),
            "localhost"
        );
        assert_eq!(
            parse_loopback_url("http://[::1]:5173")
                .unwrap()
                .forward_host(),
            "::1"
        );
    }

    #[test]
    fn parses_bare_loopback_hosts_as_http() {
        assert_eq!(
            parse_loopback_url("localhost:3000/path")
                .unwrap()
                .normalized(),
            "http://localhost:3000/path"
        );
        assert_eq!(
            parse_loopback_url("127.0.0.1:8080?q=1")
                .unwrap()
                .normalized(),
            "http://127.0.0.1:8080?q=1"
        );
    }

    #[test]
    fn rejects_non_loopback_or_ambiguous_tokens() {
        assert!(parse_loopback_url("https://example.com:443").is_none());
        assert!(parse_loopback_url("localhost").is_none());
        assert!(parse_loopback_url("localhost:99999").is_none());
        assert!(parse_loopback_url("ftp://localhost:21").is_none());
        assert!(parse_loopback_url("localhost:3000abc").is_none());
    }

    #[test]
    fn detects_loopback_url_span_in_text() {
        let text = "open (localhost:3000/path), now";
        let (start, end, url) = loopback_url_span_at(text, 8).unwrap();
        assert_eq!((start, end), (6, 24));
        assert_eq!(url, "http://localhost:3000/path");
        assert!(loopback_url_span_at(text, 5).is_none());
    }

    /// A token made only of trailing delimiters at column 0 used to spin the
    /// trim loop forever (`>> dns i…` hung the GUI on hover). Runs on a
    /// thread so a regression fails the test instead of hanging the suite.
    #[test]
    fn delimiter_only_token_at_column_zero_terminates() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            for text in [">> dns ixfr", "}", ")", "});", "> x", "'\"", "(", "({[<"] {
                for col in 0..text.chars().count() {
                    out.push(loopback_url_span_at(text, col));
                }
            }
            let _ = tx.send(out);
        });
        let spans = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("trim_wrapped_token did not terminate");
        assert!(spans.iter().all(Option::is_none));
    }

    #[test]
    fn trims_wrapping_delimiters() {
        let chars: Vec<char> = "(http://localhost:3000),".chars().collect();
        let (start, end, token) = trim_wrapped_token(0, chars.len() - 1, &chars);
        assert_eq!(token, "http://localhost:3000");
        assert_eq!((start, end), (1, 21));

        let chars: Vec<char> = ">>".chars().collect();
        assert_eq!(trim_wrapped_token(0, 1, &chars).2, "");
    }
}
