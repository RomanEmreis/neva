//! Diagnostics a server must not lose: a failed start, or a promise it
//! cannot keep.
//!
//! They go through `tracing` when there is a subscriber to take them, and to
//! stderr when there is none: without the `tracing` feature, or with it but
//! no subscriber installed, which is where an event goes nowhere and a server
//! that exits without a word looks like a hang. stderr, not stdout: the stdio
//! transport owns stdout. A subscriber that filters the event out has been
//! told what to drop, so that one is left alone.

/// Reports a diagnostic at `tracing::Level::$level`, under the `neva` logger,
/// or on stderr when no subscriber would take it (see the
/// [module docs](self)).
///
/// The arguments are those of `format!`, implicit captures included.
macro_rules! diagnostic {
    ($level:ident, $($arg:tt)+) => {{
        #[cfg(feature = "tracing")]
        {
            if $crate::shared::diagnostics::has_subscriber() {
                ::tracing::event!(::tracing::Level::$level, logger = "neva", $($arg)+);
            } else {
                ::std::eprintln!("neva: {}", ::std::format_args!($($arg)+));
            }
        }
        #[cfg(not(feature = "tracing"))]
        ::std::eprintln!("neva: {}", ::std::format_args!($($arg)+));
    }};
}

pub(crate) use diagnostic;

/// Whether a `tracing` subscriber is installed for this thread: the current
/// dispatcher, scoped or global, is anything but the no-op one `tracing`
/// falls back to when nothing was set.
#[cfg(feature = "tracing")]
pub(crate) fn has_subscriber() -> bool {
    tracing::dispatcher::get_default(|dispatch| !dispatch.is::<tracing::subscriber::NoSubscriber>())
}

#[cfg(all(test, feature = "tracing"))]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::subscriber::{NoSubscriber, with_default};
    use tracing_subscriber::{Layer, layer::SubscriberExt};

    /// The messages of the events a subscriber took.
    #[derive(Clone, Default)]
    struct Taken(Arc<Mutex<Vec<String>>>);

    impl<S: tracing::Subscriber> Layer<S> for Taken {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Message<'a>(&'a mut String);
            impl tracing::field::Visit for Message<'_> {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        *self.0 = format!("{value:?}");
                    }
                }
            }

            let mut message = String::new();
            event.record(&mut Message(&mut message));
            self.0.lock().expect("taken").push(message);
        }
    }

    // Scoped dispatchers on both sides: another test in this binary may have
    // installed a global subscriber, and a scoped one takes precedence.

    #[test]
    fn the_no_op_dispatcher_is_no_subscriber() {
        with_default(NoSubscriber::default(), || assert!(!has_subscriber()));
    }

    #[test]
    fn an_installed_subscriber_takes_the_diagnostic() {
        let taken = Taken::default();
        let subscriber = tracing_subscriber::registry().with(taken.clone());

        with_default(subscriber, || {
            assert!(has_subscriber());
            let reason = "address in use";
            diagnostic!(ERROR, "HTTP engine error: {reason}");
        });

        assert_eq!(
            *taken.0.lock().expect("taken"),
            ["HTTP engine error: address in use"]
        );
    }
}
