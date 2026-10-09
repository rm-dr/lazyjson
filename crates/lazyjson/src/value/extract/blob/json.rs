use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

use crate::AsyncReader;
use crate::Label;
use crate::value::extract::EvalError;
use crate::value::extract::ExtractState;
use crate::value::extract::traits::ObjectExtractor;
use crate::value::{BinaryLazyValue, ELazyValue, LazyValue};

fn json_to_lazy(value: serde_json::Value) -> ELazyValue {
	match value {
		serde_json::Value::Null => LazyValue::Null,
		serde_json::Value::Bool(b) => LazyValue::String(Arc::new(b.to_string().into())),
		serde_json::Value::Number(n) => LazyValue::String(Arc::new(n.to_string().into())),
		serde_json::Value::String(s) => LazyValue::String(Arc::new(s.into())),
		serde_json::Value::Array(a) => {
			LazyValue::Array(Arc::new(a.into_iter().map(json_to_lazy).collect()))
		}
		serde_json::Value::Object(_) => LazyValue::Null,
	}
	.into()
}

pub struct JsonExtractor {
	item: BinaryLazyValue,
	output: Mutex<Option<HashMap<Label, ELazyValue>>>,
}

impl JsonExtractor {
	pub fn new(item: &BinaryLazyValue) -> Self {
		Self {
			item: item.clone(),
			output: Mutex::new(None),
		}
	}

	async fn compute(
		&self,
		state: &ExtractState,
	) -> Result<HashMap<Label, ELazyValue>, std::io::Error> {
		if !state.ignore_mime
			&& (self.item.mime().type_() != mime::APPLICATION
				&& self.item.mime().type_() != mime::TEXT)
		{
			return Ok(HashMap::new());
		}

		let mut reader = self.item.read().await?;
		let bytes = reader.read_to_end().await?;
		let json: serde_json::Value = match serde_json::from_slice(&bytes) {
			Ok(x) => x,
			Err(_) => return Ok(HashMap::new()),
		};

		let output: HashMap<Label, ELazyValue> = match json {
			serde_json::Value::Object(map) => map
				.into_iter()
				.filter_map(|(k, v)| Label::new(&k).map(|label| (label, json_to_lazy(v))))
				.collect(),
			_ => HashMap::new(),
		};

		return Ok(output);
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for JsonExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, EvalError> {
		if args.is_some() {
			return state.unexpected_args(name);
		}

		let mut out = self.output.lock().await;
		if out.is_none() {
			*out = Some(self.compute(state).await?);
		}

		#[expect(clippy::unwrap_used)]
		let value = out.as_ref().unwrap().get(name).cloned();
		state.resolve_field(name, value)
	}

	async fn fields(&self, state: &ExtractState) -> Result<Vec<Label>, EvalError> {
		let mut out = self.output.lock().await;
		if out.is_none() {
			*out = Some(self.compute(state).await?);
		}

		#[expect(clippy::unwrap_used)]
		return Ok(out.as_ref().unwrap().keys().cloned().collect());
	}
}
