use parking_lot::RwLock;
use std::collections::HashMap;

use crate::Label;
use crate::value::ELazyValue;
use crate::value::extract::EvalError;
use crate::value::extract::ExtractState;
use crate::value::extract::traits::ObjectExtractor;

#[derive(Default)]
pub struct MapExtractor {
	pub inner: RwLock<HashMap<Label, ELazyValue>>,
}

#[async_trait::async_trait]
impl ObjectExtractor for MapExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, EvalError> {
		if args.is_some() {
			return state.unexpected_args(name);
		}

		state.resolve_field(name, self.inner.read().get(name).cloned())
	}

	async fn fields(&self, _state: &ExtractState) -> Result<Vec<Label>, EvalError> {
		Ok(self.inner.read().keys().cloned().collect())
	}
}
