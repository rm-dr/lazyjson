use std::pin::Pin;
use std::sync::Arc;

use crate::{
	expr::tree::LazyExpressionTree,
	value::{
		ELazyValue, LazyValue,
		extract::{ExtractError, ExtractState},
	},
};

pub struct ExpressionEvaluator<'a> {
	pub root: &'a ELazyValue,
	pub state: &'a ExtractState,
}

/// True if `expr` is a path node, i.e. one that reads from the value its
/// `left` produces. Used to log each path once, at its outermost node.
fn is_path(expr: &LazyExpressionTree) -> bool {
	matches!(
		expr,
		LazyExpressionTree::Root
			| LazyExpressionTree::Alias(_)
			| LazyExpressionTree::Field { .. }
			| LazyExpressionTree::Index { .. }
			| LazyExpressionTree::Slice { .. }
	)
}

impl ExpressionEvaluator<'_> {
	/// Evaluate an expression. A path that resolves to nothing is `null`.
	pub fn eval<'a>(
		&'a self,
		expr: &'a LazyExpressionTree,
	) -> Pin<Box<impl Future<Output = Result<ELazyValue, ExtractError>> + 'a>> {
		Box::pin(async move {
			Ok(self
				.eval_node(expr, true)
				.await?
				.unwrap_or(LazyValue::Null.into()))
		})
	}

	/// Evaluate one node.
	fn eval_node<'a>(
		&'a self,
		expr: &'a LazyExpressionTree,
		root: bool,
	) -> Pin<Box<dyn Future<Output = Result<Option<ELazyValue>, ExtractError>> + 'a>> {
		Box::pin(async move {
			if root
				&& is_path(expr)
				&& let Some(log) = &self.state.access_log
			{
				log.lock().push(expr.clone());
			}

			Ok(match expr {
				LazyExpressionTree::Root => Some(self.root.clone()),
				LazyExpressionTree::Literal(x) => Some(x.clone()),

				LazyExpressionTree::Alias(name) => {
					let bound = self.state.aliases.get(name).ok_or_else(|| {
						ExtractError::BadField(format!("unknown alias {:?}", name.as_str()))
					})?;

					self.eval_node(bound, false).await?
				}

				LazyExpressionTree::Binop { left, op, right } => {
					let left = self.eval(left).await?;
					let right = self.eval(right).await?;
					Some(op.eval(self.state.strict, &left, &right)?)
				}

				LazyExpressionTree::Field { left, field, args } => {
					let Some(left) = self.eval_node(left, false).await? else {
						return Ok(None);
					};

					left.object()
						.field(self.state, field, args.as_deref())
						.await?
				}

				LazyExpressionTree::Index { left, idx } => {
					let Some(left) = self.eval_node(left, false).await? else {
						return Ok(None);
					};

					// The index is an expression of its own — `[0]` and
					// `[@.i]` are both indices — and must be a number.
					let idx = self.eval(idx).await?;
					let Some(idx) = idx.inner.num() else {
						return Err(ExtractError::BadField(format!(
							"index must be a number, got {:?}",
							idx.inner
						)));
					};
					let idx = idx as i64;

					let list = left.list();

					// A negative index counts back from the end.
					let idx = if idx >= 0 {
						usize::try_from(idx).ok()
					} else {
						usize::try_from(list.len(self.state).await? as i64 + idx).ok()
					};

					let Some(idx) = idx else {
						return Ok(None);
					};

					list.get(self.state, idx).await?
				}

				LazyExpressionTree::Slice {
					left,
					start,
					end,
					inclusive,
				} => {
					let Some(left) = self.eval_node(left, false).await? else {
						return Ok(None);
					};

					let list = left.list();
					let len = list.len(self.state).await? as i64;

					// An omitted start is 0; an omitted end is the list length.
					// Negative bounds count back from the end.
					let start_idx = match start {
						Some(s) if *s >= 0 => *s,
						Some(s) => len + s,
						None => 0,
					};
					let end_idx = match end {
						Some(e) if *e >= 0 => *e,
						Some(e) => len + e,
						None => len,
					};
					let end_idx = if *inclusive { end_idx + 1 } else { end_idx };

					let start_idx = start_idx.max(0) as usize;
					let end_idx = (end_idx.max(0) as usize).min(len as usize);

					let mut items = Vec::new();
					for i in start_idx..end_idx {
						match list.get(self.state, i).await? {
							Some(v) => items.push(v),
							None => break,
						}
					}

					// TODO: lazy view?
					Some(LazyValue::Array(Arc::new(items)).into())
				}
			})
		})
	}
}
