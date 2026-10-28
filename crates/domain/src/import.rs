use crate::{Draft, Header};
/// Restricted curl parser. Never evaluates shell expressions.
pub fn parse_curl(source: &str) -> Result<Draft, String> {
    let mut tokens = vec![];
    let mut token = String::new();
    let mut quote = None;
    let mut escape = false;
    for c in source.chars() {
        if escape {
            if c != '\n' {
                token.push(c);
            }
            escape = false;
        } else if c == '\\' && quote != Some('\'') {
            escape = true;
        } else if Some(c) == quote {
            quote = None;
        } else if quote.is_none() && (c == '\'' || c == '"') {
            quote = Some(c);
        } else if quote.is_none() && c.is_whitespace() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else if quote != Some('\'') && (c == '$' || c == '\x60') {
            return Err("Shell expansion is not supported; paste literal values".into());
        } else {
            token.push(c);
        }
    }
    if quote.is_some() || escape {
        return Err("Unterminated quotation or escape".into());
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    if tokens.first().map(String::as_str) != Some("curl") {
        return Err("Expected a curl command".into());
    }
    let mut draft = Draft::default();
    draft.url.clear();
    let mut explicit_method = false;
    let mut iter = tokens.into_iter().skip(1);
    while let Some(token) = iter.next() {
        match token.as_str() {
            "-X" | "--request" => {
                draft.method = iter.next().ok_or("Missing method")?;
                explicit_method = true;
            }
            "-H" | "--header" => draft
                .headers
                .extend(crate::parse_headers(&iter.next().ok_or("Missing header")?)?),
            "-d" | "--data" | "--data-raw" | "--data-binary" => {
                let data = iter.next().ok_or("Missing request body")?;
                if data.starts_with('@') && token != "--data-raw" {
                    return Err("Attach files explicitly; automatic file reads are disabled".into());
                }
                if !draft.body.is_empty() {
                    draft.body.push('&');
                }
                draft.body.push_str(&data);
                if !explicit_method {
                    draft.method = "POST".into();
                }
            }
            "-b" | "--cookie" => draft.headers.push(Header {
                name: "Cookie".into(),
                value: iter.next().ok_or("Missing cookie")?,
            }),
            "-L" | "--location" => draft.follow_redirects = true,
            "--compressed" => {}
            "--url" => draft.url = iter.next().ok_or("Missing URL")?,
            value if value.starts_with('-') => {
                return Err(format!("Unsupported curl option: {value}"));
            }
            value if draft.url.is_empty() => draft.url = value.into(),
            _ => return Err("Multiple URLs are not supported".into()),
        }
    }
    if !draft.url.starts_with("http://") && !draft.url.starts_with("https://") {
        return Err("An HTTP(S) URL is required".into());
    }
    Ok(draft)
}
pub fn parse_raw(source: &str) -> Result<Draft, String> {
    let (head, body) = source
        .split_once("\r\n\r\n")
        .or_else(|| source.split_once("\n\n"))
        .unwrap_or((source, ""));
    let normalized = head.replace("\r\n", "\n");
    let mut lines = normalized.lines();
    let mut first = lines
        .next()
        .ok_or("Missing request line")?
        .split_whitespace();
    let method = first.next().ok_or("Missing method")?.to_owned();
    let target = first.next().ok_or("Missing URL")?;
    let headers = crate::parse_headers(&lines.collect::<Vec<_>>().join("\n"))?;
    let url = if target.starts_with("http://") || target.starts_with("https://") {
        target.to_owned()
    } else {
        let host = headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("host"))
            .ok_or("Relative targets require a Host header")?;
        format!("https://{}{target}", host.value)
    };
    Ok(Draft {
        method,
        url,
        headers,
        body: body.into(),
        ..Default::default()
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn curl_round_trip() {
        let d = Draft {
            url: "https://example.test/a?q=space here".into(),
            method: "POST".into(),
            body: "{\"name\":\"O'Reilly\",\"token\":\"$literal\"}".into(),
            headers: vec![
                Header {
                    name: "X-A".into(),
                    value: "1".into(),
                },
                Header {
                    name: "X-A".into(),
                    value: "2".into(),
                },
            ],
            ..Default::default()
        };
        let parsed = parse_curl(&crate::curl(&d)).expect("round trip");
        assert_eq!(parsed.body, d.body);
        assert_eq!(parsed.headers, d.headers);
        assert_eq!(parsed.url, d.url);
    }
    #[test]
    fn refuses_expansion_files_and_unknown_flags() {
        for s in [
            "curl $(touch marker)",
            "curl https://x --data-binary @secret",
            "curl https://x --insecure",
        ] {
            assert!(parse_curl(s).is_err(), "{s}");
        }
    }
}
