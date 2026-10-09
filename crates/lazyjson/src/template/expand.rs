use serde_json::Value;
use std::{collections::HashMap, pin::Pin};

use crate::{
	Label,
	expr::{eval::ExpressionEvaluator, tree::LazyExpressionTree},
	template::{ExpandError, TemplateNode},
	value::{ELazyValue, extract::ExtractState},
};

/// The result of expanding a template.
#[derive(Debug, Default, Clone)]
pub struct Expansion {
	/// The expanded template
	pub text: String,

	/// The final state of {{ let }} aliases after this expansion
	pub scope: HashMap<Label, ELazyValue>,
}

/// Evaluate one {{ ... }} expression and render it as text.
async fn substitute(
	src: &str,
	state: &ExtractState,
	scoped_state: &ExtractState,
	root: &ELazyValue,
) -> Result<String, ExpandError> {
	let tree = LazyExpressionTree::parse(src).map_err(|source| ExpandError::ExprParse {
		expr: src.to_owned(),
		source,
	})?;

	let eval = ExpressionEvaluator {
		root,
		state: scoped_state,
	};

	match eval.eval(&tree).await {
		Ok(value) => match value.to_json(state).await {
			Err(source) => Err(ExpandError::Serialize(source)),
			Ok(None) | Ok(Some(Value::Null)) => Ok(String::new()),
			Ok(Some(Value::String(s))) => Ok(s),
			Ok(Some(other)) => Ok(other.to_string()),
		},

		Err(source) => Err(ExpandError::Eval {
			expr: src.to_owned(),
			source,
		}),
	}
}

/// Expand a single bare expression — `src` is what would sit inside a `{{ … }}`,
/// with no surrounding literal text — against `scope`.
///
/// This is the entry point for the places that store one expression rather than
/// a whole template (a prompt version's media attachments, say). It renders
/// exactly as the matching substitution in a template would.
pub async fn expand_expr(
	src: &str,
	scope: HashMap<Label, ELazyValue>,
	state: &ExtractState,
	root: &ELazyValue,
) -> Result<Expansion, ExpandError> {
	let scoped_state = ExtractState {
		aliases: scope_to_aliases(&scope),
		..state.clone()
	};

	Ok(Expansion {
		text: substitute(src, state, &scoped_state, root).await?,
		scope,
	})
}

/// Expand `nodes` against `scope`, returning the rendered text plus the scope
/// it ends with — the bindings its `{{ let }}` nodes made, on top of the scope
/// it was given. See [`Expansion`].
///
/// Handing the scope back is what lets an `{{ if }}` branch bind for the nodes
/// that follow it: the branch expands against a copy of the scope, and the copy
/// comes back. Only the branch that actually rendered can bind anything, which
/// is the point — a name is bound by the code that ran.
///
/// `root` is the value that `@` resolves against; building it (and whatever
/// sources it exposes) is the caller's job.
///
/// `+ Send`: callers await this inside an axum handler, whose own future must
/// be `Send`. A CLI's `block_on` doesn't need it.
pub fn expand<'a>(
	nodes: &'a [TemplateNode<'a>],
	scope: HashMap<Label, ELazyValue>,
	state: &'a ExtractState,
	root: &'a ELazyValue,
) -> Pin<Box<dyn Future<Output = Result<Expansion, ExpandError>> + Send + 'a>> {
	Box::pin(async move {
		let mut scope = scope;

		let mut aliases = scope_to_aliases(&scope);
		let mut scoped_state = ExtractState {
			aliases: aliases.clone(),
			..state.clone()
		};

		let mut out = String::new();
		for node in nodes {
			match node {
				TemplateNode::Literal(text) => out.push_str(text),

				TemplateNode::Substitution(src) => {
					out.push_str(&substitute(src, state, &scoped_state, root).await?)
				}

				TemplateNode::Let { name, expr } => {
					// Evaluate against the current scope in a block so `eval`'s borrow
					// of `scoped_state` ends before we replace it below.
					let value = {
						let eval = ExpressionEvaluator {
							root,
							state: &scoped_state,
						};
						let tree = LazyExpressionTree::parse(expr).map_err(|source| {
							ExpandError::ExprParse {
								expr: (*expr).to_owned(),
								source,
							}
						})?;
						match eval.eval(&tree).await {
							Ok(value) => value,
							Err(source) => {
								return Err(ExpandError::Eval {
									expr: (*expr).to_owned(),
									source,
								});
							}
						}
					};

					// The binding is visible to every node that follows; a repeat
					// binding to the same name overwrites the earlier one.
					scope.insert(name.clone(), value);
					aliases = scope_to_aliases(&scope);
					scoped_state = ExtractState {
						aliases: aliases.clone(),
						..state.clone()
					};
				}

				TemplateNode::If {
					cond,
					body,
					else_body,
				} => {
					// Evaluated in a block so `eval`'s borrow of `scoped_state` ends
					// before the branch's own bindings replace it below.
					let branch = {
						let eval = ExpressionEvaluator {
							root,
							state: &scoped_state,
						};
						let tree = LazyExpressionTree::parse(cond).map_err(|source| {
							ExpandError::ExprParse {
								expr: (*cond).to_owned(),
								source,
							}
						})?;
						match eval.eval(&tree).await {
							// Truthy => then-branch; falsy (`null` / `false`) =>
							// else-branch (empty when the template had no `{{ else }}`).
							Ok(value) => match value.inner.truthy() {
								true => body,
								false => else_body,
							},

							Err(source) => {
								return Err(ExpandError::Eval {
									expr: (*cond).to_owned(),
									source,
								});
							}
						}
					};

					// The branch binds for the nodes that follow the `{{ endif }}`:
					// it expands against a copy of the scope, and the copy comes
					// back. So a `{{ let }}` may sit inside the branch that needs it
					// rather than being hoisted out — which, for a binding whose
					// value is what the branch is choosing between, is the only place
					// it can sit at all.
					let ex = expand(branch, scope.clone(), state, root).await?;
					out.push_str(&ex.text);

					scope = ex.scope;
					aliases = scope_to_aliases(&scope);
					scoped_state = ExtractState {
						aliases: aliases.clone(),
						..state.clone()
					};
				}

				TemplateNode::ForLoop {
					var,
					iterable,
					body,
				} => {
					let eval = ExpressionEvaluator {
						root,
						state: &scoped_state,
					};
					let tree = LazyExpressionTree::parse(iterable).map_err(|source| {
						ExpandError::ExprParse {
							expr: (*iterable).to_owned(),
							source,
						}
					})?;
					let value = match eval.eval(&tree).await {
						Ok(value) => value,

						Err(source) => {
							return Err(ExpandError::Eval {
								expr: (*iterable).to_owned(),
								source,
							});
						}
					};

					let list = value.list().clone();
					let len = match list.len(state).await {
						Ok(len) => len,

						Err(source) => {
							return Err(ExpandError::Iterate {
								what: "reading loop iterable length",
								source,
							});
						}
					};
					for i in 0..len {
						let elem = match list.get(state, i).await {
							Ok(Some(elem)) => elem,
							Ok(None) => continue,
							Err(source) => {
								return Err(ExpandError::Iterate {
									what: "reading loop element",
									source,
								});
							}
						};
						// Unlike an `{{ if }}` branch, a loop body's bindings stay in
						// the iteration that made them: the loop variable is rebound
						// every pass, and a `{{ let }}` derived from it would otherwise
						// leave the last iteration's value standing after the loop.
						let mut child = scope.clone();
						child.insert(var.clone(), elem);
						let expanded = expand(body, child, state, root).await?;
						out.push_str(&expanded.text);
					}
				}
			}
		}

		Ok(Expansion { text: out, scope })
	})
}

/// Mirror a template `scope` as the alias map expressions resolve against: each
/// bound value becomes a literal expression tree keyed by its name.
fn scope_to_aliases(scope: &HashMap<Label, ELazyValue>) -> HashMap<Label, LazyExpressionTree> {
	scope
		.iter()
		.map(|(name, value)| (name.clone(), LazyExpressionTree::Literal(value.clone())))
		.collect()
}

//
// MARK: tests
//

#[cfg(test)]
#[expect(clippy::unwrap_used)]
#[expect(clippy::expect_used)]
mod tests {
	use std::sync::Arc;

	use super::*;
	use crate::{
		template::Template,
		value::{LazyValue, extract::core::MapExtractor},
	};

	fn str_val(v: &str) -> ELazyValue {
		LazyValue::String(Arc::new(v.into())).into()
	}

	fn obj(name: &str) -> ELazyValue {
		let mut m = HashMap::new();
		m.insert(Label::new("name").unwrap(), str_val(name));
		LazyValue::ObjectExtractor(Arc::new(MapExtractor { inner: m.into() })).into()
	}

	fn arr(items: Vec<ELazyValue>) -> ELazyValue {
		LazyValue::Array(Arc::new(items)).into()
	}

	fn bool_val(b: bool) -> ELazyValue {
		LazyValue::Bool(b).into()
	}

	fn scope(entries: &[(&str, ELazyValue)]) -> HashMap<Label, ELazyValue> {
		entries
			.iter()
			.map(|(k, v)| (Label::new(k).unwrap(), v.clone()))
			.collect()
	}

	/// A root exposing `fields` under `@` and nothing else — these tests exercise
	/// expansion, not any particular source.
	fn root_with(fields: HashMap<Label, ELazyValue>) -> ELazyValue {
		let inner: ELazyValue = LazyValue::ObjectExtractor(Arc::new(MapExtractor {
			inner: fields.into(),
		}))
		.into();
		LazyValue::Root(Arc::new(inner)).into()
	}

	fn empty_root() -> ELazyValue {
		root_with(HashMap::new())
	}

	fn state() -> ExtractState {
		ExtractState::default()
	}

	async fn render(src: &str, scope: HashMap<Label, ELazyValue>) -> String {
		let template = Template::parse(src).unwrap();
		let state = ExtractState::default();
		expand(&template.nodes, scope, &state, &empty_root())
			.await
			.unwrap()
			.text
	}

	async fn expansion(
		src: &str,
		scope: HashMap<Label, ELazyValue>,
		root: &ELazyValue,
	) -> Expansion {
		let template = Template::parse(src).unwrap();
		let state = ExtractState::default();
		expand(&template.nodes, scope, &state, root).await.unwrap()
	}

	#[tokio::test]
	async fn literal_only() {
		assert_eq!(render("just text", scope(&[])).await, "just text");
	}

	#[tokio::test]
	async fn substitution_of_scope_var() {
		assert_eq!(
			render("hi {{ x }}!", scope(&[("x", str_val("there"))])).await,
			"hi there!",
		);
	}

	#[tokio::test]
	async fn substitution_field_of_scope_var() {
		assert_eq!(
			render("{{ x.name }}", scope(&[("x", obj("bee"))])).await,
			"bee"
		);
	}

	#[tokio::test]
	async fn scope_var_is_not_reachable_via_root() {
		// A binding is lexical: reachable as `x`, but never grafted onto `@`.
		assert_eq!(
			render("[{{ @.x }}]", scope(&[("x", str_val("there"))])).await,
			"[]",
		);
	}

	#[tokio::test]
	async fn number_literal_renders_without_quotes() {
		assert_eq!(render("n={{ 42 }}", scope(&[])).await, "n=42");
	}

	#[tokio::test]
	async fn missing_path_renders_empty() {
		assert_eq!(render("[{{ @.nope }}]", scope(&[])).await, "[]");
	}

	#[tokio::test]
	async fn substitution_from_root() {
		let root = root_with(scope(&[("who", str_val("world"))]));
		assert_eq!(
			expansion("hi {{ @.who }}", HashMap::new(), &root)
				.await
				.text,
			"hi world",
		);
	}

	#[tokio::test]
	async fn for_loop_over_scope_array() {
		let items = arr(vec![obj("a"), obj("b"), obj("c")]);
		assert_eq!(
			render(
				"{{ for it in items }}{{ it.name }},{{ endfor }}",
				scope(&[("items", items)]),
			)
			.await,
			"a,b,c,",
		);
	}

	#[tokio::test]
	async fn for_loop_sees_outer_scope_var() {
		let items = arr(vec![str_val("x"), str_val("y")]);
		// The outer `sep` binding stays visible inside the loop body.
		assert_eq!(
			render(
				"{{ for v in items }}{{ v }}{{ sep }}{{ endfor }}",
				scope(&[("items", items), ("sep", str_val("|"))]),
			)
			.await,
			"x|y|",
		);
	}

	#[tokio::test]
	async fn nested_for_loops() {
		let rows = arr(vec![
			arr(vec![str_val("1"), str_val("2")]),
			arr(vec![str_val("3")]),
		]);
		assert_eq!(
			render(
				"{{ for row in rows }}{{ for c in row }}{{ c }}{{ endfor }};{{ endfor }}",
				scope(&[("rows", rows)]),
			)
			.await,
			"12;3;",
		);
	}

	#[tokio::test]
	async fn invalid_substitution_expression_errors() {
		let template = Template::parse("{{ @.a == }}").unwrap();
		let state = ExtractState::default();
		assert!(
			expand(&template.nodes, HashMap::new(), &state, &empty_root())
				.await
				.is_err()
		);
	}

	#[tokio::test]
	async fn if_true_renders_body() {
		assert_eq!(
			render(
				"{{ if flag }}yes{{ endif }}",
				scope(&[("flag", bool_val(true))])
			)
			.await,
			"yes",
		);
	}

	#[tokio::test]
	async fn if_false_renders_nothing() {
		assert_eq!(
			render(
				"[{{ if flag }}yes{{ endif }}]",
				scope(&[("flag", bool_val(false))]),
			)
			.await,
			"[]",
		);
	}

	#[tokio::test]
	async fn if_false_renders_else_branch() {
		assert_eq!(
			render(
				"{{ if flag }}yes{{ else }}no{{ endif }}",
				scope(&[("flag", bool_val(false))]),
			)
			.await,
			"no",
		);
	}

	#[tokio::test]
	async fn if_null_condition_is_falsy() {
		// A missing path evaluates to null, which is falsy — the else-branch wins.
		assert_eq!(
			render("{{ if @.nope }}y{{ else }}n{{ endif }}", scope(&[])).await,
			"n",
		);
	}

	#[tokio::test]
	async fn if_string_condition_is_truthy() {
		// Any non-null, non-false value is truthy, so the then-branch renders.
		assert_eq!(
			render(
				"{{ if x }}y{{ else }}n{{ endif }}",
				scope(&[("x", str_val("hi"))])
			)
			.await,
			"y",
		);
	}

	#[tokio::test]
	async fn if_with_weight_threshold() {
		let tmpl = "{{ if w >= 0.7 }}BIG{{ else }}SMALL{{ endif }}";
		assert_eq!(render(tmpl, scope(&[("w", str_val("0.8"))])).await, "BIG");
		assert_eq!(render(tmpl, scope(&[("w", str_val("0.6"))])).await, "SMALL");
	}

	#[tokio::test]
	async fn if_body_sees_scope_and_loop_var() {
		// A binding from an enclosing loop stays visible inside an `if` body.
		let items = arr(vec![str_val("a"), str_val("b")]);
		assert_eq!(
			render(
				"{{ for v in items }}{{ if v }}{{ v }},{{ endif }}{{ endfor }}",
				scope(&[("items", items)]),
			)
			.await,
			"a,b,",
		);
	}

	#[tokio::test]
	async fn let_binds_value_for_later_substitution() {
		assert_eq!(
			render("{{ let u = x }}{{ u.name }}", scope(&[("x", obj("bee"))])).await,
			"bee",
		);
	}

	#[tokio::test]
	async fn let_emits_nothing_itself() {
		assert_eq!(
			render("[{{ let u = x }}]", scope(&[("x", obj("bee"))])).await,
			"[]",
		);
	}

	#[tokio::test]
	async fn let_from_root() {
		let root = root_with(scope(&[("uid", str_val("abc-123"))]));
		assert_eq!(
			expansion("{{ let u = @.uid }}id={{ u }}", HashMap::new(), &root)
				.await
				.text,
			"id=abc-123",
		);
	}

	#[tokio::test]
	async fn repeated_let_overwrites() {
		assert_eq!(
			render(
				"{{ let v = a }}{{ let v = b }}{{ v.name }}",
				scope(&[("a", obj("first")), ("b", obj("second"))]),
			)
			.await,
			"second",
		);
	}

	#[tokio::test]
	async fn let_visible_in_following_if() {
		assert_eq!(
			render(
				"{{ let flag = t }}{{ if flag }}yes{{ endif }}",
				scope(&[("t", bool_val(true))]),
			)
			.await,
			"yes",
		);
	}

	#[tokio::test]
	async fn let_visible_in_following_for() {
		let items = arr(vec![obj("a"), obj("b")]);
		assert_eq!(
			render(
				"{{ let xs = items }}{{ for it in xs }}{{ it.name }},{{ endfor }}",
				scope(&[("items", items)]),
			)
			.await,
			"a,b,",
		);
	}

	#[tokio::test]
	async fn let_in_taken_if_branch_outlives_it() {
		// The branch that renders binds for everything after the `{{ endif }}`,
		// so a value the `if` is choosing between can be bound where it is chosen.
		assert_eq!(
			render(
				"{{ if flag }}{{ let pick = \"then\" }}{{ else }}{{ let pick = \"else\" }}{{ endif }}{{ pick }}",
				scope(&[("flag", LazyValue::Bool(true).into())]),
			)
			.await,
			"then",
		);

		assert_eq!(
			render(
				"{{ if flag }}{{ let pick = \"then\" }}{{ else }}{{ let pick = \"else\" }}{{ endif }}{{ pick }}",
				scope(&[("flag", LazyValue::Bool(false).into())]),
			)
			.await,
			"else",
		);
	}

	#[tokio::test]
	async fn let_in_untaken_if_branch_binds_nothing() {
		// Only the branch that ran can bind: the outer binding still stands.
		assert_eq!(
			render(
				"{{ if flag }}{{ let pick = \"then\" }}{{ endif }}{{ pick }}",
				scope(&[
					("flag", LazyValue::Bool(false).into()),
					("pick", str_val("outer")),
				]),
			)
			.await,
			"outer",
		);
	}

	#[tokio::test]
	async fn let_in_if_branch_shadows_an_outer_binding() {
		assert_eq!(
			render(
				"{{ if flag }}{{ let pick = \"inner\" }}{{ endif }}{{ pick }}",
				scope(&[
					("flag", LazyValue::Bool(true).into()),
					("pick", str_val("outer")),
				]),
			)
			.await,
			"inner",
		);
	}

	#[tokio::test]
	async fn let_in_for_body_is_scoped_per_iteration() {
		// A `let` inside a loop body binds only within that iteration; it does not
		// leak into the next one or out of the loop.
		let items = arr(vec![obj("a"), obj("b")]);
		assert_eq!(
			render(
				"{{ for it in items }}{{ let n = it.name }}{{ n }},{{ endfor }}",
				scope(&[("items", items)]),
			)
			.await,
			"a,b,",
		);
	}

	#[tokio::test]
	async fn later_let_can_reference_earlier_one() {
		assert_eq!(
			render(
				"{{ let a = x }}{{ let b = a.name }}{{ b }}",
				scope(&[("x", obj("bee"))]),
			)
			.await,
			"bee",
		);
	}

	//
	// MARK: bare expressions
	//

	#[tokio::test]
	async fn expr_renders_scope_var() {
		let exp = expand_expr(
			"x.name",
			scope(&[("x", obj("bee"))]),
			&state(),
			&empty_root(),
		)
		.await
		.unwrap();
		assert_eq!(exp.text, "bee");
	}

	#[tokio::test]
	async fn expr_renders_from_root() {
		let root = root_with(scope(&[("url", str_val("data:image/png;base64,AA"))]));
		let exp = expand_expr("@.url", HashMap::new(), &state(), &root)
			.await
			.unwrap();
		assert_eq!(exp.text, "data:image/png;base64,AA");
	}

	/// A bare expression is *not* a template: `{{ … }}` has no special meaning and
	/// literal text around it is a parse error, not something rendered verbatim.
	#[tokio::test]
	async fn expr_is_not_a_template() {
		assert!(
			expand_expr(
				"url={{ x }}",
				scope(&[("x", str_val("v"))]),
				&state(),
				&empty_root()
			)
			.await
			.is_err()
		);
	}

	#[tokio::test]
	async fn expr_missing_path_renders_empty() {
		let exp = expand_expr("@.nope", HashMap::new(), &state(), &empty_root())
			.await
			.unwrap();
		assert_eq!(exp.text, "");
	}

	#[tokio::test]
	async fn expr_parse_error_is_an_error() {
		assert!(
			expand_expr("@.a ==", HashMap::new(), &state(), &empty_root())
				.await
				.is_err()
		);
	}

	/// A fully-resolved template defers nothing.
	#[tokio::test]
	async fn ready_template_defers_nothing() {
		let exp = expansion(
			"hi {{ x }}",
			scope(&[("x", str_val("there"))]),
			&empty_root(),
		)
		.await;
		assert_eq!(exp.text, "hi there");
	}
}
