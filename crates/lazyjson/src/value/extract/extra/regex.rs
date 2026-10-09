use regex::Regex;
use smartstring::{LazyCompact, SmartString};
use std::sync::Arc;

use crate::value::extract::ExtractState;
use crate::{
	Label,
	value::{
		ELazyValue, LazyValue,
		extract::{
			ExtractError,
			traits::{ListExtractor, ObjectExtractor},
		},
	},
};

struct RegexData {
	regex: Arc<Regex>,
	/// Captured substrings indexed by group index (0 = whole match).
	captures: Vec<Option<Arc<SmartString<LazyCompact>>>>,
}

impl RegexData {
	fn new(regex: Arc<Regex>, input: &str) -> Option<Self> {
		let caps = regex.captures(input)?;
		let captures = caps
			.iter()
			.map(|m| m.map(|m| Arc::new(m.as_str().into())))
			.collect();
		Some(Self { regex, captures })
	}
}

/// Exposes named capture groups as object fields.
pub struct RegexExtractor(Arc<RegexData>);

impl RegexExtractor {
	/// Run `regex` against `input`. Returns `None` if there is no match.
	pub fn new(regex: Arc<Regex>, input: &str) -> Option<Self> {
		Some(Self(Arc::new(RegexData::new(regex, input)?)))
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for RegexExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, ExtractError> {
		if args.is_some() {
			return state.unexpected_args(name);
		}

		let Some(idx) = self
			.0
			.regex
			.capture_names()
			.position(|n| n == Some(name.as_str()))
		else {
			return state.unknown_field(name);
		};

		Ok(Some(
			match self.0.captures.get(idx).and_then(|v| v.as_ref()) {
				Some(s) => LazyValue::String(s.clone()).into(),
				None => LazyValue::Null.into(),
			},
		))
	}

	async fn fields(&self, _state: &ExtractState) -> Result<Vec<Label>, ExtractError> {
		#[expect(clippy::unwrap_used)]
		Ok(self
			.0
			.regex
			.capture_names()
			.flatten()
			.map(|n| Label::new(n).unwrap())
			.collect())
	}

	fn as_list(&self) -> Option<Arc<dyn ListExtractor>> {
		Some(Arc::new(RegexExtractor(self.0.clone())))
	}
}

#[async_trait::async_trait]
impl ListExtractor for RegexExtractor {
	async fn get(
		&self,
		_state: &ExtractState,
		idx: usize,
	) -> Result<Option<ELazyValue>, ExtractError> {
		let raw_idx = idx + 1;
		let Some(slot) = self.0.captures.get(raw_idx) else {
			return Ok(None);
		};
		Ok(Some(match slot {
			Some(s) => LazyValue::String(s.clone()).into(),
			None => LazyValue::Null.into(),
		}))
	}

	async fn len(&self, _state: &ExtractState) -> Result<usize, ExtractError> {
		Ok(self.0.captures.len().saturating_sub(1))
	}
}
