use crate::{Error, DEFAULT_HOST};

pub(crate) fn is_key(value: &str) -> bool {
    matches(value, "mk_", 40)
}

pub(crate) fn is_message_id(value: &str) -> bool {
    matches(value, "mm_", 32)
}

fn matches(value: &str, prefix: &str, digits: usize) -> bool {
    match value.strip_prefix(prefix) {
        Some(rest) => {
            rest.len() == digits
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        None => false,
    }
}

/// Split a channel URL into its origin, the prefix that was pasted, and the
/// key.
///
/// Takes the short form the app copies, the explicit `/m/` form the management
/// API hands out, a host with no scheme, and a bare key. The edge rewrites `/`
/// onto `/m/`, but a self-hosted one in front of the management API may not, so
/// whichever prefix came in is kept rather than normalised.
pub(crate) fn parse_channel_url(raw: &str) -> Result<(String, String, String), Error> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(Error::InvalidUrl("no channel URL given".into()));
    }
    if is_key(text) {
        return Ok((DEFAULT_HOST.to_string(), String::new(), text.to_string()));
    }

    let (scheme, rest) = match text.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("https".to_string(), text),
    };
    if scheme != "https" && scheme != "http" {
        return Err(Error::InvalidUrl(
            "a channel URL has to be an http(s) URL".into(),
        ));
    }

    let rest = rest
        .split(['?', '#'])
        .next()
        .expect("split always yields one part");
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, path),
        None => (rest, ""),
    };
    if authority.is_empty() {
        return Err(Error::InvalidUrl(format!("{raw:?} has no host")));
    }

    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let mut first = segments.next().unwrap_or_default();
    let mut prefix = String::new();
    if first == "m" {
        prefix.push_str("/m");
        first = segments.next().unwrap_or_default();
    }

    if first.is_empty() {
        return Err(Error::InvalidUrl("there is no key in that URL".into()));
    }
    if !is_key(first) {
        return Err(Error::InvalidUrl(format!(
            "{first:?} is not a channel key: expected mk_ and 40 hex characters"
        )));
    }
    Ok((format!("{scheme}://{authority}"), prefix, first.to_string()))
}

/// Percent-encode one value for an `application/x-www-form-urlencoded` body.
pub(crate) fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
