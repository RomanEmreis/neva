//! Utilities for MemChr

/// Wrapper around memchr
pub(crate) struct MemChr;
impl MemChr {
    /// Returns true if the string contains the given character
    #[inline]
    pub(crate) fn contains(s: &str, ch: u8) -> bool {
        memchr::memchr(ch, s.as_bytes()).is_some()
    }

    /// Splits the string on the given character
    #[inline]
    pub(crate) fn split(s: &str, ch: u8) -> impl Iterator<Item = &str> {
        let bytes = s.as_bytes();
        let mut last = 0;
        memchr::memchr_iter(ch, bytes)
            .chain(std::iter::once(bytes.len()))
            .map(move |i| {
                let part = &s[last..i];
                last = i + 1;
                part
            })
            .filter(|part| !part.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_contains_char() {
        assert!(MemChr::contains("a/b", b'/'));
    }

    #[test]
    fn it_does_not_contain_char() {
        assert!(!MemChr::contains("ab", b'/'));
    }

    #[test]
    fn it_splits_on_char() {
        assert_eq!(MemChr::split("a/b", b'/').collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn it_splits_on_char1() {
        assert_eq!(MemChr::split("res://a/b", b'/').collect::<Vec<_>>(), ["res:", "a", "b"]);
    }

    #[test]
    fn it_does_not_split_on_char() {
        assert_eq!(MemChr::split("ab", b'/').collect::<Vec<_>>(), ["ab"]);
    }

    #[test]
    fn it_splits_on_char_with_trailing_slash() {
        assert_eq!(MemChr::split("a/b/", b'/').collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn it_splits_on_char_with_leading_slash() {
        assert_eq!(MemChr::split("/a/b", b'/').collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn it_splits_on_empty() {
        assert_eq!(MemChr::split("", b'/').collect::<Vec<_>>(), Vec::<&str>::new());
    }
}