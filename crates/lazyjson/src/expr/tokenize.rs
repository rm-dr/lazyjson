use std::fmt::Display;

use crate::{Label, expr::operator::BinOp, expr::quote::scan_quoted};

/// Tokens produced by a [Tokenizer]
#[derive(Debug, Eq, PartialEq)]
pub enum Token<'a> {
	/// A segment, like `@` and `content` of `@.content`
	/// or `add(6)` of `5.add(6)`
	Segment(&'a str),

	/// A `.` between segments or literals
	SegSep,

	/// An index or slice applied to whatever precedes it, brackets included.
	/// Parsed from `[0]`, `[1..2]`, `[1..=2]`, `[1..]`, `[..1]`, etc.
	/// Internal syntax is not guaranteed to be valid
	Slice(&'a str),

	/// A literal number
	///
	/// Parsed from `1`, `1.1`, `-1.1`, `001`, etc.
	LitNum(&'a str),

	/// A string literal.
	///
	/// Parsed from anything quoted by matching ', ", or """.
	LitStr(&'a str),

	/// A binary operator, like |, ==, ~= etc.
	BinOp(&'a str),

	/// Grouping open paren, like `(5 + 6) * 8`.
	/// Does not match parens for segments with args, like `5.add(6)`.
	OpenParen,

	/// Grouping close paren, like `(5 + 6) * 8`.
	/// Does not match parens for segments with args, like `5.add(6)`.
	CloseParen,

	/// A character that cannot begin a token, e.g. the `{` of `url={{ x }}`.
	Unexpected(&'a str),
}

impl Display for Token<'_> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let s = match self {
			Self::BinOp(x) => x,
			Self::CloseParen => ")",
			Self::SegSep => ".",
			Self::Segment(x) => x,
			Self::Slice(x) => x,
			Self::LitStr(x) => x,
			Self::LitNum(x) => x,
			Self::OpenParen => "(",
			Self::Unexpected(x) => x,
		};

		write!(f, "{s}")
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenizerState {
	Value,
	BinOp,
}

/// Given a string, produces an iterator of tokens.
///
/// The tokenizer is infallible, we are always able to tokenize a string.
/// Syntax is checked after tokenization.
pub struct Tokenizer<'a> {
	state: TokenizerState,
	src: &'a str,

	/// When false, `5` and `'x'` read as [`Token::Segment`]s instead of
	/// literals. A path has no literals — `@.5` selects the field named `5` —
	/// while an expression needs `5` to be a number.
	literals: bool,

	/// Current index into `str`,
	/// always on a char boundary.
	cursor: usize,
}

impl<'a> Tokenizer<'a> {
	/// Create a [Tokenizer] for an expression.
	pub fn new(src: &'a str) -> Self {
		Self {
			cursor: 0,
			src,
			state: TokenizerState::Value,
			literals: true,
		}
	}

	/// Create a [Tokenizer] for a path, where every value is a segment.
	pub fn new_path(src: &'a str) -> Self {
		Self {
			literals: false,
			..Self::new(src)
		}
	}

	/// Read `self.src` until we have a reason to stop.
	fn read_binop(&mut self) -> &'a str {
		let start = self.cursor;
		let mut end = self.cursor;
		for c in self.src[start..].chars() {
			match c {
				_ if !BinOp::is_char(c) => break,
				_ => (),
			}

			end += c.len_utf8();
		}

		self.cursor = end;
		return self.src[start..end].trim();
	}

	/// Read one path segment from `self.src`, stopping at the `.` that starts
	/// the next one (or at anything that can't be part of a segment).
	fn read_value(&mut self) -> &'a str {
		let start = self.cursor;
		let mut end = self.cursor;
		let mut paren_depth = 0;
		let mut first = true;
		// Set by a `\`, consumed by the char it escapes.
		let mut escaped = false;
		for c in self.src[self.cursor..].chars() {
			// An escaped char has no special meaning — notably `\(` and `\)`,
			// which must not move the arg depth.
			if escaped || c == '\\' {
				escaped = !escaped;
				end += c.len_utf8();
				first = false;
				continue;
			}

			// We are inside args, ignore everything
			if paren_depth != 0 {
				match c {
					')' => paren_depth -= 1,
					'(' => paren_depth += 1,
					_ => {}
				}
				end += c.len_utf8();
				continue;
			}

			match c {
				')' => break, // we are not inside arg parens, this ) is external.
				'(' => paren_depth += 1,

				// A top-level `.` or `[` ends this segment
				'.' | '[' => break,

				'@' if first => {}

				// Not a valid label char, but ignored inside a path string
				_ if c.is_whitespace() => {}

				_ if !Label::is_valid(c) => break,
				_ => (),
			}

			end += c.len_utf8();
			first = false;
		}

		self.cursor = end;
		return self.src[start..end].trim();
	}

	/// Read a numeric literal (integer or float) from `self.src`.
	///
	/// Reads a leading `-`, digits, and `.`; stops at anything else.
	fn read_num(&mut self) -> &'a str {
		let start = self.cursor;
		let bytes = self.src.as_bytes();
		let mut end = self.cursor;
		for (i, c) in self.src[start..].char_indices() {
			match c {
				'-' if i == 0 => {}
				'0'..='9' => {}

				// A `.` belongs to the number only when a digit follows it.
				// Otherwise it is a `SegSep`, as in `5.add(6)`
				'.' if bytes.get(end + 1).is_some_and(u8::is_ascii_digit) => {}

				_ => break,
			}

			end += c.len_utf8();
		}

		self.cursor = end;
		return self.src[start..end].trim();
	}

	/// Read a quoted string literal from `self.src`.
	///
	/// Returns the raw source slice, including both surrounding quotes.
	fn read_quo(&mut self) -> &'a str {
		let start = self.cursor;

		// The caller only enters here when the next char is a quote, so
		// `scan_quoted` finds the string's extent (or the end, if unterminated).
		self.cursor = start + scan_quoted(&self.src[start..]);
		return &self.src[start..self.cursor];
	}

	/// Read one index/slice group `[0]`, `[1..=2]`, `[..1]`
	/// Not guaranteed to be valid (e.g, `[bad[slice]token]`), syntax is checked later
	fn read_slice(&mut self) -> &'a str {
		let start = self.cursor;
		let mut end = self.cursor;
		let mut depth = 0u32;

		assert_eq!(
			&self.src[start..start + 1],
			"[",
			"read_slice must be called at the start of a slice"
		);

		for c in self.src[start..].chars() {
			match c {
				'[' => depth += 1,
				']' => depth -= 1,
				_ => {}
			}

			end += c.len_utf8();
			if depth == 0 {
				break;
			}
		}

		self.cursor = end;
		return &self.src[start..end];
	}
}

impl<'a> Iterator for Tokenizer<'a> {
	/// Each token, with the byte offset it starts at in `src`.
	type Item = (usize, Token<'a>);

	fn next(&mut self) -> Option<Self::Item> {
		if self.cursor >= self.src.len() {
			return None;
		}

		// Skip whitespace.
		// Some `read_*` fns stop before reading whitespace, some stop after.
		while self.cursor < self.src.len() && self.src.as_bytes()[self.cursor].is_ascii_whitespace()
		{
			self.cursor += 1;
		}

		let peek = self.src[self.cursor..].chars().next()?;

		let start = self.cursor;

		match peek {
			'.' => {
				self.cursor += peek.len_utf8();
				self.state = TokenizerState::Value;
				return Some((start, Token::SegSep));
			}

			'[' => {
				self.state = TokenizerState::BinOp;
				return Some((start, Token::Slice(self.read_slice())));
			}

			'(' => {
				self.cursor += peek.len_utf8();
				return Some((start, Token::OpenParen));
			}

			')' => {
				self.cursor += peek.len_utf8();
				return Some((start, Token::CloseParen));
			}

			_ => {}
		}

		// Are we starting a string literal?
		let is_quo = self.literals && matches!(peek, '"' | '\'');

		// Are we starting a num literal?
		let is_num = self.literals && {
			let mut chars = self.src[self.cursor..].chars();
			match chars.next() {
				Some(c) if c.is_ascii_digit() => true,
				Some('-') => matches!(chars.next(), Some(c) if c.is_ascii_digit()),
				_ => false,
			}
		};

		let out = match self.state {
			TokenizerState::BinOp => Token::BinOp(self.read_binop()),
			TokenizerState::Value if is_quo => Token::LitStr(self.read_quo()),
			TokenizerState::Value if is_num => Token::LitNum(self.read_num()),
			TokenizerState::Value => Token::Segment(self.read_value()),
		};

		if self.cursor == start {
			self.cursor += peek.len_utf8();
			// Resume in `Value` state, so what follows still tokenizes as itself
			// and the error names the one character that is actually wrong.
			self.state = TokenizerState::Value;
			return Some((start, Token::Unexpected(&self.src[start..self.cursor])));
		}

		self.state = match self.state {
			TokenizerState::BinOp => TokenizerState::Value,
			TokenizerState::Value => TokenizerState::BinOp,
		};

		return Some((start, out));
	}
}

//
// MARK: tests
//

#[cfg(test)]
mod tests {
	use super::*;
	use Token::*;

	fn toks(src: &str) -> Vec<Token<'_>> {
		Tokenizer::new(src).map(|(_, t)| t).collect()
	}

	/// Tokenize in path mode, where there are no literals.
	fn path_toks(src: &str) -> Vec<Token<'_>> {
		Tokenizer::new_path(src).map(|(_, t)| t).collect()
	}

	#[test]
	fn empty() {
		assert_eq!(toks(""), vec![]);
	}

	//
	// paths
	//

	#[test]
	fn path_simple() {
		assert_eq!(
			toks("@.content"),
			vec![Segment("@"), SegSep, Segment("content")]
		);
	}

	#[test]
	fn path_root_only() {
		assert_eq!(toks("@"), vec![Segment("@")]);
	}

	#[test]
	fn path_with_index() {
		assert_eq!(
			toks("@.a.b[0].c"),
			vec![
				Segment("@"),
				SegSep,
				Segment("a"),
				SegSep,
				Segment("b"),
				Slice("[0]"),
				SegSep,
				Segment("c")
			]
		)
	}

	#[test]
	fn path_with_slice() {
		// Every slice form reads as one token, brackets included. The chars
		// inside are not inspected here — notably the `=` of an inclusive range,
		// which would otherwise read as a `BinOp`.
		for src in [
			"[0]", "[-1]", "[1..2]", "[1..]", "[..1]", "[..]", "[1..=2]", "[..=2]",
		] {
			assert_eq!(
				toks(&format!("@.a{src}")),
				vec![Segment("@"), SegSep, Segment("a"), Slice(src)],
				"while tokenizing {src}"
			);
		}
	}

	#[test]
	fn path_with_chained_slices() {
		assert_eq!(
			toks("@.a[0][1]"),
			vec![
				Segment("@"),
				SegSep,
				Segment("a"),
				Slice("[0]"),
				Slice("[1]")
			]
		);
	}

	#[test]
	fn slice_on_root() {
		assert_eq!(toks("@[0]"), vec![Segment("@"), Slice("[0]")]);
	}

	#[test]
	fn slice_unterminated() {
		// The tokenizer is infallible; the path parser rejects this later.
		assert_eq!(
			toks("@.a[0"),
			vec![Segment("@"), SegSep, Segment("a"), Slice("[0")]
		);
	}

	#[test]
	fn inclusive_range_path_in_binop() {
		assert_eq!(
			toks("@.a[1..=2] == @.b"),
			vec![
				Segment("@"),
				SegSep,
				Segment("a"),
				Slice("[1..=2]"),
				BinOp("=="),
				Segment("@"),
				SegSep,
				Segment("b")
			],
		);
	}

	#[test]
	fn path_with_args() {
		// Parens, whitespace and binops inside args are swallowed into the path.
		assert_eq!(
			toks("@.a(b | c)"),
			vec![Segment("@"), SegSep, Segment("a(b | c)")]
		);
	}

	#[test]
	fn path_with_nested_args() {
		assert_eq!(
			toks("@.f((x))"),
			vec![Segment("@"), SegSep, Segment("f((x))")]
		)
	}

	#[test]
	fn alias_path_simple() {
		assert_eq!(
			toks("alias.content"),
			vec![Segment("alias"), SegSep, Segment("content")]
		);
	}

	#[test]
	fn alias_path_root_only() {
		assert_eq!(toks("alias"), vec![Segment("alias")]);
	}

	#[test]
	fn alias_path_with_index() {
		assert_eq!(
			toks("alias.a.b[0].c"),
			vec![
				Segment("alias"),
				SegSep,
				Segment("a"),
				SegSep,
				Segment("b"),
				Slice("[0]"),
				SegSep,
				Segment("c"),
			]
		);
	}

	#[test]
	fn operator_tab() {
		assert_eq!(
			toks("x\t=> y"),
			vec![Segment("x"), BinOp("=>"), Segment("y")]
		);
		assert_eq!(
			toks("x =>\t\ty"),
			vec![Segment("x"), BinOp("=>"), Segment("y")]
		);
	}

	#[test]
	fn comparison_operators() {
		assert_eq!(
			toks("@.a < 5"),
			vec![Segment("@"), SegSep, Segment("a"), BinOp("<"), LitNum("5")]
		);
		assert_eq!(
			toks("@.a <= 5"),
			vec![Segment("@"), SegSep, Segment("a"), BinOp("<="), LitNum("5")]
		);
		assert_eq!(
			toks("@.a > 5"),
			vec![Segment("@"), SegSep, Segment("a"), BinOp(">"), LitNum("5")]
		);
		assert_eq!(
			toks("@.a >= 5"),
			vec![Segment("@"), SegSep, Segment("a"), BinOp(">="), LitNum("5")]
		);
	}

	#[test]
	fn gte_and_then_are_distinct() {
		// `>=` (greater-or-equal) and `=>` (if-then) share chars but differ in
		// order, so the maximal-munch binop read keeps them apart.
		assert_eq!(
			toks("@.a >= 5"),
			vec![Segment("@"), SegSep, Segment("a"), BinOp(">="), LitNum("5")]
		);
		assert_eq!(
			toks("@.a => 5"),
			vec![Segment("@"), SegSep, Segment("a"), BinOp("=>"), LitNum("5")]
		);
	}

	#[test]
	fn comparison_no_spaces() {
		// `<` is not a label char, so a path read stops at it with no whitespace.
		assert_eq!(toks("w<5"), vec![Segment("w"), BinOp("<"), LitNum("5")]);
	}

	//
	// literals
	//

	#[test]
	fn lit_word() {
		assert_eq!(toks("foo"), vec![Segment("foo")]);
	}

	#[test]
	fn lit_word_quoted() {
		assert_eq!(toks("'foo'"), vec![LitStr("'foo'")]);
	}

	#[test]
	fn lit_word_quoted_escape() {
		assert_eq!(toks("'foo\\''"), vec![LitStr("'foo\\''")]);
	}

	#[test]
	fn lit_tab_start() {
		assert_eq!(toks("'\tfoo'"), vec![LitStr("'\tfoo'")]);
	}

	#[test]
	fn lit_tab_mid() {
		assert_eq!(toks("'foo\tbar'"), vec![LitStr("'foo\tbar'")]);
	}

	#[test]
	fn lit_tab_end() {
		assert_eq!(toks("'foo\t'"), vec![LitStr("'foo\t'")]);
	}

	#[test]
	fn lit_number() {
		assert_eq!(toks("12"), vec![LitNum("12")]);
	}

	#[test]
	fn lit_number_float() {
		assert_eq!(toks("1.5"), vec![LitNum("1.5")]);
	}

	#[test]
	fn lit_number_negative() {
		assert_eq!(toks("-1.1"), vec![LitNum("-1.1")]);
	}

	#[test]
	fn lit_number_leading_zeros() {
		assert_eq!(toks("001"), vec![LitNum("001")]);
	}

	#[test]
	fn lit_number_then_segment() {
		// A `.` not followed by a digit ends the number and separates it from
		// the segment that follows.
		assert_eq!(
			toks("5.add(6)"),
			vec![LitNum("5"), SegSep, Segment("add(6)")]
		);
	}

	#[test]
	fn lit_with_whitespace() {
		assert_eq!(toks("hello world"), vec![Segment("hello world")]);
	}

	//
	// quoted literals
	//

	#[test]
	fn quo_single() {
		assert_eq!(toks("'quoted lit'"), vec![LitStr("'quoted lit'")]);
	}

	#[test]
	fn quo_double() {
		assert_eq!(toks("\"dq lit\""), vec![LitStr("\"dq lit\"")]);
	}

	#[test]
	fn quo_takes_specials_verbatim() {
		// Operators and parens inside quotes are part of the literal.
		assert_eq!(toks("'a | (b)'"), vec![LitStr("'a | (b)'")]);
	}

	//
	// literals with a trailing path suffix
	//

	#[test]
	fn num_with_field_suffix() {
		assert_eq!(
			toks("6.5.upper"),
			vec![LitNum("6.5"), SegSep, Segment("upper")]
		);
	}

	#[test]
	fn lit_with_field_suffix() {
		assert_eq!(
			toks("'x'.upper"),
			vec![LitStr("'x'"), SegSep, Segment("upper")]
		);
	}

	#[test]
	fn lit_with_index_suffix() {
		// An index needs no separator before it, unlike a segment.
		assert_eq!(toks("\"abc\"[0]"), vec![LitStr("\"abc\""), Slice("[0]")]);
	}

	#[test]
	fn lit_with_chained_suffix() {
		assert_eq!(
			toks("'a b'.split(' ')[0].upper"),
			vec![
				LitStr("'a b'"),
				SegSep,
				Segment("split(' ')"),
				Slice("[0]"),
				SegSep,
				Segment("upper")
			]
		);
	}

	#[test]
	fn lit_suffix_in_binop() {
		assert_eq!(
			toks("'x'.upper == 'X'"),
			vec![
				LitStr("'x'"),
				SegSep,
				Segment("upper"),
				BinOp("=="),
				LitStr("'X'")
			],
		);
	}

	#[test]
	fn lit_without_suffix_unchanged() {
		// A literal not followed by `[` is a token on its own.
		assert_eq!(
			toks("'x' | 'y'"),
			vec![LitStr("'x'"), BinOp("|"), LitStr("'y'")]
		);
	}

	//
	// parens
	//

	#[test]
	fn paren_open() {
		assert_eq!(toks("("), vec![OpenParen]);
	}

	#[test]
	fn paren_close() {
		assert_eq!(toks(")"), vec![CloseParen]);
	}

	#[test]
	fn paren_repeated() {
		assert_eq!(toks("(("), vec![OpenParen, OpenParen]);
	}

	#[test]
	fn paren_around_path() {
		assert_eq!(
			toks("(@.a)"),
			vec![OpenParen, Segment("@"), SegSep, Segment("a"), CloseParen]
		);
	}

	//
	// mismatched parens
	//

	#[test]
	fn mismatched_paren_unclosed_in_args() {
		// An unclosed paren inside path args is not an error; we read to the end.
		assert_eq!(toks("@.f(a"), vec![Segment("@"), SegSep, Segment("f(a")]);
	}

	#[test]
	fn mismatched_paren_extra_close_after_args() {
		assert_eq!(
			toks("@.f(a))"),
			vec![Segment("@"), SegSep, Segment("f(a)"), CloseParen]
		);
	}

	#[test]
	fn mismatched_paren_stray_open_outside() {
		assert_eq!(toks("(a"), vec![OpenParen, Segment("a")]);
	}

	#[test]
	fn abcd() {
		assert_eq!(
			toks("a ~= (b | c) == @.d"),
			vec![
				Segment("a"),
				BinOp("~="),
				OpenParen,
				Segment("b"),
				BinOp("|"),
				Segment("c"),
				CloseParen,
				BinOp("=="),
				Segment("@"),
				SegSep,
				Segment("d")
			]
		);
	}

	#[test]
	fn mismatched_paren_excess_opens() {
		assert_eq!(
			toks("((a)"),
			vec![OpenParen, OpenParen, Segment("a"), CloseParen]
		);
	}

	#[test]
	fn paren_in_literal() {
		// An open paren does not terminate an unquoted literal.
		assert_eq!(toks("a(b"), vec![Segment("a(b")]);
	}

	//
	// mismatched quotes
	//

	#[test]
	fn mismatched_quote_unterminated() {
		assert_eq!(toks("'abc"), vec![LitStr("'abc")]);
	}

	#[test]
	fn mismatched_quote_wrong_closer() {
		assert_eq!(toks("'abc\""), vec![LitStr("'abc\"")]);
	}

	#[test]
	fn trailing_ws_after_number() {
		assert_eq!(
			toks("@.a == 5 "),
			vec![Segment("@"), SegSep, Segment("a"), BinOp("=="), LitNum("5")],
		);
	}

	#[test]
	fn trailing_ws_after_binop() {
		assert_eq!(
			toks("@.a == "),
			vec![Segment("@"), SegSep, Segment("a"), BinOp("==")]
		);
	}

	#[test]
	fn trailing_ws_after_number_multiple_spaces() {
		assert_eq!(toks("1   "), vec![LitNum("1")]);
	}

	#[test]
	fn trailing_ws_after_negative_number() {
		assert_eq!(
			toks("@.a == -1  "),
			vec![
				Segment("@"),
				SegSep,
				Segment("a"),
				BinOp("=="),
				LitNum("-1")
			]
		);
	}

	#[test]
	fn only_whitespace() {
		assert_eq!(toks("   "), vec![]);
	}

	#[test]
	fn trailing_tabs_after_number() {
		assert_eq!(toks("5\t\t"), vec![LitNum("5")]);
	}

	#[test]
	fn unexpected_char_terminates() {
		assert_eq!(
			toks("url={{ x }}"),
			vec![
				Segment("url"),
				BinOp("="),
				Unexpected("{"),
				Unexpected("{"),
				Segment("x"),
				Unexpected("}"),
				Unexpected("}"),
			]
		);
	}

	#[test]
	fn segsep_without_following_segment() {
		assert_eq!(
			toks("@.a."),
			vec![Segment("@"), SegSep, Segment("a"), SegSep]
		);
	}

	#[test]
	fn segsep_without_leading_segment() {
		assert_eq!(toks(".a"), vec![SegSep, Segment("a")]);
		assert_eq!(toks("..a"), vec![SegSep, SegSep, Segment("a")]);
	}

	#[test]
	fn escaped_parens_in_args() {
		assert_eq!(
			toks(r"@.f(a\)b)"),
			vec![Segment("@"), SegSep, Segment(r"f(a\)b)")]
		);
		assert_eq!(
			toks(r"@.f(a\(b)"),
			vec![Segment("@"), SegSep, Segment(r"f(a\(b)")]
		);
	}

	#[test]
	fn path_mode_has_no_literals() {
		assert_eq!(path_toks("@.5a"), vec![Segment("@"), SegSep, Segment("5a")]);
		assert_eq!(
			toks("@.5a"),
			vec![Segment("@"), SegSep, LitNum("5"), Unexpected("a")]
		);
	}

	#[test]
	fn positions_are_reported() {
		assert_eq!(
			Tokenizer::new("@.ab[0] == 'x'").collect::<Vec<_>>(),
			vec![
				(0, Segment("@")),
				(1, SegSep),
				(2, Segment("ab")),
				(4, Slice("[0]")),
				(8, BinOp("==")),
				(11, LitStr("'x'")),
			]
		);
	}

	#[test]
	fn trailing_garbage_is_not_dropped() {
		assert_eq!(
			toks("@.a {oops"),
			vec![
				Segment("@"),
				SegSep,
				Segment("a"),
				Unexpected("{"),
				Segment("oops")
			]
		);
	}

	#[test]
	fn valid_expressions_are_unchanged() {
		assert_eq!(
			toks("@.a.b(1) == 'x'"),
			vec![
				Segment("@"),
				SegSep,
				Segment("a"),
				SegSep,
				Segment("b(1)"),
				BinOp("=="),
				LitStr("'x'")
			]
		);
	}
}
