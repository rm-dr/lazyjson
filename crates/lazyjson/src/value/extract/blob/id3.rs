use id3::Tag;
use mime::Mime;
use std::{borrow::Cow, collections::HashMap, io::BufReader, sync::Arc};
use tokio::sync::Mutex;
use tracing::trace;

use crate::{
	Label, SyncReadBridge,
	value::{
		ArcBytes, BinaryLazyValue, ELazyValue, LazyValue,
		extract::{
			EvalError, ExtractState,
			traits::{ListExtractor, ObjectExtractor},
		},
	},
};

pub struct Id3ImagesExtractor {
	item: BinaryLazyValue,
	cached_count: Mutex<Option<usize>>,
}

impl Id3ImagesExtractor {
	pub fn new(item: &BinaryLazyValue) -> Self {
		Self {
			item: item.clone(),
			cached_count: Mutex::new(None),
		}
	}

	async fn read_tag(&self) -> Result<Option<Tag>, std::io::Error> {
		let item = self.item.clone();
		let reader = SyncReadBridge::new_current(self.item.read().await?);
		tokio::task::spawn_blocking(move || match Tag::read_from2(BufReader::new(reader)) {
			Ok(tag) => Ok(Some(tag)),
			Err(id3::Error {
				kind: id3::ErrorKind::Io(e),
				..
			}) => Err(e),
			Err(error) => {
				trace!(message = "Could not parse id3 tags", ?item, ?error);
				Ok(None)
			}
		})
		.await
		.map_err(std::io::Error::other)?
	}

	fn mime_ok(&self, state: &ExtractState) -> bool {
		state.ignore_mime || self.item.mime().essence_str() == "audio/mpeg"
	}
}

#[async_trait::async_trait]
impl ListExtractor for Id3ImagesExtractor {
	async fn get(&self, state: &ExtractState, idx: usize) -> Result<Option<ELazyValue>, EvalError> {
		if !(self.mime_ok(state) || state.ignore_mime) {
			return Ok(None);
		}

		let Some(tag) = self.read_tag().await? else {
			return Ok(None);
		};

		let Some(picture) = tag.pictures().nth(idx) else {
			return Ok(None);
		};

		let mime: Mime = picture
			.mime_type
			.parse()
			.unwrap_or(mime::APPLICATION_OCTET_STREAM);
		let data = picture.data.clone();

		Ok(Some(
			LazyValue::Binary(BinaryLazyValue::Blob {
				mime,
				bytes: ArcBytes(Arc::new(data)),
			})
			.into(),
		))
	}

	async fn len(&self, state: &ExtractState) -> Result<usize, EvalError> {
		if !(self.mime_ok(state) || state.ignore_mime) {
			return Ok(0);
		}

		let mut cc = self.cached_count.lock().await;
		if cc.is_none() {
			*cc = Some(match self.read_tag().await? {
				Some(tag) => tag.pictures().count(),
				None => 0,
			});
		}

		Ok(cc.unwrap())
	}
}

pub struct Id3Extractor {
	item: BinaryLazyValue,
	output: Mutex<Option<HashMap<Label, ELazyValue>>>,
	images: ELazyValue,
}

impl Id3Extractor {
	pub fn new(item: &BinaryLazyValue) -> Self {
		Self {
			item: item.clone(),
			output: Mutex::new(None),
			images: LazyValue::ListExtractor(Arc::new(Id3ImagesExtractor::new(item))).into(),
		}
	}

	async fn compute(
		&self,
		state: &ExtractState,
	) -> Result<HashMap<Label, ELazyValue>, std::io::Error> {
		if !state.ignore_mime && self.item.mime().essence_str() != "audio/mpeg" {
			return Ok(HashMap::new());
		}

		trace!(message = "Reading id3 tags", key = ?self.item);

		let item = self.item.clone();
		let reader = SyncReadBridge::new_current(self.item.read().await?);
		let tag = match tokio::task::spawn_blocking(move || Tag::read_from2(BufReader::new(reader)))
			.await
		{
			Ok(Ok(tag)) => tag,

			Err(e) => return Err(e.into()),
			Ok(Err(id3::Error {
				kind: id3::ErrorKind::Io(e),
				..
			})) => return Err(e),

			Ok(Err(error)) => {
				trace!(message = "Could not parse id3 tags", ?item, ?error);
				return Ok(HashMap::new());
			}
		};

		let mut output: HashMap<Label, Vec<ELazyValue>> = HashMap::new();
		for frame in tag.frames() {
			if let Some(texts) = frame.content().text_values() {
				let name = frame_id_to_field(frame.id());
				if let Some(key) = Label::new(name) {
					for text in texts {
						output
							.entry(key.clone())
							.or_default()
							.push(LazyValue::String(Arc::new(text.into())).into());
					}
				}
			}
		}

		let output = output
			.into_iter()
			.map(|(k, v)| (k, LazyValue::Array(Arc::new(v)).into()))
			.collect();

		return Ok(output);
	}
}

/// Map an ID3 frame ID to the equivalent Vorbis Comment field name.
/// Falls back to the lowercased frame ID if no mapping exists.
fn frame_id_to_field(id: &str) -> Cow<'static, str> {
	match id {
		// spell:off
		"TIT2" => Cow::Borrowed("title"),
		"TIT1" => Cow::Borrowed("grouping"),
		"TIT3" => Cow::Borrowed("subtitle"),
		"TPE1" => Cow::Borrowed("artist"),
		"TPE2" => Cow::Borrowed("albumartist"),
		"TPE3" => Cow::Borrowed("conductor"),
		"TOPE" => Cow::Borrowed("originalartist"),
		"TALB" => Cow::Borrowed("album"),
		"TOAL" => Cow::Borrowed("originalalbum"),
		"TRCK" => Cow::Borrowed("tracknumber"),
		"TPOS" => Cow::Borrowed("discnumber"),
		"TSST" => Cow::Borrowed("discsubtitle"),
		"TDRC" | "TYER" => Cow::Borrowed("date"),
		"TDOR" | "TORY" => Cow::Borrowed("originaldate"),
		"TCON" => Cow::Borrowed("genre"),
		"TCOM" => Cow::Borrowed("composer"),
		"TEXT" => Cow::Borrowed("lyricist"),
		"TPUB" => Cow::Borrowed("label"),
		"TSRC" => Cow::Borrowed("isrc"),
		"TBPM" => Cow::Borrowed("bpm"),
		"TLAN" => Cow::Borrowed("language"),
		"TMED" => Cow::Borrowed("media"),
		"TMOO" => Cow::Borrowed("mood"),
		"TCOP" => Cow::Borrowed("copyright"),
		"TENC" => Cow::Borrowed("encodedby"),
		"TSSE" => Cow::Borrowed("encodersettings"),
		"TSOA" => Cow::Borrowed("albumsort"),
		"TSOP" => Cow::Borrowed("artistsort"),
		"TSOT" => Cow::Borrowed("titlesort"),
		"MVNM" => Cow::Borrowed("movement"),
		"MVIN" => Cow::Borrowed("movementnumber"),
		_ => Cow::Owned(id.to_lowercase()),
		// spell:on
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for Id3Extractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, EvalError> {
		if args.is_some() {
			return state.unexpected_args(name);
		}

		if name.as_str() == "images" {
			return Ok(Some(self.images.clone()));
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
		Ok(out
			.as_ref()
			.unwrap()
			.keys()
			.cloned()
			.chain([Label::new("images").unwrap()])
			.collect::<Vec<_>>())
	}
}
