use std::{collections::HashMap, sync::Arc};

use crate::{
	Label,
	expr::tree::LazyExpressionTree,
	value::{ELazyValue, extract::EvalError},
};

/// How `to_json` handles values that have no JSON representation
/// (e.g. binary blobs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JsonMode {
	/// Serializing such a value is a hard error ([ExtractError::NotJsonable]).
	Strict,

	/// Such a value is rendered as a placeholder string.
	Placeholder,

	/// Such a value is omitted: `to_json` yields `None`, and containers drop
	/// the field/element.
	#[default]
	None,
}

#[derive(Debug, Clone, Default)]
pub struct ExtractState {
	/// If true, extract all fields from all items.
	/// Do not pre-filter using mime type.
	///
	/// This may detect additional fields, but
	/// makes extraction take much longer
	pub ignore_mime: bool,

	/// How `to_json` handles values with no JSON representation. See [JsonMode].
	pub json_mode: JsonMode,

	/// If true, a request for a field the extractor does not recognize (or
	/// arguments passed to a field that does not accept them) is a hard error
	/// ([ExtractError::BadField]) instead of a silent `None`.
	pub strict: bool,

	/// Aliases in scope for expressions, parsed lazily as needed.
	pub aliases: HashMap<Label, LazyExpressionTree>,

	/// All paths we resolved, in the order we resolved them.
	pub access_log: Option<Arc<parking_lot::Mutex<Vec<LazyExpressionTree>>>>,
}

impl ExtractState {
	pub fn unknown_field(&self, name: &Label) -> Result<Option<ELazyValue>, EvalError> {
		if self.strict {
			Err(EvalError::BadField(format!(
				"unknown field {:?}",
				name.as_str()
			)))
		} else {
			Ok(None)
		}
	}

	pub fn unexpected_args(&self, name: &Label) -> Result<Option<ELazyValue>, EvalError> {
		if self.strict {
			Err(EvalError::InvalidArgs(format!(
				"field {:?} does not accept arguments",
				name.as_str()
			)))
		} else {
			Ok(None)
		}
	}

	pub fn resolve_field(
		&self,
		name: &Label,
		value: Option<ELazyValue>,
	) -> Result<Option<ELazyValue>, EvalError> {
		match value {
			Some(v) => Ok(Some(v)),
			None => self.unknown_field(name),
		}
	}
}
