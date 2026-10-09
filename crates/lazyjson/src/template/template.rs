use std::collections::HashMap;

use crate::{
	Label,
	template::{ExpandError, TemplateParseError},
	value::{ELazyValue, extract::ExtractState},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateNode<'a> {
	/// Literal text
	Literal(&'a str),

	/// An `{{ <expr> }}` substitution
	Substitution(&'a str),

	/// A `{{ let <name> = <expr> }}` binding
	Let {
		/// The bound name, e.g. `u`.
		name: Label,
		/// The bound expression source, kept verbatim and unparsed.
		expr: &'a str,
	},

	/// A `{{ for <var> in <expr> }} ... {{ endfor }}` block
	ForLoop {
		var: Label,
		iterable: &'a str,
		body: Vec<TemplateNode<'a>>,
	},

	/// A `{{ if <expr> }} ... {{ else }} ... {{ endif }}` block
	If {
		cond: &'a str,
		body: Vec<TemplateNode<'a>>,
		else_body: Vec<TemplateNode<'a>>,
	},
}

/// A parsed template
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template<'a> {
	pub nodes: Vec<TemplateNode<'a>>,
}

impl<'a> TryFrom<&'a str> for Template<'a> {
	type Error = TemplateParseError;

	fn try_from(source: &'a str) -> Result<Self, Self::Error> {
		Template::parse(source)
	}
}

impl<'a> Template<'a> {
	/// Parse a template, borrowing text from `source`.
	pub fn parse(source: &'a str) -> Result<Self, TemplateParseError> {
		super::parse::parse(source)
	}

	/// Expand this template against `context`.
	pub async fn expand(
		&self,
		state: &ExtractState,
		context: ELazyValue,
	) -> Result<String, ExpandError> {
		Ok(
			super::expand::expand(&self.nodes, HashMap::new(), state, &context)
				.await?
				.text,
		)
	}
}
