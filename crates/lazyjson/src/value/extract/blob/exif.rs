use std::{collections::HashMap, io::BufReader, sync::Arc};
use tokio::sync::Mutex;
use tracing::trace;

use crate::{
	Label, SyncReadBridge,
	value::{
		BinaryLazyValue, ELazyValue, LazyValue,
		extract::{EvalError, ExtractState, traits::ObjectExtractor},
	},
};

pub struct ExifExtractor {
	item: BinaryLazyValue,
	output: Mutex<Option<HashMap<Label, ELazyValue>>>,
}

impl ExifExtractor {
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
		if !state.ignore_mime && self.item.mime().type_() != mime::IMAGE {
			return Ok(HashMap::new());
		}

		let reader = SyncReadBridge::new_current(self.item.read().await?);
		let raw_fields = tokio::task::spawn_blocking(move || {
			let mut br = BufReader::new(reader);
			let exif = exif::Reader::new().read_from_container(&mut br)?;

			let fields: Vec<(String, String)> = exif
				.fields()
				.map(|f| {
					(
						f.tag.to_string(),
						f.display_value().with_unit(&exif).to_string(),
					)
				})
				.collect();

			Ok::<_, exif::Error>(fields)
		})
		.await?;

		let raw_fields = match raw_fields {
			Ok(x) => x,
			Err(exif::Error::Io(x)) => return Err(x),
			Err(error) => {
				trace!(message = "Could not process exif", ?error, item = ?self.item);
				return Ok(HashMap::new());
			}
		};

		let mut output: HashMap<Label, ELazyValue> = HashMap::new();

		for (tag_name, value) in raw_fields {
			let Some(label) = tag_to_label(&tag_name) else {
				continue;
			};

			// First occurrence wins (PRIMARY IFD comes before THUMBNAIL)
			output
				.entry(label)
				.or_insert_with(|| LazyValue::String(Arc::new(value.into())).into());
		}

		return Ok(output);
	}
}

fn tag_to_label(tag: &str) -> Option<Label> {
	let sanitized: String = tag
		.chars()
		.map(|c| if c == ' ' { '_' } else { c })
		.filter(|c| Label::is_valid(*c))
		.collect();
	Label::new(sanitized)
}

#[async_trait::async_trait]
impl ObjectExtractor for ExifExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, EvalError> {
		trace!(
			?args,
			item = ?self.item,
			"Getting field {name:?} from ExifExtractor",
		);

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
		Ok(out.as_ref().unwrap().keys().cloned().collect())
	}
}
