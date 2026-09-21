//! What a manifest says about a value the user has to supply: an environment
//! variable, a command-line argument, or a variable a remote URL interpolates.
//!
//! All three are the registry's `Input` with a little added on top -- a name, a
//! flag name, a value hint -- so the properties are declared once here and the
//! wrappers delegate to them.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// `false` is the schema's default for every boolean here, so it is left out of
/// the JSON rather than written as `false`.
#[inline]
pub(super) fn is_false(value: &bool) -> bool {
    !*value
}

/// How a client should read the value of an [`Input`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputFormat {
    /// Plain text -- the default when nothing is said.
    String,
    /// A decimal number, written as a string.
    Number,
    /// `"true"` or `"false"`, written as a string.
    Boolean,
    /// A path on the user's filesystem, which a client may offer to browse for.
    #[serde(rename = "filepath")]
    FilePath,
}

/// A value the user supplies when installing the server: what it is for, and
/// whether it has to be given at all.
///
/// Carried by [`KeyValueInput`] (environment variables and headers),
/// [`Argument`] (command line) and a remote transport's `variables`.
///
/// # Examples
/// ```rust
/// use neva::registry::Input;
///
/// let key = Input::new()
///     .with_description("API key for the upstream service")
///     .required()
///     .secret();
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    /// What this value is, in the words shown to whoever has to provide it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,

    /// Whether the server cannot run without it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) is_required: bool,

    /// Whether clients must handle it as a credential rather than as text.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) is_secret: bool,

    /// How to read the value: see [`InputFormat`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) format: Option<InputFormat>,

    /// A fixed value. Set one and the user is not asked: it is the publisher's
    /// answer, not theirs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) value: Option<String>,

    /// What to start from when the user *is* asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) default: Option<String>,

    /// The values allowed, if the answer is one of a few.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) choices: Vec<String>,

    /// An example of the shape expected, shown while the field is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) placeholder: Option<String>,

    /// What `{curly_braces}` in [`value`](Self::value) stand for.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(super) variables: HashMap<String, Input>,
}

impl Input {
    /// An input with nothing said about it yet.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Input;
    ///
    /// let input = Input::new().with_description("where to write the log");
    /// ```
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// An input is its own properties -- what the generated setters write to.
    #[inline]
    fn input_mut(&mut self) -> &mut Input {
        self
    }
}

/// Generates the builder that fills an [`Input`], for the input itself and for
/// each of the three things that wrap one.
///
/// The properties belong to `Input` and the wrappers only add a name, so
/// writing the setters once and pointing each type at its own `Input` is what
/// keeps the four builders from drifting apart.
macro_rules! input_setters {
    ($($ty:ty => $input:ident),* $(,)?) => {$(
        impl $ty {
            /// Describes this value to whoever has to provide it.
            #[inline]
            pub fn with_description(mut self, description: impl Into<String>) -> Self {
                self.$input().description = Some(description.into());
                self
            }

            /// Marks it as one the server cannot run without.
            #[inline]
            pub fn required(mut self) -> Self {
                self.$input().is_required = true;
                self
            }

            /// Marks it as a credential, so clients handle it as one.
            #[inline]
            pub fn secret(mut self) -> Self {
                self.$input().is_secret = true;
                self
            }

            /// Says how the value should be read -- see [`InputFormat`].
            #[inline]
            pub fn with_format(mut self, format: InputFormat) -> Self {
                self.$input().format = Some(format);
                self
            }

            /// Fixes the value, so the user is not asked for it.
            #[inline]
            pub fn with_value(mut self, value: impl Into<String>) -> Self {
                self.$input().value = Some(value.into());
                self
            }

            /// Sets what the user starts from when they *are* asked.
            #[inline]
            pub fn with_default(mut self, default: impl Into<String>) -> Self {
                self.$input().default = Some(default.into());
                self
            }

            /// Restricts the answer to these values.
            #[inline]
            pub fn with_choices<I, S>(mut self, choices: I) -> Self
            where
                I: IntoIterator<Item = S>,
                S: Into<String>,
            {
                self.$input().choices = choices.into_iter().map(Into::into).collect();
                self
            }

            /// Shows an example of the expected shape while the field is empty.
            #[inline]
            pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
                self.$input().placeholder = Some(placeholder.into());
                self
            }

            /// Gives `{curly_braces}` in the value something to stand for.
            #[inline]
            pub fn with_variable(mut self, name: impl Into<String>, variable: Input) -> Self {
                self.$input().variables.insert(name.into(), variable);
                self
            }
        }
    )*};
}

/// A named value: an environment variable the server is run with, or an HTTP
/// header a remote is called with.
///
/// # Examples
/// ```rust
/// use neva::registry::KeyValueInput;
///
/// let api_key = KeyValueInput::new("WEATHER_API_KEY")
///     .with_description("API key for the weather provider")
///     .required()
///     .secret();
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValueInput {
    /// Name of the environment variable or header.
    pub(super) name: String,

    /// Everything else the registry says about a value.
    #[serde(flatten)]
    pub(super) input: Input,
}

impl KeyValueInput {
    /// A value carried under `name`, with nothing said about it yet.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::KeyValueInput;
    ///
    /// let var = KeyValueInput::new("RUST_LOG").with_default("info");
    /// ```
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            input: Input::default(),
        }
    }

    /// The name this value is carried under.
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    fn input_mut(&mut self) -> &mut Input {
        &mut self.input
    }
}

/// One command-line argument: a value inserted verbatim, or a `--flag`.
///
/// > **Note:** arguments end up on a command line, and a client that runs it
/// > through a shell can be made to run more than the server. Prefer
/// > [`KeyValueInput`] environment variables for anything user-supplied.
///
/// # Examples
/// ```rust
/// use neva::registry::Argument;
///
/// let port = Argument::named("--port").with_default("3000");
/// let config = Argument::positional("config_path")
///     .with_description("path to the server's configuration file");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Argument {
    /// A value inserted into the command line as it stands.
    #[serde(rename_all = "camelCase")]
    Positional {
        /// Names the slot for a client's configuration UI and for URL
        /// interpolation. Not part of the command line itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value_hint: Option<String>,

        /// Whether it may appear more than once.
        #[serde(default, skip_serializing_if = "is_false")]
        is_repeated: bool,

        /// Everything the registry says about a value.
        #[serde(flatten)]
        input: Input,
    },

    /// A `--flag=value` pair.
    #[serde(rename_all = "camelCase")]
    Named {
        /// The flag, leading dashes included.
        name: String,

        /// Whether it may appear more than once.
        #[serde(default, skip_serializing_if = "is_false")]
        is_repeated: bool,

        /// Everything the registry says about a value.
        #[serde(flatten)]
        input: Input,
    },
}

impl Argument {
    /// A value inserted into the command line as it stands, under a hint that
    /// names the slot for clients and for URL interpolation.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Argument;
    ///
    /// let arg = Argument::positional("config_path").required();
    /// ```
    pub fn positional(value_hint: impl Into<String>) -> Self {
        Self::Positional {
            value_hint: Some(value_hint.into()),
            is_repeated: false,
            input: Input::default(),
        }
    }

    /// A `--flag=value` pair. `name` carries its own leading dashes.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Argument;
    ///
    /// let arg = Argument::named("--port").with_default("3000");
    /// ```
    pub fn named(name: impl Into<String>) -> Self {
        Self::Named {
            name: name.into(),
            is_repeated: false,
            input: Input::default(),
        }
    }

    /// Allows this argument to appear more than once.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Argument;
    ///
    /// let arg = Argument::named("--include").repeated();
    /// ```
    pub fn repeated(mut self) -> Self {
        match &mut self {
            Self::Positional { is_repeated, .. } | Self::Named { is_repeated, .. } => {
                *is_repeated = true;
            }
        }
        self
    }

    #[inline]
    fn input_mut(&mut self) -> &mut Input {
        match self {
            Self::Positional { input, .. } | Self::Named { input, .. } => input,
        }
    }

    /// What this argument can be referred to by in a transport URL's
    /// `{curly_braces}`: a flag by its name, a positional by its value hint.
    #[inline]
    pub(super) fn template_name(&self) -> Option<&str> {
        match self {
            Self::Positional { value_hint, .. } => value_hint.as_deref(),
            Self::Named { name, .. } => Some(name),
        }
    }
}

input_setters! {
    Input => input_mut,
    KeyValueInput => input_mut,
    Argument => input_mut,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wrappers add a name and delegate the rest, so the flattened shape is
    /// what the registry's `KeyValueInput` looks like: the name beside the
    /// input's own properties, not nested under one.
    #[test]
    fn a_key_value_input_flattens_its_properties() {
        let input = KeyValueInput::new("WEATHER_API_KEY")
            .with_description("API key")
            .required()
            .secret();

        let json = serde_json::to_value(&input).expect("a key-value input serializes");

        assert_eq!(
            json,
            serde_json::json!({
                "name": "WEATHER_API_KEY",
                "description": "API key",
                "isRequired": true,
                "isSecret": true,
            })
        );
        assert_eq!(
            serde_json::from_value::<KeyValueInput>(json).expect("and reads back"),
            input
        );
    }

    /// `false` is the schema's default for all three booleans, so an input that
    /// says nothing writes nothing.
    #[test]
    fn defaults_are_left_out_of_the_json() {
        let json = serde_json::to_value(KeyValueInput::new("RUST_LOG")).expect("serializes");

        assert_eq!(json, serde_json::json!({ "name": "RUST_LOG" }));
    }

    /// Arguments are tagged by kind, and each kind carries its own key beside
    /// the shared properties.
    #[test]
    fn arguments_are_tagged_by_kind() {
        let positional = serde_json::to_value(
            Argument::positional("config_path").with_description("the config file"),
        )
        .expect("a positional argument serializes");
        assert_eq!(
            positional,
            serde_json::json!({
                "type": "positional",
                "valueHint": "config_path",
                "description": "the config file",
            })
        );

        let named = serde_json::to_value(Argument::named("--port").repeated().with_default("3000"))
            .expect("a named argument serializes");
        assert_eq!(
            named,
            serde_json::json!({
                "type": "named",
                "name": "--port",
                "isRepeated": true,
                "default": "3000",
            })
        );
        assert_eq!(
            serde_json::from_value::<Argument>(named).expect("and reads back"),
            Argument::named("--port").repeated().with_default("3000")
        );
    }

    /// The setters are generated for `Input` itself as well, so the exotic
    /// properties are reachable wherever an input is.
    #[test]
    fn an_input_carries_the_less_common_properties() {
        let input = Input::new()
            .with_format(InputFormat::FilePath)
            .with_choices(["debug", "info"])
            .with_placeholder("/var/log/server.log")
            .with_variable("home", Input::new().with_value("/home/user"));

        let json = serde_json::to_value(&input).expect("an input serializes");

        assert_eq!(json["format"], "filepath");
        assert_eq!(json["choices"], serde_json::json!(["debug", "info"]));
        assert_eq!(json["placeholder"], "/var/log/server.log");
        assert_eq!(json["variables"]["home"]["value"], "/home/user");
    }
}
