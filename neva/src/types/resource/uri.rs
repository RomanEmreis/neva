﻿//! URI helpers and utilities

use serde::{Serialize, Deserialize};
use crate::error::{Error, ErrorCode};
use std::{
    borrow::Cow,
    ops::{Deref, DerefMut}
};
use std::fmt::{Display, Formatter};

/// Represents a resource URI
#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct Uri(String);

impl Deref for Uri {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        &self.0
    }
}

impl DerefMut for Uri {
    #[inline]
    fn deref_mut(&mut self) -> &mut str {
        &mut self.0
    }
}

impl Display for Uri {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for Uri {
    #[inline]
    fn from(s: String) -> Self {
        Uri(s)
    }
}

impl From<&str> for Uri {
    #[inline]
    fn from(s: &str) -> Self {
        Uri(s.to_owned())
    }
}

impl Uri {
    #[inline]
    pub fn into_inner(self) -> String {
        self.0
    }

    #[inline]
    pub fn parts<'a>(&self) -> Result<impl Iterator<Item = Cow<'a, str>> + use<'a, '_>, Error> {
        let parts = self.rsplit("//")
            .next()
            .ok_or(Error::new(ErrorCode::InvalidParams, "Invalid URI provided"))?
            .split("/")
            .map(|s| Cow::Owned(s.to_owned()));
        Ok(parts)
    }

    #[inline]
    pub fn as_vec<'a>(&self) -> Vec<Cow<'a, str>> {
        match self.parts() {
            Ok(parts) => parts.collect(),
            Err(_) => Vec::new()
        }
    }
}