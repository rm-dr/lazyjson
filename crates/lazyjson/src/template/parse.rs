use crate::{
	label::Label,
	template::{Template, TemplateNode, TemplateParseError},
};

/// All types of a {{ ... }} substitution
enum Tag<'a> {
	Substitution(&'a str),
	Let { name: Label, expr: &'a str },
	For { var: Label, iterable: &'a str },
	EndFor,
	If { cond: &'a str },
	Else,
	EndIf,
}

/// A {{ ... }} tag that has been opened but not yet closed.
enum OpenTag<'a> {
	For {
		/// Byte index of the opening `{{`
		position: usize,
		var: Label,
		iterable: &'a str,
		/// Nodes collected for this loop's body so far.
		body: Vec<TemplateNode<'a>>,
	},

	If {
		/// Byte index of the opening `{{` for error reporting.
		position: usize,
		cond: &'a str,
		body: Vec<TemplateNode<'a>>,
		else_body: Vec<TemplateNode<'a>>,

		/// Whether an `{{ else }}` has been seen.
		/// If true, nodes fill `else_body`
		in_else: bool,
	},
}

impl<'a> OpenTag<'a> {
	/// The byte index of this block's opening `{{`.
	fn position(&self) -> usize {
		match self {
			OpenTag::For { position, .. } | OpenTag::If { position, .. } => *position,
		}
	}

	/// The node list that new nodes should currently append to
	fn current_body(&mut self) -> &mut Vec<TemplateNode<'a>> {
		match self {
			OpenTag::For { body, .. } => body,
			OpenTag::If {
				body,
				else_body,
				in_else,
				..
			} => {
				if *in_else {
					else_body
				} else {
					body
				}
			}
		}
	}
}

//
// MARK: parse
//

pub(super) fn parse(source: &str) -> Result<Template<'_>, TemplateParseError> {
	let mut root: Vec<TemplateNode<'_>> = Vec::new();
	let mut stack: Vec<OpenTag<'_>> = Vec::new();

	let mut rest = source;
	let mut offset = 0usize;

	while !rest.is_empty() {
		let Some(open) = rest.find("{{") else {
			push_node(&mut root, &mut stack, TemplateNode::Literal(rest));
			break;
		};

		let tag_start = offset + open;
		let after_open = &rest[open + 2..];
		let close = after_open
			.find("}}")
			.ok_or(TemplateParseError::UnclosedTag {
				position: tag_start,
			})?;

		let inner = after_open[..close].trim();
		let tag = parse_tag(inner, tag_start)?;

		// Bytes consumed by the tag itself (`{{ ... }}`).
		let mut consumed = open + 2 + close + 2;

		let before = &rest[..open];
		let before = if matches!(
			tag,
			Tag::For { .. }
				| Tag::EndFor
				| Tag::If { .. }
				| Tag::Else | Tag::EndIf
				| Tag::Let { .. }
		) {
			let (trimmed, eat) = line_alone_trim(source, tag_start, before, &rest[consumed..]);
			consumed += eat;
			trimmed
		} else {
			before
		};

		if !before.is_empty() {
			push_node(&mut root, &mut stack, TemplateNode::Literal(before));
		}

		match tag {
			Tag::Substitution(expr) => {
				push_node(&mut root, &mut stack, TemplateNode::Substitution(expr))
			}

			Tag::Let { name, expr } => {
				push_node(&mut root, &mut stack, TemplateNode::Let { name, expr })
			}

			Tag::For { var, iterable } => stack.push(OpenTag::For {
				position: tag_start,
				var,
				iterable,
				body: Vec::new(),
			}),

			Tag::EndFor => {
				let node = match stack.pop() {
					Some(OpenTag::For {
						var,
						iterable,
						body,
						..
					}) => TemplateNode::ForLoop {
						var,
						iterable,
						body,
					},

					_ => {
						return Err(TemplateParseError::UnmatchedEndfor {
							position: tag_start,
						});
					}
				};
				push_node(&mut root, &mut stack, node);
			}

			Tag::If { cond } => stack.push(OpenTag::If {
				position: tag_start,
				cond,
				body: Vec::new(),
				else_body: Vec::new(),
				in_else: false,
			}),

			Tag::Else => match stack.last_mut() {
				Some(OpenTag::If { in_else, .. }) if !*in_else => *in_else = true,
				_ => {
					return Err(TemplateParseError::UnmatchedElse {
						position: tag_start,
					});
				}
			},

			Tag::EndIf => {
				let node = match stack.pop() {
					Some(OpenTag::If {
						cond,
						body,
						else_body,
						..
					}) => TemplateNode::If {
						cond,
						body,
						else_body,
					},

					_ => {
						return Err(TemplateParseError::UnmatchedEndif {
							position: tag_start,
						});
					}
				};
				push_node(&mut root, &mut stack, node);
			}
		}

		offset += consumed;
		rest = &rest[consumed..];
	}

	// Any block still on the stack was never closed.
	if let Some(unclosed) = stack.first() {
		let position = unclosed.position();
		return Err(match unclosed {
			OpenTag::For { .. } => TemplateParseError::UnclosedForLoop { position },
			OpenTag::If { .. } => TemplateParseError::UnclosedIf { position },
		});
	}

	Ok(Template { nodes: root })
}

/// Append a node to the innermost open block's active body,
/// or to `root` if none is open.
fn push_node<'a>(
	root: &mut Vec<TemplateNode<'a>>,
	stack: &mut [OpenTag<'a>],
	node: TemplateNode<'a>,
) {
	match stack.last_mut() {
		Some(frame) => frame.current_body().push(node),
		None => root.push(node),
	}
}

/// Classify the trimmed contents of a single `{{ ... }}` tag.
fn parse_tag(inner: &str, position: usize) -> Result<Tag<'_>, TemplateParseError> {
	if inner == "endfor" {
		return Ok(Tag::EndFor);
	}

	if inner == "endif" {
		return Ok(Tag::EndIf);
	}

	if inner == "else" {
		return Ok(Tag::Else);
	}

	if let Some(rest) = strip_keyword(inner, "let") {
		return parse_let(rest, inner, position);
	}

	if let Some(rest) = strip_keyword(inner, "for") {
		return parse_for(rest, inner, position);
	}

	if let Some(cond) = strip_keyword(inner, "if") {
		if cond.is_empty() {
			return Err(TemplateParseError::MalformedIf {
				position,
				source_str: inner.into(),
			});
		}
		return Ok(Tag::If { cond });
	}

	if inner.is_empty() {
		return Err(TemplateParseError::EmptySubstitution { position });
	}

	Ok(Tag::Substitution(inner))
}

/// Parse the part of a `for` header after the `for` keyword: `<var> in <expr>`.
fn parse_for<'a>(
	rest: &'a str,
	full: &str,
	position: usize,
) -> Result<Tag<'a>, TemplateParseError> {
	let malformed = || TemplateParseError::MalformedFor {
		position,
		source_str: full.into(),
	};

	// `<var>` is the next word, then the `in` keyword, then the expression.
	let (var_str, after_var) = split_word(rest);
	if var_str.is_empty() {
		return Err(malformed());
	}

	let after_in = strip_keyword(after_var, "in").ok_or_else(malformed)?;
	let iterable = after_in.trim();
	if iterable.is_empty() {
		return Err(malformed());
	}

	let var = Label::new(var_str).ok_or_else(|| TemplateParseError::InvalidLoopVar {
		position,
		str: var_str.into(),
	})?;

	Ok(Tag::For { var, iterable })
}

/// Parse the part of a `let` header after the `let` keyword: `<name> = <expr>`.
fn parse_let<'a>(
	rest: &'a str,
	full: &str,
	position: usize,
) -> Result<Tag<'a>, TemplateParseError> {
	let malformed = || TemplateParseError::MalformedLet {
		position,
		source_str: full.into(),
	};

	let eq = rest.find('=').ok_or_else(malformed)?;
	let name_str = rest[..eq].trim();
	let expr = rest[eq + 1..].trim();
	if name_str.is_empty() || expr.is_empty() {
		return Err(malformed());
	}

	let name = Label::new(name_str).ok_or_else(|| TemplateParseError::InvalidLetVar {
		position,
		str: name_str.into(),
	})?;

	Ok(Tag::Let { name, expr })
}

/// If `s` begins with the whole word `kw` (followed by whitespace or end),
/// return the remainder trimmed of leading whitespace. Otherwise `None`.
fn strip_keyword<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
	let rest = s.strip_prefix(kw)?;
	match rest.chars().next() {
		None => Some(rest),
		Some(c) if c.is_whitespace() => Some(rest.trim_start()),
		Some(_) => None,
	}
}

/// Split off the leading whitespace-delimited word, returning `(word, rest)`
/// where `rest` has its leading whitespace trimmed.
fn split_word(s: &str) -> (&str, &str) {
	let s = s.trim_start();
	let end = s.find(char::is_whitespace).unwrap_or(s.len());
	let (word, rest) = s.split_at(end);
	(word, rest.trim_start())
}

/// Apply block-level whitespace control for a `for` / `endfor` tag that is the
/// only text on its line.
fn line_alone_trim<'a>(
	source: &str,
	tag_start: usize,
	before: &'a str,
	after: &str,
) -> (&'a str, usize) {
	// Left: the tag's indentation is the trailing spaces/tabs of `before`; the
	// tag leads its line iff a newline (or the start of source) precedes it.
	let trimmed = before.trim_end_matches([' ', '\t']);
	let indent = before.len() - trimmed.len();
	let line_start = tag_start - indent;
	let left_alone = line_start == 0 || source.as_bytes()[line_start - 1] == b'\n';

	// Right: optional spaces/tabs, then a newline (`\n` / `\r\n`) or end of
	// source.
	let after_ws = after.trim_start_matches([' ', '\t']);
	let trailing_ws = after.len() - after_ws.len();
	let (right_alone, newline) = if after_ws.is_empty() {
		(true, 0)
	} else if after_ws.starts_with("\r\n") {
		(true, 2)
	} else if after_ws.starts_with('\n') {
		(true, 1)
	} else {
		(false, 0)
	};

	if left_alone && right_alone {
		(trimmed, trailing_ws + newline)
	} else {
		(before, 0)
	}
}

//
// MARK: tests
//

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
	use super::{Template, TemplateNode, TemplateParseError};
	use crate::label::Label;

	fn parse(s: &str) -> Vec<TemplateNode<'_>> {
		Template::parse(s).unwrap().nodes
	}

	fn lit(s: &str) -> TemplateNode<'_> {
		TemplateNode::Literal(s)
	}

	fn sub(s: &str) -> TemplateNode<'_> {
		TemplateNode::Substitution(s)
	}

	fn let_bind<'a>(name: &str, expr: &'a str) -> TemplateNode<'a> {
		TemplateNode::Let {
			name: Label::new(name).unwrap(),
			expr,
		}
	}

	fn for_loop<'a>(var: &str, iterable: &'a str, body: Vec<TemplateNode<'a>>) -> TemplateNode<'a> {
		TemplateNode::ForLoop {
			var: Label::new(var).unwrap(),
			iterable,
			body,
		}
	}

	fn if_block<'a>(
		cond: &'a str,
		body: Vec<TemplateNode<'a>>,
		else_body: Vec<TemplateNode<'a>>,
	) -> TemplateNode<'a> {
		TemplateNode::If {
			cond,
			body,
			else_body,
		}
	}

	#[test]
	fn plain_literal() {
		assert_eq!(parse("just text"), vec![lit("just text")]);
	}

	#[test]
	fn empty_template() {
		assert_eq!(parse(""), vec![]);
	}

	#[test]
	fn single_substitution() {
		assert_eq!(
			parse("{{ @.substitute.value.here }}"),
			vec![sub("@.substitute.value.here")],
		);
	}

	#[test]
	fn substitution_between_literals() {
		assert_eq!(
			parse("ima template {{ @.substitute.value.here }} continue template"),
			vec![
				lit("ima template "),
				sub("@.substitute.value.here"),
				lit(" continue template"),
			],
		);
	}

	#[test]
	fn adjacent_substitutions() {
		assert_eq!(parse("{{ @.a }}{{ @.b }}"), vec![sub("@.a"), sub("@.b")]);
	}

	#[test]
	fn substitution_whitespace_is_trimmed() {
		assert_eq!(parse("{{@.a}}"), vec![sub("@.a")]);
		assert_eq!(parse("{{    @.a   }}"), vec![sub("@.a")]);
	}

	#[test]
	fn substitution_keeps_inner_expression_verbatim() {
		// The expression is not parsed; operators/args are preserved as-is.
		assert_eq!(
			parse("{{ @.a.b | @.a.c == 5 }}"),
			vec![sub("@.a.b | @.a.c == 5")],
		);
	}

	#[test]
	fn simple_for_loop() {
		assert_eq!(
			parse("{{ for x in @.user.images }}{{ x.uuid }}{{ endfor }}"),
			vec![for_loop("x", "@.user.images", vec![sub("x.uuid")])],
		);
	}

	#[test]
	fn for_loop_with_literal_body() {
		assert_eq!(
			parse("{{ for x in @.user.images }}image: {{ x.uuid }}\n{{ endfor }}"),
			vec![for_loop(
				"x",
				"@.user.images",
				vec![lit("image: "), sub("x.uuid"), lit("\n")],
			)],
		);
	}

	#[test]
	fn for_loop_between_literals() {
		assert_eq!(
			parse("before {{ for x in @.xs }}{{ x }}{{ endfor }} after"),
			vec![
				lit("before "),
				for_loop("x", "@.xs", vec![sub("x")]),
				lit(" after"),
			],
		);
	}

	#[test]
	fn nested_for_loops() {
		assert_eq!(
			parse("{{ for x in @.a }}{{ for y in x.b }}{{ y.c }}{{ endfor }}{{ endfor }}"),
			vec![for_loop(
				"x",
				"@.a",
				vec![for_loop("y", "x.b", vec![sub("y.c")])],
			)],
		);
	}

	#[test]
	fn simple_let() {
		assert_eq!(
			parse("{{ let u = @.http.user }}{{ u.name }}"),
			vec![let_bind("u", "@.http.user"), sub("u.name")],
		);
	}

	#[test]
	fn let_without_spaces_around_eq() {
		assert_eq!(parse("{{ let x=@.a }}"), vec![let_bind("x", "@.a")]);
	}

	#[test]
	fn let_expression_kept_verbatim() {
		// Only the first `=` separates the name; the rest is the expression,
		// preserved as-is (operators and all).
		assert_eq!(
			parse("{{ let ok = @.a >= 5 }}"),
			vec![let_bind("ok", "@.a >= 5")],
		);
	}

	#[test]
	fn repeated_let_overwrites_produce_two_nodes() {
		// Parsing keeps both bindings; the evaluator applies them in order so the
		// later one wins.
		assert_eq!(
			parse("{{ let x = @.a }}{{ let x = @.b }}{{ x }}"),
			vec![let_bind("x", "@.a"), let_bind("x", "@.b"), sub("x")],
		);
	}

	#[test]
	fn let_alone_on_line_eats_its_line() {
		// A `let` emits nothing, so like block tags it owns its line rather than
		// leaving a blank one.
		assert_eq!(
			parse("a\n{{ let x = @.a }}\n{{ x }}"),
			vec![lit("a\n"), let_bind("x", "@.a"), sub("x")],
		);
	}

	#[test]
	fn let_inside_for_body() {
		assert_eq!(
			parse("{{ for i in @.xs }}{{ let n = i.name }}{{ n }}{{ endfor }}"),
			vec![for_loop(
				"i",
				"@.xs",
				vec![let_bind("n", "i.name"), sub("n")],
			)],
		);
	}

	#[test]
	fn let_missing_eq() {
		assert!(matches!(
			Template::parse("{{ let x @.a }}"),
			Err(TemplateParseError::MalformedLet { position: 0, .. }),
		));
	}

	#[test]
	fn let_missing_name() {
		assert!(matches!(
			Template::parse("{{ let = @.a }}"),
			Err(TemplateParseError::MalformedLet { position: 0, .. }),
		));
	}

	#[test]
	fn let_missing_expression() {
		assert!(matches!(
			Template::parse("{{ let x = }}"),
			Err(TemplateParseError::MalformedLet { position: 0, .. }),
		));
	}

	#[test]
	fn let_invalid_name() {
		assert!(matches!(
			Template::parse("{{ let x.y = @.a }}"),
			Err(TemplateParseError::InvalidLetVar { position: 0, .. }),
		));
	}

	#[test]
	fn simple_if() {
		assert_eq!(
			parse("{{ if @.a }}yes{{ endif }}"),
			vec![if_block("@.a", vec![lit("yes")], vec![])],
		);
	}

	#[test]
	fn if_else() {
		assert_eq!(
			parse("{{ if @.a }}yes{{ else }}no{{ endif }}"),
			vec![if_block("@.a", vec![lit("yes")], vec![lit("no")])],
		);
	}

	#[test]
	fn if_condition_kept_verbatim() {
		// The condition is not parsed; operators/args are preserved as-is.
		assert_eq!(
			parse("{{ if @.a == 5 }}x{{ endif }}"),
			vec![if_block("@.a == 5", vec![lit("x")], vec![])],
		);
	}

	#[test]
	fn if_between_literals() {
		assert_eq!(
			parse("before {{ if @.a }}x{{ endif }} after"),
			vec![
				lit("before "),
				if_block("@.a", vec![lit("x")], vec![]),
				lit(" after"),
			],
		);
	}

	#[test]
	fn if_nested_in_for() {
		assert_eq!(
			parse("{{ for x in @.xs }}{{ if x.ok }}{{ x.v }}{{ endif }}{{ endfor }}"),
			vec![for_loop(
				"x",
				"@.xs",
				vec![if_block("x.ok", vec![sub("x.v")], vec![])],
			)],
		);
	}

	#[test]
	fn for_nested_in_if_branches() {
		// A loop in the then-branch and another in the else-branch land in the
		// right bodies.
		assert_eq!(
			parse(
				"{{ if @.a }}{{ for x in @.xs }}{{ x }}{{ endfor }}{{ else }}{{ for y in @.ys }}{{ y }}{{ endfor }}{{ endif }}"
			),
			vec![if_block(
				"@.a",
				vec![for_loop("x", "@.xs", vec![sub("x")])],
				vec![for_loop("y", "@.ys", vec![sub("y")])],
			)],
		);
	}

	#[test]
	fn nested_if_blocks() {
		assert_eq!(
			parse("{{ if @.a }}{{ if @.b }}x{{ endif }}{{ endif }}"),
			vec![if_block(
				"@.a",
				vec![if_block("@.b", vec![lit("x")], vec![])],
				vec![],
			)],
		);
	}

	#[test]
	fn if_else_endif_alone_on_line_eat_their_line() {
		// Like `for`/`endfor`, each block tag owns its line, so those lines
		// disappear: no blank lines, and the bodies keep their own newlines.
		assert_eq!(
			parse("a\n{{ if @.c }}\nb\n{{ else }}\nd\n{{ endif }}\ne"),
			vec![
				lit("a\n"),
				if_block("@.c", vec![lit("b\n")], vec![lit("d\n")]),
				lit("e"),
			],
		);
	}

	#[test]
	fn for_endfor_alone_on_line_eat_their_line() {
		// Each block tag owns its line, so that whole line disappears: no blank
		// lines, and the body keeps its own newlines.
		assert_eq!(
			parse("a\n{{ for x in @.xs }}\nb\n{{ endfor }}\nc"),
			vec![
				lit("a\n"),
				for_loop("x", "@.xs", vec![lit("b\n")]),
				lit("c"),
			],
		);
	}

	#[test]
	fn block_tag_line_indentation_is_stripped() {
		// Leading indentation on the tag's line and the trailing newline are both
		// eaten; body indentation is untouched.
		assert_eq!(
			parse("  {{ for x in @.xs }}  \n  body\n  {{ endfor }}\n"),
			vec![for_loop("x", "@.xs", vec![lit("  body\n")])],
		);
	}

	#[test]
	fn block_tag_not_alone_on_line_is_untouched() {
		// A `for` with body text on the same line keeps all surrounding text.
		assert_eq!(
			parse("x {{ for i in @.xs }}y{{ endfor }}\n"),
			vec![lit("x "), for_loop("i", "@.xs", vec![lit("y")]), lit("\n"),],
		);
	}

	#[test]
	fn substitution_alone_on_line_keeps_its_newline() {
		// Only `for` / `endfor` get whitespace control — substitutions do not.
		assert_eq!(parse("{{ @.a }}\n"), vec![sub("@.a"), lit("\n")],);
	}

	#[test]
	fn for_iterable_kept_verbatim() {
		assert_eq!(
			parse("{{ for x in @.user.images[0..10] }}{{ x }}{{ endfor }}"),
			vec![for_loop("x", "@.user.images[0..10]", vec![sub("x")])],
		);
	}

	#[test]
	fn unclosed_tag() {
		assert_eq!(
			Template::parse("hello {{ @.a ").unwrap_err(),
			TemplateParseError::UnclosedTag { position: 6 },
		);
	}

	#[test]
	fn empty_substitution() {
		assert_eq!(
			Template::parse("{{  }}").unwrap_err(),
			TemplateParseError::EmptySubstitution { position: 0 },
		);
	}

	#[test]
	fn unmatched_endfor() {
		assert_eq!(
			Template::parse("{{ endfor }}").unwrap_err(),
			TemplateParseError::UnmatchedEndfor { position: 0 },
		);
	}

	#[test]
	fn unclosed_for_loop() {
		assert_eq!(
			Template::parse("{{ for x in @.a }}body").unwrap_err(),
			TemplateParseError::UnclosedForLoop { position: 0 },
		);
	}

	#[test]
	fn for_missing_in() {
		assert!(matches!(
			Template::parse("{{ for x @.a }}{{ endfor }}"),
			Err(TemplateParseError::MalformedFor { position: 0, .. }),
		));
	}

	#[test]
	fn for_missing_iterable() {
		assert!(matches!(
			Template::parse("{{ for x in }}{{ endfor }}"),
			Err(TemplateParseError::MalformedFor { position: 0, .. }),
		));
	}

	#[test]
	fn for_invalid_loop_var() {
		assert!(matches!(
			Template::parse("{{ for x.y in @.a }}{{ endfor }}"),
			Err(TemplateParseError::InvalidLoopVar { position: 0, .. }),
		));
	}

	#[test]
	fn if_missing_condition() {
		assert!(matches!(
			Template::parse("{{ if }}{{ endif }}"),
			Err(TemplateParseError::MalformedIf { position: 0, .. }),
		));
	}

	#[test]
	fn unmatched_else() {
		assert_eq!(
			Template::parse("{{ else }}").unwrap_err(),
			TemplateParseError::UnmatchedElse { position: 0 },
		);
	}

	#[test]
	fn double_else_is_unmatched() {
		// A second `else` in the same block has no `if` still awaiting its else.
		assert!(matches!(
			Template::parse("{{ if @.a }}x{{ else }}y{{ else }}z{{ endif }}"),
			Err(TemplateParseError::UnmatchedElse { .. }),
		));
	}

	#[test]
	fn unmatched_endif() {
		assert_eq!(
			Template::parse("{{ endif }}").unwrap_err(),
			TemplateParseError::UnmatchedEndif { position: 0 },
		);
	}

	#[test]
	fn unclosed_if() {
		assert_eq!(
			Template::parse("{{ if @.a }}body").unwrap_err(),
			TemplateParseError::UnclosedIf { position: 0 },
		);
	}

	#[test]
	fn endif_closing_a_for_is_unmatched() {
		assert!(matches!(
			Template::parse("{{ for x in @.a }}{{ endif }}"),
			Err(TemplateParseError::UnmatchedEndif { .. }),
		));
	}

	#[test]
	fn endfor_closing_an_if_is_unmatched() {
		assert!(matches!(
			Template::parse("{{ if @.a }}{{ endfor }}"),
			Err(TemplateParseError::UnmatchedEndfor { .. }),
		));
	}
}
