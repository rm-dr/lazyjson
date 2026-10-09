use smartstring::{LazyCompact, SmartString};
use thiserror::Error;

use crate::{expr::tree::ParseError, value::extract::EvalError};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TemplateParseError {
	#[error("unclosed `{{{{` tag opened at index {position}")]
	UnclosedTag { position: usize },

	#[error("empty substitution at index {position}")]
	EmptySubstitution { position: usize },

	#[error("malformed `for` header {source_str:?} at index {position}")]
	MalformedFor {
		position: usize,
		source_str: SmartString<LazyCompact>,
	},

	#[error("invalid loop variable {str:?} at index {position}")]
	InvalidLoopVar {
		position: usize,
		str: SmartString<LazyCompact>,
	},

	#[error("malformed `if` header {source_str:?} at index {position}")]
	MalformedIf {
		position: usize,
		source_str: SmartString<LazyCompact>,
	},

	#[error("malformed `let` header {source_str:?} at index {position}")]
	MalformedLet {
		position: usize,
		source_str: SmartString<LazyCompact>,
	},

	#[error("invalid `let` name {str:?} at index {position}")]
	InvalidLetVar {
		position: usize,
		str: SmartString<LazyCompact>,
	},

	#[error("`endfor` at index {position} has no matching `for`")]
	UnmatchedEndfor { position: usize },

	#[error("`for` loop opened at index {position} is never closed with `endfor`")]
	UnclosedForLoop { position: usize },

	#[error("`else` at index {position} has no matching `if`")]
	UnmatchedElse { position: usize },

	#[error("`endif` at index {position} has no matching `if`")]
	UnmatchedEndif { position: usize },

	#[error("`if` block opened at index {position} is never closed with `endif`")]
	UnclosedIf { position: usize },
}

//
// MARK: expand
//

/// A failure while expanding a template. Every variant is typed so callers can
/// react to the specific cause — in particular, a source's own failures surface
/// through [`ExpandError::extract_error`] (e.g. [`ExtractError::HttpStatus`] /
/// [`ExtractError::HttpUnreachable`]) with no downcasting required.
#[derive(Debug, Error)]
pub enum ExpandError {
	#[error("parsing template")]
	TemplateParse(#[from] TemplateParseError),

	#[error("invalid expression {expr:?}")]
	ExprParse {
		expr: String,

		#[source]
		source: ParseError,
	},

	#[error("evaluating {expr:?}")]
	Eval {
		expr: String,
		#[source]
		source: EvalError,
	},

	/// Reading a `for` iterable's length or one of its elements failed.
	#[error("{what}")]
	Iterate {
		what: &'static str,

		#[source]
		source: EvalError,
	},

	/// Serializing a substitution's evaluated value to text failed.
	#[error("serializing value")]
	Serialize(#[source] EvalError),
}

impl ExpandError {
	pub fn extract_error(&self) -> Option<&EvalError> {
		match self {
			ExpandError::Eval { source, .. }
			| ExpandError::Iterate { source, .. }
			| ExpandError::Serialize(source) => Some(source),
			ExpandError::TemplateParse(_) | ExpandError::ExprParse { .. } => None,
		}
	}
}
