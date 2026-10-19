use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// The unreserved characters of RFC 3986, the only ones a url carries as they are.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encodes a value for one path segment or one query value, the way
/// `encodeURIComponent` does. Every runtime value pasted into a url goes through here, since
/// usernames, queries and even provider ids may carry `/`, `&`, spaces or non-ASCII letters.
pub fn component(value: &str) -> String {
    utf8_percent_encode(value, UNRESERVED).to_string()
}
