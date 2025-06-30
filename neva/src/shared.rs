//! Shared utilities for server and client

#[cfg(any(feature = "server", feature = "client"))]
use tokio_util::sync::CancellationToken;

#[cfg(any(feature = "server", feature = "client"))]
pub(crate) use requests_queue::RequestQueue;
pub(crate) use arc_str::ArcStr;
pub(crate) use arc_slice::ArcSlice;

#[cfg(any(feature = "server", feature = "client"))]
pub(crate) mod requests_queue;
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) mod message_registry;
#[cfg(feature = "http-client")]
pub mod mt;
pub(crate) mod arc_str;
pub(crate) mod arc_slice;

#[inline]
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) fn wait_for_shutdown_signal(token: CancellationToken) {
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(_) => (),
            #[cfg(feature = "tracing")]
            Err(err) => tracing::error!(
                logger = "neva",
                "Unable to listen for shutdown signal: {}", err),
            #[cfg(not(feature = "tracing"))]
            Err(_) => ()
        }
        token.cancel();
    });
}


pub(crate) struct MemChr;
impl MemChr {
    #[inline]
    pub(crate) fn contains(s: &str, ch: u8) -> bool {
        memchr::memchr(ch, s.as_bytes()).is_some()
    }
    
    #[inline]
    pub(crate) fn split(s: &str, ch: u8) -> impl Iterator<Item=&str> {
        let bytes = s.as_bytes();
        let mut last = 0;
        memchr::memchr_iter(ch, bytes)
            .chain(std::iter::once(bytes.len()))
            .map(move |i| {
                let part = &s[last..i];
                last = i + 1;
                part
            })
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
    fn it_does_not_split_on_char() {
        assert_eq!(MemChr::split("ab", b'/').collect::<Vec<_>>(), ["ab"]);
    }
}
