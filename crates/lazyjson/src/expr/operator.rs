use regex::Regex;
use std::str::FromStr;

use crate::{
	expr::glob::GlobPattern,
	value::{ELazyValue, LazyValue, extract::EvalError},
};

/// A binary operator that computes a value
/// from two others.
///
/// For logical ops, `null` and `false` are the only falsy values.
/// All others are truthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
	/// Parsed from `==`. Simple equality.
	/// true if two values are equal
	Eq,

	/// Parsed from `!=`. Simple equality.
	/// true if two values are not equal
	Neq,

	/// Parsed from `<`. Numeric less-than.
	/// true if left is numerically less than right.
	Lt,

	/// Parsed from `<=`. Numeric less-than-or-equal.
	/// true if left is numerically less than or equal to right.
	Lte,

	/// Parsed from `>`. Numeric greater-than.
	/// true if left is numerically greater than right.
	Gt,

	/// Parsed from `>=`. Numeric greater-than-or-equal.
	/// true if left is numerically greater than or equal to right.
	Gte,

	/// Parsed from `~=`. Regex match.
	/// true if left (value) matches right (pattern)
	Match,

	/// Parsed from `~=/`. Path glob match.
	/// true if left (value) matches right (pattern)
	GlobMatch,

	/// Parsed from `|`. Next-if-null.
	/// left if left is non-null, otherwise right.
	///
	/// Note that `false` is non-null. Thus:
	/// - `false | true` is `false`
	/// - `null | false` is `false`
	/// - `null | true` is `true`
	/// - `null | null` is `null`
	Or,

	/// Parsed from `=>`. if-then.
	/// right if left is truthy, otherwise left.
	Then,

	/// Parsed from `||`. Logical or.
	/// false if left and right are both falsy, true otherwise.
	///
	/// `false || true` is true
	LogOr,

	/// Parsed from `&&`. Logical and.
	/// true if left and right are both truthy, false otherwise.
	LogAnd,
}

impl FromStr for BinOp {
	type Err = ();

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(match s {
			"==" => Self::Eq,
			"!=" => Self::Neq,
			"<" => Self::Lt,
			"<=" => Self::Lte,
			">" => Self::Gt,
			">=" => Self::Gte,
			"~=" => Self::Match,
			"~=/" => Self::GlobMatch,
			"|" => Self::Or,
			"=>" => Self::Then,
			"||" => Self::LogOr,
			"&&" => Self::LogAnd,
			_ => return Err(()),
		})
	}
}

impl BinOp {
	/// All strings that parse to a [BinOp]
	pub const ALL_OPS: &'static [&'static str] = &[
		"==", "!=", "<", "<=", ">", ">=", "~=", "~=/", "|", "=>", "||", "&&",
	];

	/// Returns true if `c` can appear in a binop.
	pub fn is_char(c: char) -> bool {
		Self::ALL_OPS.iter().any(|x| x.contains(c))
	}

	/// Returns (left, right) bind power of this operator.
	/// This determines order of operations.
	/// Operators with a higher bind power are evaluated first.
	///
	/// Operators with equal bind power are evaluated
	/// in the order they are seen, left to right.
	#[inline(always)]
	pub const fn bind_power(&self) -> (u32, u32) {
		match self {
			Self::Then => (0, 0),
			Self::LogOr => (1, 1),
			Self::LogAnd => (1, 1),

			Self::Eq => (2, 2),
			Self::Neq => Self::Eq.bind_power(),
			Self::Lt => Self::Eq.bind_power(),
			Self::Lte => Self::Eq.bind_power(),
			Self::Gt => Self::Eq.bind_power(),
			Self::Gte => Self::Eq.bind_power(),
			Self::Match => Self::Eq.bind_power(),
			Self::GlobMatch => Self::Eq.bind_power(),

			Self::Or => (3, 3),
		}
	}

	pub fn eval(
		&self,
		strict: bool,
		left: &ELazyValue,
		right: &ELazyValue,
	) -> Result<ELazyValue, EvalError> {
		Ok(match self {
			Self::Eq => LazyValue::Bool(left.inner == right.inner).into(),
			Self::Neq => LazyValue::Bool(left.inner != right.inner).into(),

			Self::Lt | Self::Lte | Self::Gt | Self::Gte => {
				// Numeric comparison. Both sides are coerced to `f64`; if either
				// is non-numeric the result is `Null` (falsy), mirroring how
				// `~=` / `~=/` yield `Null` on a type mismatch rather than
				// erroring.
				match (left.inner.num(), right.inner.num()) {
					(Some(l), Some(r)) => {
						let out = match self {
							Self::Lt => l < r,
							Self::Lte => l <= r,
							Self::Gt => l > r,
							Self::Gte => l >= r,
							_ => unreachable!(),
						};
						LazyValue::Bool(out).into()
					}
					_ => LazyValue::Null.into(),
				}
			}

			Self::LogAnd => {
				let left = left.inner.truthy();
				let right = right.inner.truthy();
				LazyValue::Bool(left && right).into()
			}

			Self::LogOr => {
				let left = left.inner.truthy();
				let right = right.inner.truthy();
				LazyValue::Bool(left || right).into()
			}

			Self::Or => match (&left.inner, &right.inner) {
				(LazyValue::Null, _) => right.clone(),
				_ => left.clone(),
			},

			Self::Then => if left.inner.truthy() { right } else { left }.clone(),
			Self::Match => match (&left.inner, &right.inner) {
				(LazyValue::String(value), LazyValue::String(pattern)) => match Regex::new(pattern)
				{
					Ok(r) => LazyValue::Bool(r.is_match(value)).into(),
					Err(e) if strict => {
						return Err(EvalError::InvalidArgs(format!(
							"invalid regex {pattern:?}: {e}"
						)));
					}
					Err(_) => LazyValue::Bool(false).into(),
				},

				(_, _) => LazyValue::Null.into(),
			},

			Self::GlobMatch => match (&left.inner, &right.inner) {
				(LazyValue::String(value), LazyValue::String(pattern)) => {
					let rule = GlobPattern {
						patterns: pattern.split("/").map(|x| x.to_owned()).collect(),
					};

					match rule.regex() {
						Ok(r) => LazyValue::Bool(r.is_match(value)).into(),
						Err(e) if strict => {
							return Err(EvalError::InvalidArgs(format!(
								"invalid glob {pattern:?}: {e}"
							)));
						}
						Err(_) => LazyValue::Bool(false).into(),
					}
				}

				(_, _) => LazyValue::Null.into(),
			},
		})
	}
}

//
// MARK: tests
//

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
	use std::sync::Arc;

	use super::*;

	fn s(x: &str) -> ELazyValue {
		LazyValue::String(Arc::new(x.into())).into()
	}

	fn i(x: i64) -> ELazyValue {
		LazyValue::I64(x).into()
	}

	//
	// MARK: numeric comparison
	//

	#[test]
	fn compare_integers() {
		assert_eq!(
			BinOp::Lt.eval(false, &i(1), &i(2)).unwrap().inner,
			LazyValue::Bool(true)
		);
		assert_eq!(
			BinOp::Gt.eval(false, &i(1), &i(2)).unwrap().inner,
			LazyValue::Bool(false)
		);
		assert_eq!(
			BinOp::Gte.eval(false, &i(2), &i(2)).unwrap().inner,
			LazyValue::Bool(true)
		);
		assert_eq!(
			BinOp::Lte.eval(false, &i(2), &i(2)).unwrap().inner,
			LazyValue::Bool(true)
		);
	}

	#[test]
	fn compare_float_strings() {
		// Floats (weights) reach us as strings — comparison parses them.
		assert_eq!(
			BinOp::Gte.eval(false, &s("0.7"), &s("0.7")).unwrap().inner,
			LazyValue::Bool(true),
		);
		assert_eq!(
			BinOp::Gte.eval(false, &s("0.65"), &s("0.7")).unwrap().inner,
			LazyValue::Bool(false),
		);
		// Lexically "0.7" > "0.65", but numerically it is not — coercion wins.
		assert_eq!(
			BinOp::Gt.eval(false, &s("0.7"), &s("0.65")).unwrap().inner,
			LazyValue::Bool(true),
		);
	}

	#[test]
	fn compare_int_against_float_string() {
		// A literal integer vs a string-encoded number still compares numerically.
		assert_eq!(
			BinOp::Lt.eval(false, &s("1.5"), &i(2)).unwrap().inner,
			LazyValue::Bool(true),
		);
	}

	#[test]
	fn compare_non_numeric_is_null() {
		// A non-numeric string is not a number; the result is Null (falsy), not
		// an error — even in strict mode.
		assert_eq!(
			BinOp::Gt.eval(false, &s("hello"), &i(1)).unwrap().inner,
			LazyValue::Null,
		);
		assert_eq!(
			BinOp::Gte.eval(true, &i(1), &s("world")).unwrap().inner,
			LazyValue::Null,
		);
	}

	//
	// MARK: regex match (`~=`)
	//

	#[test]
	fn match_ok_pattern() {
		let out = BinOp::Match.eval(false, &s("hello"), &s("h.*o")).unwrap();
		assert_eq!(out.inner, LazyValue::Bool(true));
	}

	#[test]
	fn match_bad_pattern_lenient_is_false() {
		// An unclosed group is an invalid regex; lenient mode swallows it.
		let out = BinOp::Match.eval(false, &s("hello"), &s("(")).unwrap();
		assert_eq!(out.inner, LazyValue::Bool(false));
	}

	#[test]
	fn match_bad_pattern_strict_errors() {
		let err = BinOp::Match.eval(true, &s("hello"), &s("(")).unwrap_err();
		assert!(matches!(err, EvalError::InvalidArgs(_)), "got {err:?}");
	}

	#[test]
	fn match_non_string_is_null_even_in_strict() {
		// A type mismatch is not a bad pattern; it stays `Null` in both modes.
		let out = BinOp::Match
			.eval(true, &LazyValue::I64(1).into(), &s("x"))
			.unwrap();
		assert_eq!(out.inner, LazyValue::Null);
	}

	//
	// MARK: glob match (`~=/`)
	//

	#[test]
	fn glob_ok_pattern() {
		let out = BinOp::GlobMatch
			.eval(false, &s("a/b/c.txt"), &s("**/*.txt"))
			.unwrap();
		assert_eq!(out.inner, LazyValue::Bool(true));
	}

	#[test]
	fn glob_bad_pattern_lenient_is_false() {
		// Three adjacent stars is an invalid wildcard (`bail!`).
		let out = BinOp::GlobMatch.eval(false, &s("a"), &s("***a")).unwrap();
		assert_eq!(out.inner, LazyValue::Bool(false));
	}

	#[test]
	fn glob_bad_pattern_strict_errors() {
		let err = BinOp::GlobMatch
			.eval(true, &s("a"), &s("***a"))
			.unwrap_err();
		assert!(matches!(err, EvalError::InvalidArgs(_)), "got {err:?}");
	}
}
