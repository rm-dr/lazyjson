use image::ImageFormat;
use mime::Mime;
use std::{io::Cursor, str::FromStr, sync::Arc};
use tracing::trace;
use transform::{CropTransformer, ImageTransformer, MaxDimTransformer};

mod transform;

use crate::Label;
use crate::asyncreader::AsyncReader;
use crate::value::extract::ExtractError;
use crate::value::extract::ExtractState;
use crate::value::extract::traits::ObjectExtractor;
use crate::value::{ArcBytes, BinaryLazyValue, ELazyValue, LazyValue};

pub struct ImageExtractor {
	item: BinaryLazyValue,
}

impl ImageExtractor {
	pub fn new(item: &BinaryLazyValue) -> Self {
		Self { item: item.clone() }
	}

	async fn apply<T: ImageTransformer + Send + 'static>(
		&self,
		args: &str,
	) -> Result<Option<ELazyValue>, ExtractError> {
		let transformer = match T::parse_args(args) {
			Ok(t) => t,
			Err(_) => return Ok(None),
		};

		let mime = self.item.mime().clone();
		let bytes = self.item.read().await?.read_to_end().await?;

		let Some(format) = ImageFormat::from_mime_type(&mime) else {
			return Ok(Some(
				LazyValue::Binary(BinaryLazyValue::Blob {
					mime,
					bytes: ArcBytes(Arc::new(bytes)),
				})
				.into(),
			));
		};

		let bytes_for_closure = bytes.clone();
		let result = tokio::task::spawn_blocking(move || {
			let mut img = image::load_from_memory_with_format(&bytes_for_closure, format)?;
			transformer.transform(&mut img);

			let mut out = Cursor::new(Vec::new());
			img.write_to(&mut out, format)?;

			let out_mime =
				Mime::from_str(format.to_mime_type()).unwrap_or(mime::APPLICATION_OCTET_STREAM);
			Ok::<_, image::ImageError>((out_mime, out.into_inner()))
		})
		.await?;

		match result {
			Ok((out_mime, out_bytes)) => Ok(Some(
				LazyValue::Binary(BinaryLazyValue::Blob {
					mime: out_mime,
					bytes: ArcBytes(Arc::new(out_bytes)),
				})
				.into(),
			)),

			Err(_) => Ok(Some(
				LazyValue::Binary(BinaryLazyValue::Blob {
					mime,
					bytes: ArcBytes(Arc::new(bytes)),
				})
				.into(),
			)),
		}
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for ImageExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, ExtractError> {
		let Some(args) = args else {
			return Ok(None);
		};

		trace!(?args, "Getting field {name:?} from ImageExtractor");

		match name.as_str() {
			"maxdim" => self.apply::<MaxDimTransformer>(args).await,
			"crop" => self.apply::<CropTransformer>(args).await,
			_ => state.unknown_field(name),
		}
	}

	#[expect(clippy::unwrap_used)]
	async fn fields(&self, _state: &ExtractState) -> Result<Vec<Label>, ExtractError> {
		Ok(vec![
			Label::new("maxdim").unwrap(),
			Label::new("crop").unwrap(),
		])
	}
}
