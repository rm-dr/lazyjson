use core::fmt;
use serde::{
	Deserialize, Deserializer,
	de::{self, Visitor},
};
use smartstring::{LazyCompact, SmartString};
use std::str::FromStr;
use thiserror::Error;

use crate::{
	Label,
	expr::{
		operator::BinOp,
		quote::unquote,
		tokenize::{Token, Tokenizer},
	},
	value::{ELazyValue, LazyValue},
};

#[derive(Debug, Error)]
pub enum ParseError {
	#[error("the left neighbor of an operator must be a value")]
	OperatorBadLeft,

	#[error("the right neighbor of an operator must be a value")]
	OperatorBadRight,

	#[error("invalid operator: {0}")]
	InvalidOperator(SmartString<LazyCompact>),

	#[error("invalid number: {0}")]
	InvalidNumber(SmartString<LazyCompact>),

	#[error("expression is empty")]
	EmptyExpression,

	#[error("malformed expression")]
	Malformed,

	#[error("unexpected character: {0}")]
	Unexpected(SmartString<LazyCompact>),

	#[error("invalid alias {0:?}")]
	InvalidAlias(SmartString<LazyCompact>),

	#[error("invalid field {0:?}")]
	InvalidField(SmartString<LazyCompact>),

	#[error("invalid index {0:?}")]
	InvalidIndex(SmartString<LazyCompact>),
}

#[derive(Debug, PartialEq, Eq)]
enum TmpTree<'a> {
	Token(Token<'a>),
	Tree(LazyExpressionTree),
	Void,
}

/// Convert a leaf token into a tree node.
fn leaf(token: &Token<'_>) -> Result<LazyExpressionTree, ParseError> {
	Ok(match token {
		Token::LitNum(x) => {
			let x = x.trim();
			let value = if let Ok(i) = x.parse::<i64>() {
				LazyValue::I64(i)
			} else if let Ok(u) = x.parse::<u64>() {
				LazyValue::U64(u)
			} else if let Ok(f) = x.parse::<f64>() {
				LazyValue::F64(f)
			} else {
				return Err(ParseError::InvalidNumber(x.into()));
			};
			LazyExpressionTree::Literal(value.into())
		}

		Token::LitStr(x) => {
			let value = LazyValue::String(SmartString::from(unquote(x).as_str()).into());
			LazyExpressionTree::Literal(value.into())
		}

		Token::Segment(x) => {
			let x = x.trim();
			match x {
				"@" => LazyExpressionTree::Root,
				_ if x.starts_with('@') => return Err(ParseError::InvalidAlias(x.into())),
				_ => LazyExpressionTree::Alias(
					Label::new(x).ok_or_else(|| ParseError::InvalidAlias(x.into()))?,
				),
			}
		}

		Token::SegSep
		| Token::Slice(_)
		| Token::BinOp(_)
		| Token::OpenParen
		| Token::CloseParen
		| Token::Unexpected(_) => return Err(ParseError::Malformed),
	})
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LazyExpressionTree {
	Root,
	Alias(Label),
	Literal(ELazyValue),

	Binop {
		left: Box<LazyExpressionTree>,
		op: BinOp,
		right: Box<LazyExpressionTree>,
	},

	Field {
		left: Box<LazyExpressionTree>,
		field: Label,
		args: Option<SmartString<LazyCompact>>,
	},

	Index {
		left: Box<LazyExpressionTree>,
		idx: Box<LazyExpressionTree>,
	},

	Slice {
		left: Box<LazyExpressionTree>,
		start: Option<i64>,
		end: Option<i64>,
		inclusive: bool,
	},
}

impl<'de> Deserialize<'de> for LazyExpressionTree {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		struct PathVisitor;

		impl Visitor<'_> for PathVisitor {
			type Value = LazyExpressionTree;

			fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str("an objectexpr")
			}

			fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
				LazyExpressionTree::parse(v).map_err(de::Error::custom)
			}
		}

		deserializer.deserialize_str(PathVisitor)
	}
}

impl FromStr for LazyExpressionTree {
	type Err = ParseError;
	fn from_str(s: &str) -> Result<Self, Self::Err> {
		return Self::parse(s);
	}
}

impl LazyExpressionTree {
	pub fn parse(source: &str) -> Result<Self, ParseError> {
		let tk = Tokenizer::new(source);
		let oe = LazyExpressionTree::build(tk.map(|(_, t)| t).collect())?;
		return Ok(oe);
	}

	pub fn build(tokens: Vec<Token<'_>>) -> Result<Self, ParseError> {
		if let Some(t) = tokens.iter().find(|t| matches!(t, Token::Unexpected(_))) {
			return Err(ParseError::Unexpected(t.to_string().into()));
		}

		let mut tmp_tree: Vec<_> = tokens.into_iter().map(TmpTree::Token).collect();

		// Grouping depth of each operand
		let depth = {
			let mut depth: Vec<(usize, u32)> = Vec::with_capacity(tmp_tree.len());
			let mut d = 0;
			for (i, t) in tmp_tree.iter().enumerate() {
				match t {
					TmpTree::Token(Token::OpenParen) => {
						depth.push((i, d));
						d += 1;
					}
					// A `)` with no `(` to close closes nothing; the expression
					// is malformed, not a panic.
					TmpTree::Token(Token::CloseParen) => {
						d = d.checked_sub(1).ok_or(ParseError::Malformed)?;
						depth.push((i, d));
					}
					_ => depth.push((i, d)),
				}
			}
			depth
		};

		let max_depth = depth.iter().map(|x| x.1).max().unwrap_or(0);

		for x in tmp_tree.iter_mut() {
			if matches!(x, TmpTree::Token(Token::OpenParen | Token::CloseParen)) {
				*x = TmpTree::Void;
			}
		}

		for d in (0..=max_depth).rev() {
			for c in depth.chunk_by(|a, b| a.1 >= d && b.1 >= d) {
				if c.is_empty() || c[0].1 < d {
					continue;
				}

				let start = c[0].0;
				let end = c[c.len() - 1].0;
				treeify(&mut tmp_tree[start..=end])?;
			}
		}

		// Trim ending voids
		while tmp_tree.last() == Some(&TmpTree::Void) {
			let _ = tmp_tree.pop();
		}

		let out = tmp_tree.pop();

		// Trim starting voids
		while tmp_tree.last() == Some(&TmpTree::Void) {
			let _ = tmp_tree.pop();
		}

		// Anything left over is two operands with nothing joining them, e.g.
		// `@.a @.b` — a malformed expression, not a panic.
		if !tmp_tree.is_empty() {
			return Err(ParseError::Malformed);
		}

		let out = match out {
			// Nothing but whitespace / parens: no expression at all.
			None => return Err(ParseError::EmptyExpression),
			Some(TmpTree::Void) => unreachable!(),
			Some(TmpTree::Tree(x)) => x,
			// A lone value that no path or operator was applied to.
			Some(TmpTree::Token(t)) => leaf(&t)?,
		};

		return Ok(out);
	}
}

#[inline(always)]
fn left_of(slice: &mut [TmpTree<'_>], idx: usize) -> Option<usize> {
	let mut i = idx.checked_sub(1);

	while let Some(x) = i
		&& slice.get(x) == Some(&TmpTree::Void)
	{
		i = i.and_then(|x| x.checked_sub(1));
	}

	return i;
}

#[inline(always)]
fn right_of(slice: &mut [TmpTree<'_>], idx: usize) -> Option<usize> {
	let mut i = idx.checked_add(1);

	while let Some(x) = i
		&& slice.get(x) == Some(&TmpTree::Void)
	{
		i = i.and_then(|x| x.checked_add(1));
	}

	if let Some(x) = i
		&& x >= slice.len()
	{
		return None;
	}

	return i;
}

/// True if this entry can stand as an operand, i.e. if `take` will turn it
/// into a tree node.
#[inline(always)]
fn is_operand(x: &TmpTree<'_>) -> bool {
	matches!(
		x,
		TmpTree::Tree(_) | TmpTree::Token(Token::Segment(_) | Token::LitNum(_) | Token::LitStr(_))
	)
}

/// Move the node at `idx` out of `slice`, leaving a [TmpTree::Void] behind.
/// Callers must have checked [is_operand] first.
fn take(slice: &mut [TmpTree<'_>], idx: usize) -> Result<LazyExpressionTree, ParseError> {
	match std::mem::replace(&mut slice[idx], TmpTree::Void) {
		TmpTree::Tree(x) => Ok(x),
		TmpTree::Token(t) => leaf(&t),
		TmpTree::Void => unreachable!(),
	}
}

/// Parse a segment token into a field name and its optional arguments,
/// handling the `name(args)` form. Parens inside args may be nested; `\(` and
/// `\)` are escaped and do not affect depth counting.
fn parse_field(ident: &str) -> Result<(Label, Option<SmartString<LazyCompact>>), ParseError> {
	let ident = ident.trim();
	let bytes = ident.as_bytes();
	let mut i = 0;

	// Find the first unescaped '(' — everything before it is the name.
	let open_paren: Option<usize> = loop {
		if i >= bytes.len() {
			break None;
		}
		match bytes[i] {
			b'\\' => i += 2, // skip escaped character
			b'(' => break Some(i),
			_ => i += 1,
		}
	};

	let name_str = ident[..open_paren.unwrap_or(bytes.len())].trim();
	let name = Label::new(name_str).ok_or_else(|| ParseError::InvalidField(name_str.into()))?;

	let Some(open_pos) = open_paren else {
		return Ok((name, None));
	};

	// Scan args, tracking paren depth.
	let args_start = open_pos + 1;
	let mut depth: usize = 1;
	let mut j = args_start;

	while j < bytes.len() {
		match bytes[j] {
			b'\\' => j += 2, // skip escaped character
			b'(' => {
				depth += 1;
				j += 1;
			}
			b')' => {
				depth -= 1;
				if depth == 0 {
					// Closing paren must be the last character.
					if j + 1 != bytes.len() {
						return Err(ParseError::InvalidField(ident.into()));
					}
					return Ok((name, Some(ident[args_start..j].into())));
				}
				j += 1;
			}
			_ => j += 1,
		}
	}

	// Reached the end of the ident without finding the matching ')'.
	return Err(ParseError::InvalidField(ident.into()));
}

/// Apply a slice token — `[0]`, `[-1]`, `[1..2]`, `[1..=2]`, `[1..]`, `[..1]`,
/// `[..]` — to the node it follows, producing an [LazyExpressionTree::Index]
/// or [LazyExpressionTree::Slice].
///
/// `slice` is the raw token, brackets included.
fn apply_slice(left: LazyExpressionTree, slice: &str) -> Result<LazyExpressionTree, ParseError> {
	// `read_slice` stops at the end of the source if the group never closes.
	let inner = slice
		.strip_prefix('[')
		.and_then(|x| x.strip_suffix(']'))
		.ok_or_else(|| ParseError::InvalidIndex(slice.into()))?;

	// No `..` makes this a plain index. Its contents are an expression of
	// their own, so `[0]` and `[@.i]` are both indices; an empty `[]` lands
	// here and fails, which is what we want — it is not an open range.
	let Some((start, end)) = inner.split_once("..") else {
		if inner.trim().is_empty() {
			return Err(ParseError::InvalidIndex(inner.into()));
		}

		return Ok(LazyExpressionTree::Index {
			left: Box::new(left),
			idx: Box::new(LazyExpressionTree::parse(inner)?),
		});
	};

	// `[..=2]` and `[1..=2]`. An inclusive range needs an end to include.
	let (end, inclusive) = match end.trim_start().strip_prefix('=') {
		Some(x) => (x, true),
		None => (end, false),
	};

	let bound =
		|x: &str| i64::from_str(x.trim()).map_err(|_err| ParseError::InvalidIndex(inner.into()));

	let start = (!start.trim().is_empty())
		.then(|| bound(start))
		.transpose()?;
	let end = (!end.trim().is_empty()).then(|| bound(end)).transpose()?;

	if inclusive && end.is_none() {
		return Err(ParseError::InvalidIndex(inner.into()));
	}

	return Ok(LazyExpressionTree::Slice {
		left: Box::new(left),
		start,
		end,
		inclusive,
	});
}

/// Fold one group of tokens into a tree, in place.
///
/// Path syntax binds tighter than any operator, so `.field` and `[idx]` are
/// applied first; what is left is a flat sequence of operands and operators.
fn treeify(slice: &mut [TmpTree<'_>]) -> Result<(), ParseError> {
	apply_paths(slice)?;
	apply_ops(slice)?;
	return Ok(());
}

/// Apply every `.field` and `[idx]` in `slice` to the value it follows,
/// left to right.
fn apply_paths(slice: &mut [TmpTree<'_>]) -> Result<(), ParseError> {
	// The node a `.` or `[` applies to: the value we last passed, if any.
	let mut base: Option<usize> = None;
	let mut i = 0;

	while i < slice.len() {
		match &slice[i] {
			// A group that has already been folded, or a value that nothing
			// has been applied to yet.
			TmpTree::Tree(_)
			| TmpTree::Token(Token::Segment(_) | Token::LitNum(_) | Token::LitStr(_)) => {
				base = Some(i);
			}

			// `.name` — the name arrives as the token after the separator.
			TmpTree::Token(Token::SegSep) => {
				let b = base.ok_or(ParseError::Malformed)?;
				let name = right_of(slice, i).ok_or(ParseError::Malformed)?;

				let (field, args) = match &slice[name] {
					TmpTree::Token(Token::Segment(x)) => parse_field(x)?,

					// An expression has number literals, a path does not:
					// `@.5` names the field `5`.
					TmpTree::Token(Token::LitNum(x)) => {
						let x = x.trim();
						let label =
							Label::new(x).ok_or_else(|| ParseError::InvalidField(x.into()))?;
						(label, None)
					}

					_ => return Err(ParseError::Malformed),
				};

				let left = take(slice, b)?;
				slice[i] = TmpTree::Void;
				slice[name] = TmpTree::Tree(LazyExpressionTree::Field {
					left: Box::new(left),
					field,
					args,
				});

				// The field we just built is what a following `.` or `[`
				// applies to; everything up to it is consumed.
				base = Some(name);
				i = name + 1;
				continue;
			}

			// `[0]`, `[1..=2]`, ... — an index needs no separator before it.
			TmpTree::Token(Token::Slice(x)) => {
				let x = *x;
				let b = base.ok_or(ParseError::Malformed)?;
				let left = take(slice, b)?;
				slice[i] = TmpTree::Tree(apply_slice(left, x)?);
				base = Some(i);
			}

			// An operator ends the path on its left; the value after it
			// starts a new one.
			TmpTree::Token(Token::BinOp(_)) => base = None,

			TmpTree::Void => {}

			// Parens are voided before `treeify` runs, and an unexpected
			// token fails `build` before it gets here.
			TmpTree::Token(Token::OpenParen | Token::CloseParen | Token::Unexpected(_)) => {
				return Err(ParseError::Malformed);
			}
		}

		i += 1;
	}

	return Ok(());
}

/// Apply every operator in `slice` to its neighbors, tightest binding first.
fn apply_ops(slice: &mut [TmpTree<'_>]) -> Result<(), ParseError> {
	let mut ops: Vec<Option<usize>> = slice
		.iter()
		.enumerate()
		.filter(|(_, x)| matches!(x, TmpTree::Token(Token::BinOp(_))))
		.map(|x| Some(x.0))
		.collect();
	let mut n_ops = ops.len();

	// The operator at `idx`, if there is one there.
	fn op_at(slice: &[TmpTree<'_>], idx: Option<usize>) -> Result<Option<BinOp>, ParseError> {
		match idx.map(|x| &slice[x]) {
			None => Ok(None),
			Some(TmpTree::Token(Token::BinOp(x))) => {
				Ok(Some(BinOp::from_str(x).map_err(|_err| {
					ParseError::InvalidOperator((*x).into())
				})?))
			}
			// A neighbor that is a value, not an operator: two operands with
			// nothing joining them.
			_ => Err(ParseError::Malformed),
		}
	}

	while n_ops != 0 {
		for o_idx in ops.iter_mut() {
			let Some(idx) = *o_idx else {
				continue;
			};

			let l_idx = left_of(slice, idx).ok_or(ParseError::OperatorBadLeft)?;
			let ll_idx = left_of(slice, l_idx);
			let r_idx = right_of(slice, idx).ok_or(ParseError::OperatorBadRight)?;
			let rr_idx = right_of(slice, r_idx);

			if !is_operand(&slice[l_idx]) {
				return Err(ParseError::OperatorBadLeft);
			}

			if !is_operand(&slice[r_idx]) {
				return Err(ParseError::OperatorBadRight);
			}

			#[expect(clippy::unwrap_used)]
			let c = op_at(slice, Some(idx))?.unwrap();

			// The operators on either side of our operands. Ours applies only
			// if it binds at least as tightly as both of them; if it does not,
			// a later pass folds it, once the tighter one has taken its
			// operand.
			let ll = op_at(slice, ll_idx)?;
			let rr = op_at(slice, rr_idx)?;

			let win_left = ll
				.map(|x| x.bind_power().0 <= c.bind_power().1)
				.unwrap_or(true);
			let win_right = rr
				.map(|x| x.bind_power().1 <= c.bind_power().0)
				.unwrap_or(true);

			if win_left && win_right {
				let left = take(slice, l_idx)?;
				let right = take(slice, r_idx)?;

				slice[idx] = TmpTree::Tree(LazyExpressionTree::Binop {
					left: Box::new(left),
					op: c,
					right: Box::new(right),
				});

				*o_idx = None;
				n_ops -= 1;
			}
		}
	}

	return Ok(());
}

//
// MARK: tests
//

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
	use smartstring::SmartString;

	use super::{LazyExpressionTree, ParseError};
	use crate::Label;
	use crate::expr::operator::BinOp;
	use crate::expr::tokenize::Token;
	use crate::value::LazyValue;

	//
	// MARK: helpers
	//

	/// Build a `Tree` from a token stream, unwrapping the result.
	fn build(tokens: Vec<Token<'_>>) -> LazyExpressionTree {
		LazyExpressionTree::build(tokens).unwrap()
	}

	/// Expected `Tree::Literal` leaf.
	fn lit(s: &str) -> LazyExpressionTree {
		LazyExpressionTree::Literal(LazyValue::String(SmartString::from(s).into()).into())
	}

	/// Expected `Tree::Literal` integer leaf.
	fn num(x: i64) -> LazyExpressionTree {
		LazyExpressionTree::Literal(LazyValue::I64(x).into())
	}

	/// Expected `Tree::Binop` node.
	fn binop(left: LazyExpressionTree, op: BinOp, right: LazyExpressionTree) -> LazyExpressionTree {
		LazyExpressionTree::Binop {
			left: Box::new(left),
			op,
			right: Box::new(right),
		}
	}

	/// Expected `Tree::Field` node.
	fn field(left: LazyExpressionTree, name: &str) -> LazyExpressionTree {
		LazyExpressionTree::Field {
			left: Box::new(left),
			field: Label::new(name).unwrap(),
			args: None,
		}
	}

	/// Expected `Tree::Field` node, with args.
	fn field_args(left: LazyExpressionTree, name: &str, args: &str) -> LazyExpressionTree {
		LazyExpressionTree::Field {
			left: Box::new(left),
			field: Label::new(name).unwrap(),
			args: Some(args.into()),
		}
	}

	/// Expected `Tree::Index` node.
	fn index(left: LazyExpressionTree, idx: LazyExpressionTree) -> LazyExpressionTree {
		LazyExpressionTree::Index {
			left: Box::new(left),
			idx: Box::new(idx),
		}
	}

	/// Expected `Tree::Slice` node.
	fn slice(
		left: LazyExpressionTree,
		start: Option<i64>,
		end: Option<i64>,
		inclusive: bool,
	) -> LazyExpressionTree {
		LazyExpressionTree::Slice {
			left: Box::new(left),
			start,
			end,
			inclusive,
		}
	}

	/// The tree a dotted path like `@.a.b[0].c` is expected to parse to.
	///
	/// Deliberately dumb — it splits on `.` and `[`, and knows nothing about
	/// operators, literals, or field args — so a test's expected value does
	/// not lean on the parser under test.
	fn path(src: &str) -> LazyExpressionTree {
		let mut out: Option<LazyExpressionTree> = None;
		let mut rest = src;

		while !rest.is_empty() {
			// `[...]` applies to whatever we have so far.
			if let Some(x) = rest.strip_prefix('[') {
				let (inner, tail) = x.split_once(']').unwrap();
				let left = out.take().unwrap();
				rest = tail;

				out = Some(match inner.split_once("..") {
					None => index(left, num(inner.parse().unwrap())),
					Some((start, end)) => {
						let (end, inclusive) = match end.strip_prefix('=') {
							Some(x) => (x, true),
							None => (end, false),
						};
						slice(left, start.parse().ok(), end.parse().ok(), inclusive)
					}
				});

				continue;
			}

			// `name`, or `.name` for all but the first segment.
			let x = match &out {
				None => rest,
				Some(_) => rest.strip_prefix('.').unwrap(),
			};
			let split = x.find(['.', '[']).unwrap_or(x.len());
			let (name, tail) = x.split_at(split);
			rest = tail;

			out = Some(match out {
				None if name == "@" => LazyExpressionTree::Root,
				None => LazyExpressionTree::Alias(Label::new(name).unwrap()),
				Some(left) => field(left, name),
			});
		}

		out.unwrap()
	}

	fn p(s: &str) -> Token<'_> {
		Token::Segment(s)
	}
	/// A bare value token — a path root (`@`) or an alias reference.
	fn v(s: &str) -> Token<'_> {
		Token::Segment(s)
	}
	fn n(s: &str) -> Token<'_> {
		Token::LitNum(s)
	}
	fn q(s: &str) -> Token<'_> {
		Token::LitStr(s)
	}
	fn o(s: &str) -> Token<'_> {
		Token::BinOp(s)
	}
	/// An index / slice token — `[0]`, `[1..=2]`, ...
	fn sl(s: &str) -> Token<'_> {
		Token::Slice(s)
	}
	const OPEN: Token<'static> = Token::OpenParen;
	const CLOSE: Token<'static> = Token::CloseParen;
	const SEP: Token<'static> = Token::SegSep;

	//
	// MARK: the path helper
	//

	#[test]
	fn path_helper_builds_what_it_says() {
		// The helper every path test leans on, spelled out once.
		assert_eq!(path("@"), LazyExpressionTree::Root);
		assert_eq!(path("@.a"), field(LazyExpressionTree::Root, "a"));
		assert_eq!(
			path("@.a.b[0].c"),
			field(
				index(field(field(LazyExpressionTree::Root, "a"), "b"), num(0)),
				"c"
			),
		);
		assert_eq!(
			path("x[1..=2]"),
			slice(
				LazyExpressionTree::Alias(Label::new("x").unwrap()),
				Some(1),
				Some(2),
				true
			),
		);
	}

	//
	// MARK: single leaves
	//

	#[test]
	fn single_path() {
		assert_eq!(build(vec![p("@"), SEP, v("a")]), path("@.a"));
	}

	#[test]
	fn single_path_root_only() {
		assert_eq!(build(vec![p("@")]), path("@"));
	}

	#[test]
	fn single_path_nested() {
		assert_eq!(
			build(vec![
				p("@"),
				SEP,
				v("a"),
				SEP,
				v("b"),
				sl("[0]"),
				SEP,
				v("c")
			]),
			path("@.a.b[0].c")
		);
	}

	#[test]
	fn root_with_args_is_not_an_alias() {
		assert!(matches!(
			LazyExpressionTree::build(vec![v("@a")]),
			Err(ParseError::InvalidAlias(_)),
		));
	}

	#[test]
	fn slice_without_a_base_is_malformed() {
		assert!(matches!(
			LazyExpressionTree::build(vec![sl("[0]")]),
			Err(ParseError::Malformed),
		));
	}

	#[test]
	fn separator_without_a_base_is_malformed() {
		// A leading `.` is valid as a *relative* path, never as an expression.
		assert!(matches!(
			LazyExpressionTree::build(vec![SEP, v("a")]),
			Err(ParseError::Malformed),
		));
	}

	#[test]
	fn separator_without_a_field_is_malformed() {
		assert!(matches!(
			"@.".parse::<LazyExpressionTree>(),
			Err(ParseError::Malformed),
		));

		assert!(matches!(
			"@.a..b".parse::<LazyExpressionTree>(),
			Err(ParseError::Malformed),
		));
	}

	#[test]
	fn every_slice_form_parses() {
		// End to end, through the tokenizer, for each shape `Token::Slice` covers.
		for src in [
			"[0]", "[-1]", "[1..2]", "[1..]", "[..1]", "[1..=2]", "[..=2]",
		] {
			let src = format!("@.a{src}");
			assert_eq!(
				src.parse::<LazyExpressionTree>().unwrap(),
				path(&src),
				"while parsing {src}"
			);
		}
	}

	#[test]
	fn fully_open_slice_parses() {
		assert_eq!(
			"@.a[..]".parse::<LazyExpressionTree>().unwrap(),
			slice(path("@.a"), None, None, false),
		);
	}

	#[test]
	fn chained_slices_apply_left_to_right() {
		assert_eq!(
			build(vec![p("@"), SEP, v("a"), sl("[0]"), sl("[1]")]),
			index(index(path("@.a"), num(0)), num(1)),
		);
	}

	#[test]
	fn slice_on_root() {
		assert_eq!(build(vec![p("@"), sl("[0]")]), path("@[0]"));
	}

	#[test]
	fn index_contents_are_an_expression() {
		// `idx` is a tree, so an index is not limited to a literal.
		assert_eq!(
			"@.a[@.i]".parse::<LazyExpressionTree>().unwrap(),
			index(path("@.a"), path("@.i")),
		);
	}

	#[test]
	fn empty_index_is_an_error() {
		// `[]` is not a valid (open) range — it has no `..`.
		assert!(matches!(
			"@.a[]".parse::<LazyExpressionTree>(),
			Err(ParseError::InvalidIndex(_)),
		));
	}

	#[test]
	fn non_numeric_slice_bound_is_an_error() {
		assert!(matches!(
			"@.a[x..2]".parse::<LazyExpressionTree>(),
			Err(ParseError::InvalidIndex(_)),
		));
	}

	#[test]
	fn inclusive_slice_needs_an_end() {
		assert!(matches!(
			"@.a[1..=]".parse::<LazyExpressionTree>(),
			Err(ParseError::InvalidIndex(_)),
		));
	}

	#[test]
	fn literal_with_slice_is_subpath() {
		// An index applies to the literal with no separator before it.
		assert_eq!(
			"'abc'[0]".parse::<LazyExpressionTree>().unwrap(),
			index(lit("abc"), num(0)),
		);
	}

	#[test]
	fn bare_word_is_an_alias() {
		// Bare values are never literals; a name roots an expression at an
		// alias, left unresolved until the expression is evaluated.
		assert_eq!(build(vec![v("artist")]), path("artist"));
	}

	#[test]
	fn alias_root_must_be_a_label() {
		assert!(matches!(
			LazyExpressionTree::build(vec![v("foo(1)"), SEP, v("bar")]),
			Err(ParseError::InvalidAlias(_)),
		));
	}

	#[test]
	fn alias_extends_with_trailing_field() {
		assert_eq!(
			build(vec![v("artist"), SEP, v("lower")]),
			path("artist.lower"),
		);
	}

	#[test]
	fn alias_extends_with_trailing_index() {
		assert_eq!(
			build(vec![v("tags"), sl("[0]"), SEP, v("name")]),
			path("tags[0].name"),
		);
	}

	#[test]
	fn alias_in_binop() {
		assert_eq!(
			build(vec![v("artist"), SEP, v("lower"), o("=="), q("'bon jovi'")]),
			binop(path("artist.lower"), BinOp::Eq, lit("bon jovi")),
		);
	}

	#[test]
	fn single_literal_number() {
		assert_eq!(build(vec![n("12")]), num(12));
	}

	#[test]
	fn single_literal_float_falls_back_to_string() {
		// `LazyValue` has no float variant, so floats become strings.
		assert_eq!(build(vec![n("1.5")]), lit("1.5"));
	}

	//
	// MARK: fields
	//

	#[test]
	fn field_with_args() {
		assert_eq!(
			"@.foo(bar)".parse::<LazyExpressionTree>().unwrap(),
			field_args(path("@"), "foo", "bar"),
		);
	}

	#[test]
	fn field_with_empty_args() {
		assert_eq!(
			"@.foo()".parse::<LazyExpressionTree>().unwrap(),
			field_args(path("@"), "foo", ""),
		);
	}

	#[test]
	fn field_with_nested_parens_in_args() {
		assert_eq!(
			"@.foo(a(b(c))d)".parse::<LazyExpressionTree>().unwrap(),
			field_args(path("@"), "foo", "a(b(c))d"),
		);
	}

	#[test]
	fn field_with_escaped_parens_in_args() {
		// `\(` and `\)` are escaped, and do not move the arg depth.
		assert_eq!(
			r"@.foo(a\(b\)c)".parse::<LazyExpressionTree>().unwrap(),
			field_args(path("@"), "foo", r"a\(b\)c"),
		);
	}

	#[test]
	fn field_args_over_multiple_segments() {
		assert_eq!(
			"@.foo(x).bar(y)".parse::<LazyExpressionTree>().unwrap(),
			field_args(field_args(path("@"), "foo", "x"), "bar", "y"),
		);
	}

	#[test]
	fn field_args_unclosed_paren_is_an_error() {
		assert!(matches!(
			"@.foo(bar".parse::<LazyExpressionTree>(),
			Err(ParseError::InvalidField(_)),
		));
	}

	#[test]
	fn invalid_field_chars_are_an_error() {
		assert!(matches!(
			"@.a b".parse::<LazyExpressionTree>(),
			Err(ParseError::InvalidField(_)),
		));
	}

	#[test]
	fn numeric_field_names() {
		// An expression has number literals, a path does not: after a `.`,
		// `5` names a field.
		assert_eq!("@.5".parse::<LazyExpressionTree>().unwrap(), path("@.5"),);
	}

	#[test]
	fn field_on_a_number_literal() {
		// `5.add(6)` — the `.` is a separator, not a decimal point.
		assert_eq!(
			"5.add(6)".parse::<LazyExpressionTree>().unwrap(),
			field_args(num(5), "add", "6"),
		);
	}

	#[test]
	fn field_on_a_group() {
		assert_eq!(
			"(@.a | @.b).lower".parse::<LazyExpressionTree>().unwrap(),
			field(binop(path("@.a"), BinOp::Or, path("@.b")), "lower"),
		);
	}

	#[test]
	fn slice_on_a_group() {
		assert_eq!(
			"(@.a | @.b)[0]".parse::<LazyExpressionTree>().unwrap(),
			index(binop(path("@.a"), BinOp::Or, path("@.b")), num(0)),
		);
	}

	//
	// MARK: quoted literals
	//

	#[test]
	fn quoted_literal_single() {
		assert_eq!(build(vec![q("'hi there'")]), lit("hi there"));
	}

	#[test]
	fn quoted_literal_double() {
		assert_eq!(build(vec![q("\"hi there\"")]), lit("hi there"));
	}

	#[test]
	fn quoted_literal_escaped_quote() {
		// An escaped quote inside a literal is part of the value, not a closer.
		assert_eq!(build(vec![q("'foo\\''")]), lit("foo'"));
		assert_eq!(build(vec![q("\"a\\\"b\"")]), lit("a\"b"));
	}

	#[test]
	fn quoted_literal_resolves_escapes() {
		assert_eq!(build(vec![q("'a\\tb\\nc'")]), lit("a\tb\nc"));
		assert_eq!(build(vec![q("'a\\\\b'")]), lit("a\\b"));
	}

	//
	// MARK: literal with a trailing path suffix
	//

	#[test]
	fn literal_with_suffix_is_subpath() {
		// `'x'.upper` -> apply the field `upper` to the string literal.
		assert_eq!(
			build(vec![q("'x'"), SEP, v("upper")]),
			field(lit("x"), "upper"),
		);
	}

	#[test]
	fn literal_with_suffix_in_binop() {
		assert_eq!(
			build(vec![q("'x'"), SEP, v("upper"), o("=="), q("'X'")]),
			binop(field(lit("x"), "upper"), BinOp::Eq, lit("X")),
		);
	}

	//
	// MARK: simple binops
	//

	#[test]
	fn eq_path_literal() {
		assert_eq!(
			build(vec![p("@"), SEP, v("a"), o("=="), q("'5'")]),
			binop(path("@.a"), BinOp::Eq, lit("5")),
		);
	}

	#[test]
	fn neq_path_literal() {
		assert_eq!(
			build(vec![p("@"), SEP, v("a"), o("!="), q("'5'")]),
			binop(path("@.a"), BinOp::Neq, lit("5")),
		);
	}

	#[test]
	fn match_path_quoted() {
		assert_eq!(
			build(vec![p("@"), SEP, v("title"), o("~="), q("\"the*\"")]),
			binop(path("@.title"), BinOp::Match, lit("the*")),
		);
	}

	#[test]
	fn or_two_paths() {
		assert_eq!(
			"@.a | @.b".parse::<LazyExpressionTree>().unwrap(),
			binop(path("@.a"), BinOp::Or, path("@.b")),
		);
	}

	#[test]
	fn logor_two_paths() {
		assert_eq!(
			"@.a || @.b".parse::<LazyExpressionTree>().unwrap(),
			binop(path("@.a"), BinOp::LogOr, path("@.b")),
		);
	}

	#[test]
	fn logand_two_paths() {
		assert_eq!(
			"@.a && @.b".parse::<LazyExpressionTree>().unwrap(),
			binop(path("@.a"), BinOp::LogAnd, path("@.b")),
		);
	}

	#[test]
	fn gte_path_number() {
		assert_eq!(
			"@.a >= 5".parse::<LazyExpressionTree>().unwrap(),
			binop(path("@.a"), BinOp::Gte, num(5)),
		);
	}

	#[test]
	fn comparison_binds_tighter_than_logand() {
		// `@.w >= 0.7 && @.c >= 0.7` == `(@.w >= 0.7) && (@.c >= 0.7)`.
		// The `0.7` float literals fall back to string leaves (no float variant),
		// then coerce back to numbers at eval time.
		assert_eq!(
			"@.w >= 0.7 && @.c >= 0.7"
				.parse::<LazyExpressionTree>()
				.unwrap(),
			binop(
				binop(path("@.w"), BinOp::Gte, lit("0.7")),
				BinOp::LogAnd,
				binop(path("@.c"), BinOp::Gte, lit("0.7")),
			),
		);
	}

	//
	// MARK: precedence
	//

	#[test]
	fn path_binds_tighter_than_any_operator() {
		// `@.a == @.b.c` — the `.c` belongs to `@.b`, not to the comparison.
		assert_eq!(
			"@.a == @.b.c[0]".parse::<LazyExpressionTree>().unwrap(),
			binop(path("@.a"), BinOp::Eq, path("@.b.c[0]")),
		);
	}

	#[test]
	fn or_binds_tighter_than_eq() {
		// `@.a | @.b == @.c`  ==  `(@.a | @.b) == @.c`
		assert_eq!(
			"@.a | @.b == @.c".parse::<LazyExpressionTree>().unwrap(),
			binop(
				binop(path("@.a"), BinOp::Or, path("@.b")),
				BinOp::Eq,
				path("@.c"),
			),
		);
	}

	#[test]
	fn or_binds_tighter_than_eq_on_right() {
		// `@.a == @.b | @.c`  ==  `@.a == (@.b | @.c)`
		assert_eq!(
			"@.a == @.b | @.c".parse::<LazyExpressionTree>().unwrap(),
			binop(
				path("@.a"),
				BinOp::Eq,
				binop(path("@.b"), BinOp::Or, path("@.c")),
			),
		);
	}

	#[test]
	fn eq_binds_tighter_than_logand() {
		// `@.a == @.b && @.c == @.d`  ==  `(@.a == @.b) && (@.c == @.d)`
		assert_eq!(
			"@.a == @.b && @.c == @.d"
				.parse::<LazyExpressionTree>()
				.unwrap(),
			binop(
				binop(path("@.a"), BinOp::Eq, path("@.b")),
				BinOp::LogAnd,
				binop(path("@.c"), BinOp::Eq, path("@.d")),
			),
		);
	}

	#[test]
	fn doc_example_or_then_eq() {
		// From the crate docs:
		// `@.a.b | @.a.c == 5`  ==  `(@.a.b | @.a.c) == 5`
		assert_eq!(
			"@.a.b | @.a.c == '5'"
				.parse::<LazyExpressionTree>()
				.unwrap(),
			binop(
				binop(path("@.a.b"), BinOp::Or, path("@.a.c")),
				BinOp::Eq,
				lit("5"),
			),
		);
	}

	//
	// MARK: associativity (equal precedence -> left to right)
	//

	#[test]
	fn or_is_left_associative() {
		// `@.a | @.b | @.c`  ==  `(@.a | @.b) | @.c`
		assert_eq!(
			"@.a | @.b | @.c".parse::<LazyExpressionTree>().unwrap(),
			binop(
				binop(path("@.a"), BinOp::Or, path("@.b")),
				BinOp::Or,
				path("@.c"),
			),
		);
	}

	#[test]
	fn eq_neq_are_left_associative() {
		// `@.a == @.b != @.c`  ==  `(@.a == @.b) != @.c`
		assert_eq!(
			"@.a == @.b != @.c".parse::<LazyExpressionTree>().unwrap(),
			binop(
				binop(path("@.a"), BinOp::Eq, path("@.b")),
				BinOp::Neq,
				path("@.c"),
			),
		);
	}

	#[test]
	fn logor_logand_are_left_associative() {
		// `@.a && @.b || @.c`  ==  `(@.a && @.b) || @.c`  (both bind power 1)
		assert_eq!(
			"@.a && @.b || @.c".parse::<LazyExpressionTree>().unwrap(),
			binop(
				binop(path("@.a"), BinOp::LogAnd, path("@.b")),
				BinOp::LogOr,
				path("@.c"),
			),
		);
	}

	//
	// MARK: parentheses
	//

	#[test]
	fn paren_around_single_path() {
		assert_eq!(build(vec![OPEN, p("@"), SEP, v("a"), CLOSE]), path("@.a"));
	}

	#[test]
	fn paren_around_single_literal() {
		assert_eq!(build(vec![OPEN, q("'5'"), CLOSE]), lit("5"));
	}

	#[test]
	fn redundant_nested_parens() {
		assert_eq!(
			build(vec![OPEN, OPEN, p("@"), SEP, v("a"), CLOSE, CLOSE]),
			path("@.a"),
		);
	}

	#[test]
	fn paren_around_binop() {
		assert_eq!(
			"(@.a == @.b)".parse::<LazyExpressionTree>().unwrap(),
			binop(path("@.a"), BinOp::Eq, path("@.b")),
		);
	}

	#[test]
	fn paren_overrides_precedence() {
		// `@.a | (@.b == @.c)` keeps the `==` grouped, even though `|` binds
		// tighter; without parens this would be `(@.a | @.b) == @.c`.
		assert_eq!(
			"@.a | (@.b == @.c)".parse::<LazyExpressionTree>().unwrap(),
			binop(
				path("@.a"),
				BinOp::Or,
				binop(path("@.b"), BinOp::Eq, path("@.c")),
			),
		);
	}

	#[test]
	fn paren_forces_right_grouping() {
		// `@.a == (@.b == @.c)` nests right, overriding left associativity.
		assert_eq!(
			"@.a == (@.b == @.c)".parse::<LazyExpressionTree>().unwrap(),
			binop(
				path("@.a"),
				BinOp::Eq,
				binop(path("@.b"), BinOp::Eq, path("@.c")),
			),
		);
	}

	#[test]
	fn parens_on_both_sides() {
		// `(@.a | @.b) == (@.c | @.d)`
		assert_eq!(
			"(@.a | @.b) == (@.c | @.d)"
				.parse::<LazyExpressionTree>()
				.unwrap(),
			binop(
				binop(path("@.a"), BinOp::Or, path("@.b")),
				BinOp::Eq,
				binop(path("@.c"), BinOp::Or, path("@.d")),
			),
		);
	}

	//
	// MARK: malformed input errors
	//

	#[test]
	fn empty_string_is_empty_expression_error() {
		assert!(matches!(
			"".parse::<LazyExpressionTree>(),
			Err(ParseError::EmptyExpression),
		));
	}

	#[test]
	fn whitespace_only_is_empty_expression_error() {
		assert!(matches!(
			"   ".parse::<LazyExpressionTree>(),
			Err(ParseError::EmptyExpression),
		));
	}

	#[test]
	fn empty_tokens_is_empty_expression_error() {
		assert!(matches!(
			LazyExpressionTree::build(vec![]),
			Err(ParseError::EmptyExpression),
		));
	}

	#[test]
	fn lone_open_paren_errors() {
		assert!(matches!(
			"(".parse::<LazyExpressionTree>(),
			Err(ParseError::EmptyExpression),
		));
	}

	#[test]
	fn empty_parens_error() {
		assert!(matches!(
			"()".parse::<LazyExpressionTree>(),
			Err(ParseError::EmptyExpression),
		));
	}

	#[test]
	fn lone_open_paren_tokens_error() {
		assert!(LazyExpressionTree::build(vec![OPEN]).is_err());
	}

	#[test]
	fn two_paths_without_operator_is_malformed() {
		// `(@.a)(@.b)` — two operands with nothing joining them.
		assert!(matches!(
			LazyExpressionTree::build(vec![
				OPEN,
				p("@"),
				SEP,
				v("a"),
				CLOSE,
				OPEN,
				p("@"),
				SEP,
				v("b"),
				CLOSE
			]),
			Err(ParseError::Malformed),
		));
	}

	#[test]
	fn trailing_whitespace_expression_parses() {
		// The tokenizer bound-check fix: a trailing space after a numeric or
		// operator token must not panic, and the whole thing still parses.
		assert_eq!(
			"@.a == 5 ".parse::<LazyExpressionTree>().unwrap(),
			binop(path("@.a"), BinOp::Eq, num(5)),
		);
	}

	#[test]
	fn deserialize_empty_errors_without_panic() {
		// Reachable from serde; must surface as an error, not a panic.
		let res: Result<LazyExpressionTree, _> = serde_json::from_str("\"\"");
		assert!(res.is_err());
	}

	#[test]
	fn dangling_operator_is_error_not_panic() {
		// A lone operator, and operator-only sequences, error cleanly.
		assert!(LazyExpressionTree::build(vec![o("==")]).is_err());
		assert!(LazyExpressionTree::build(vec![o("=="), o("==")]).is_err());
		assert!("@.a ==".parse::<LazyExpressionTree>().is_err());
		assert!("== @.a".parse::<LazyExpressionTree>().is_err());
		assert!("@.a == == @.b".parse::<LazyExpressionTree>().is_err());
	}

	#[test]
	fn junk_is_an_error_not_a_panic() {
		// Every four-token soup of the pieces the grammar is made of: each
		// either parses or errors, and none of them panics or hangs.
		let bits = [
			"@", ".", "a", "[", "]", "[0]", "==", "(", ")", "'x'", "5", "..", "[..=]", "f(", " ",
		];

		for a in bits {
			for b in bits {
				for c in bits {
					for d in bits {
						let src = format!("{a}{b}{c}{d}");
						let _ = src.parse::<LazyExpressionTree>();
					}
				}
			}
		}
	}

	#[test]
	fn nested_group_as_operand() {
		// `@.a && (@.b || @.c)`
		assert_eq!(
			"@.a && (@.b || @.c)".parse::<LazyExpressionTree>().unwrap(),
			binop(
				path("@.a"),
				BinOp::LogAnd,
				binop(path("@.b"), BinOp::LogOr, path("@.c")),
			),
		);
	}
}
