//! stdio transport options

/// Represents stdio transport options
pub(crate) struct StdIoOptions {
    pub command: &'static str,
    pub args: Vec<&'static str>,
}

impl StdIoOptions {
    /// Creates new stdio options
    pub(crate) fn new<T>(command: &'static str, args: T) -> Self
    where 
        T: IntoIterator<Item=&'static str>
    {
        Self {
            args: args.into_iter().collect(),
            command
        }
    }
}