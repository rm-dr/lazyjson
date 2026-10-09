use std::sync::Arc;

use crate::{
	Label,
	value::{
		ELazyValue, LazyValue,
		extract::{
			EvalError, ExtractState,
			traits::{ListExtractor, ObjectExtractor},
		},
	},
};

/// The object view over a list value
pub struct ListObjectExtractor {
	list: Arc<dyn ListExtractor>,
}

impl ListObjectExtractor {
	pub fn new(list: Arc<dyn ListExtractor>) -> Self {
		Self { list }
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for ListObjectExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, EvalError> {
		if name.as_str() != "len" {
			return state.unknown_field(name);
		}

		if args.is_some() {
			return state.unexpected_args(name);
		}

		let len = self.list.len(state).await?;
		Ok(Some(LazyValue::U64(len as u64).into()))
	}

	async fn fields(&self, _state: &ExtractState) -> Result<Vec<Label>, EvalError> {
		Ok(Label::new("len").into_iter().collect())
	}
}
