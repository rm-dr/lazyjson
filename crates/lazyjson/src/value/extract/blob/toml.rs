use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

use crate::AsyncReader;
use crate::Label;
use crate::value::extract::ExtractError;
use crate::value::extract::ExtractState;
use crate::value::extract::traits::ObjectExtractor;
use crate::value::{BinaryLazyValue, ELazyValue, LazyValue};

fn toml_to_lazy(value: toml::Value) -> ELazyValue {
	match value {
		toml::Value::String(s) => LazyValue::String(Arc::new(s.into())),
		toml::Value::Integer(i) => LazyValue::String(Arc::new(i.to_string().into())),
		toml::Value::Float(f) => LazyValue::String(Arc::new(f.to_string().into())),
		toml::Value::Boolean(b) => LazyValue::String(Arc::new(b.to_string().into())),
		toml::Value::Datetime(d) => LazyValue::String(Arc::new(d.to_string().into())),
		toml::Value::Array(a) => {
			LazyValue::Array(Arc::new(a.into_iter().map(toml_to_lazy).collect()))
		}
		toml::Value::Table(_) => LazyValue::Null,
	}
	.into()
}

pub struct TomlExtractor {
	item: BinaryLazyValue,
	output: Mutex<Option<HashMap<Label, ELazyValue>>>,
}

impl TomlExtractor {
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
		if !state.ignore_mime && self.item.mime().type_() != mime::TEXT {
			return Ok(HashMap::new());
		}

		let mut reader = self.item.read().await?;
		let bytes = reader.read_to_end().await?;
		let toml: toml::Value = match toml::from_slice(&bytes) {
			Ok(x) => x,
			Err(_) => return Ok(HashMap::new()),
		};

		let output: HashMap<Label, ELazyValue> = match toml {
			toml::Value::Table(t) => t
				.into_iter()
				.filter_map(|(k, v)| Label::new(&k).map(|label| (label, toml_to_lazy(v))))
				.collect(),
			_ => HashMap::new(),
		};

		return Ok(output);
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for TomlExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, ExtractError> {
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

	async fn fields(&self, state: &ExtractState) -> Result<Vec<Label>, ExtractError> {
		let mut out = self.output.lock().await;
		if out.is_none() {
			*out = Some(self.compute(state).await?);
		}

		#[expect(clippy::unwrap_used)]
		return Ok(out.as_ref().unwrap().keys().cloned().collect());
	}
}
