use std::sync::Arc;

use crate::value::{
	ELazyValue,
	extract::ExtractState,
	extract::{EvalError, traits::ListExtractor},
};

#[derive(Default)]
pub struct VecExtractor {
	pub inner: Arc<Vec<ELazyValue>>,
}

impl VecExtractor {
	pub fn new(inner: Arc<Vec<ELazyValue>>) -> Self {
		Self { inner }
	}
}

#[async_trait::async_trait]
impl ListExtractor for VecExtractor {
	async fn get(
		&self,
		_state: &ExtractState,
		idx: usize,
	) -> Result<Option<ELazyValue>, EvalError> {
		Ok(self.inner.get(idx).cloned())
	}

	async fn len(&self, _state: &ExtractState) -> Result<usize, EvalError> {
		Ok(self.inner.len())
	}
}
