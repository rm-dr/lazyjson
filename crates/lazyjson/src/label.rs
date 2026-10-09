use serde::{Deserialize, Serialize};
use smartstring::{LazyCompact, SmartString};
use std::fmt::{Debug, Display};
use std::ops::Deref;

/// A sanitized string, guaranteed to only contain
/// chars in `A-z`, `0-9`, and `-_`.
///
/// Used for names of datasets, fields, etc.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Label(SmartString<LazyCompact>);

impl Label {
	pub fn is_valid(c: char) -> bool {
		matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_')
	}

	pub fn new(str: impl AsRef<str>) -> Option<Self> {
		let str: &str = str.as_ref();
		for c in str.chars() {
			if !Self::is_valid(c) {
				return None;
			}
		}

		return Some(Self(str.into()));
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}

	pub fn into_string(self) -> String {
		self.0.into()
	}
}

impl Display for Label {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.0)
	}
}

impl From<Label> for String {
	fn from(value: Label) -> Self {
		value.0.into()
	}
}

impl TryFrom<String> for Label {
	type Error = InvalidLabel;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		Self::new(value).ok_or(InvalidLabel)
	}
}

impl TryFrom<&str> for Label {
	type Error = InvalidLabel;

	fn try_from(value: &str) -> Result<Self, Self::Error> {
		Self::new(value).ok_or(InvalidLabel)
	}
}

impl AsRef<str> for Label {
	fn as_ref(&self) -> &str {
		&self.0
	}
}

impl Deref for Label {
	type Target = str;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidLabel;

impl Display for InvalidLabel {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(
			f,
			"Invalid label: must only contain characters in A-z, 0-9, or -_.",
		)
	}
}

impl std::error::Error for InvalidLabel {}
