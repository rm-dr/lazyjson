use std::sync::Arc;
use tokio::sync::Mutex;

use crate::AsyncReader;
use crate::Label;
use crate::value::extract::EvalError;
use crate::value::extract::ExtractState;
use crate::value::extract::traits::ObjectExtractor;
use crate::value::{BinaryLazyValue, ELazyValue, LazyValue};

pub struct TextExtractor {
	item: BinaryLazyValue,
	output: Mutex<Option<ELazyValue>>,
}

impl TextExtractor {
	pub fn new(item: &BinaryLazyValue) -> Self {
		Self {
			item: item.clone(),
			output: Mutex::new(None),
		}
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for TextExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, EvalError> {
		if args.is_some() {
			return state.unexpected_args(name);
		}

		if name.as_str() != "text" {
			return state.unknown_field(name);
		}

		if !state.ignore_mime
			&& (self.item.mime().type_() != mime::TEXT
				&& self.item.mime().type_() != mime::APPLICATION)
		{
			return Ok(None);
		}

		let mut out = self.output.lock().await;
		if out.is_none() {
			let mut reader = self.item.read().await?;
			let bytes = reader.read_to_end().await?;
			let string = String::from_utf8(bytes).ok();
			let value = match string {
				Some(x) => LazyValue::String(Arc::new(x.into())),
				None => LazyValue::Null,
			};

			*out = Some(value.into());
		}

		#[expect(clippy::unwrap_used)]
		return Ok(Some(out.as_ref().unwrap().clone()));
	}

	#[expect(clippy::unwrap_used)]
	async fn fields(&self, _state: &ExtractState) -> Result<Vec<Label>, EvalError> {
		Ok(vec![Label::new("text").unwrap()])
	}
}
