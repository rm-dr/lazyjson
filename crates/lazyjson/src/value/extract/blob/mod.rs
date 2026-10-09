use crate::value::extract::ExtractState;
use std::{collections::HashMap, sync::Arc};

mod json;
pub use json::*;

mod text;
pub use text::*;

#[cfg(feature = "image")]
mod image;
#[cfg(feature = "image")]
pub use image::*;

#[cfg(feature = "id3")]
mod id3;
#[cfg(feature = "id3")]
pub use id3::*;

#[cfg(feature = "exif")]
mod exif;
#[cfg(feature = "exif")]
pub use exif::*;

#[cfg(feature = "toml")]
mod toml;
#[cfg(feature = "toml")]
pub use toml::*;

#[cfg(feature = "hash")]
mod hash;
#[cfg(feature = "hash")]
pub use hash::*;

use crate::{
	Label,
	value::{
		BinaryLazyValue, ELazyValue, LazyValue,
		extract::{EvalError, core::MapExtractor, traits::ObjectExtractor},
	},
};

pub struct BinaryExtractor {
	inner: MapExtractor,

	#[cfg(feature = "image")]
	image: Arc<ImageExtractor>,
}

impl BinaryExtractor {
	#[expect(clippy::unwrap_used)]
	pub fn new(item: &BinaryLazyValue) -> Self {
		let inner = MapExtractor {
			inner: HashMap::from([
				#[cfg(feature = "id3")]
				(
					Label::new("id3").unwrap(),
					LazyValue::ObjectExtractor(Arc::new(Id3Extractor::new(item))).into(),
				),
				#[cfg(feature = "exif")]
				(
					Label::new("exif").unwrap(),
					LazyValue::ObjectExtractor(Arc::new(ExifExtractor::new(item))).into(),
				),
				#[cfg(feature = "toml")]
				(
					Label::new("toml").unwrap(),
					LazyValue::ObjectExtractor(Arc::new(TomlExtractor::new(item))).into(),
				),
				#[cfg(feature = "hash")]
				(
					Label::new("hash").unwrap(),
					LazyValue::ObjectExtractor(Arc::new(HashExtractor::new(item))).into(),
				),
				(
					Label::new("json").unwrap(),
					LazyValue::ObjectExtractor(Arc::new(JsonExtractor::new(item))).into(),
				),
				(
					Label::new("text").unwrap(),
					LazyValue::ObjectExtractor(Arc::new(TextExtractor::new(item))).into(),
				),
			])
			.into(),
		};

		Self {
			inner,

			#[cfg(feature = "image")]
			image: Arc::new(ImageExtractor::new(item)),
		}
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for BinaryExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &crate::Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, EvalError> {
		#[cfg(feature = "image")]
		if self.image.fields(state).await?.contains(name) {
			return self.image.field(state, name, args).await;
		}

		self.inner.field(state, name, args).await
	}

	#[expect(clippy::allow_attributes)]
	#[allow(unused_mut)]
	async fn fields(&self, state: &ExtractState) -> Result<Vec<Label>, EvalError> {
		let mut fields = self.inner.fields(state).await?;

		#[cfg(feature = "image")]
		fields.extend(self.image.fields(state).await?);

		Ok(fields)
	}
}
