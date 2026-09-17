mod support;

use mast::{Error, DEFAULT_HOST};
use support::{open, KEY};

#[test]
fn takes_the_url_the_app_copies() {
    let channel = open(&format!("https://mast.tissue.dev/{KEY}")).unwrap();
    assert_eq!(channel.origin(), "https://mast.tissue.dev");
    assert_eq!(channel.key(), KEY);
    assert_eq!(channel.url(), format!("https://mast.tissue.dev/{KEY}"));
}

#[test]
fn keeps_the_m_prefix_it_was_given() {
    // The edge rewrites / onto /m/, but a self-hosted one in front of the
    // management API may not, so the prefix that came in is the one used.
    let channel = open(&format!("https://mast.tissue.dev/m/{KEY}")).unwrap();
    assert_eq!(channel.url(), format!("https://mast.tissue.dev/m/{KEY}"));
}

#[test]
fn a_bare_key_goes_to_the_default_host() {
    let channel = open(KEY).unwrap();
    assert_eq!(channel.origin(), DEFAULT_HOST);
    assert_eq!(channel.url(), format!("{DEFAULT_HOST}/{KEY}"));
}

#[test]
fn assumes_https_when_no_scheme_was_pasted() {
    let channel = open(&format!("mast.tissue.dev/{KEY}")).unwrap();
    assert_eq!(channel.origin(), "https://mast.tissue.dev");
}

#[test]
fn allows_http_for_a_host_on_a_private_network() {
    let channel = open(&format!("http://mast.internal:8080/m/{KEY}")).unwrap();
    assert_eq!(channel.origin(), "http://mast.internal:8080");
    assert_eq!(channel.url(), format!("http://mast.internal:8080/m/{KEY}"));
}

#[test]
fn reads_a_scheme_in_capitals() {
    let channel = open(&format!("HTTPS://mast.tissue.dev/{KEY}")).unwrap();
    assert_eq!(channel.origin(), "https://mast.tissue.dev");
}

#[test]
fn ignores_a_trailing_slash_a_query_and_a_fragment() {
    for raw in [
        format!("https://mast.tissue.dev/{KEY}/"),
        format!("https://mast.tissue.dev/{KEY}?utm_source=app"),
        format!("https://mast.tissue.dev/{KEY}#copied"),
    ] {
        let channel = open(&raw).unwrap();
        assert_eq!(
            channel.url(),
            format!("https://mast.tissue.dev/{KEY}"),
            "{raw}"
        );
    }
}

#[test]
fn trims_what_came_off_the_clipboard() {
    let channel = open(&format!("  https://mast.tissue.dev/{KEY}\n")).unwrap();
    assert_eq!(channel.key(), KEY);
}

#[test]
fn refuses_a_url_with_no_key_in_it() {
    let err = open("https://mast.tissue.dev/").unwrap_err();
    assert!(matches!(err, Error::InvalidUrl(_)), "{err}");
    assert!(err.to_string().contains("no key"), "{err}");
}

#[test]
fn refuses_something_that_is_not_a_key() {
    for raw in [
        "https://mast.tissue.dev/mk_short",
        "https://mast.tissue.dev/mk_0123456789abcdef0123456789abcdef0123456",
        "https://mast.tissue.dev/mk_0123456789abcdef0123456789abcdef012345678",
        "https://mast.tissue.dev/mk_0123456789ABCDEF0123456789abcdef01234567",
        "https://mast.tissue.dev/mk_0123456789abcdefg123456789abcdef01234567",
        "https://mast.tissue.dev/mm_0123456789abcdef0123456789abcdef",
    ] {
        let err = open(raw).unwrap_err();
        assert!(matches!(err, Error::InvalidUrl(_)), "{raw} gave {err}");
    }
}

#[test]
fn refuses_a_scheme_that_is_not_http() {
    let err = open(&format!("ftp://mast.tissue.dev/{KEY}")).unwrap_err();
    assert!(err.to_string().contains("http(s)"), "{err}");
}

#[test]
fn refuses_an_empty_url() {
    let err = open("   ").unwrap_err();
    assert!(matches!(err, Error::InvalidUrl(_)), "{err}");
}

#[test]
fn printing_a_channel_does_not_print_the_key() {
    let channel = open(&format!("https://mast.tissue.dev/{KEY}")).unwrap();
    let shown = format!("{channel} {channel:?}");
    assert!(!shown.contains(KEY), "{shown}");
    assert!(shown.contains("mk_0123"), "{shown}");
}
