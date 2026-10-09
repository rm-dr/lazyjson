use crate::Label;
use crate::value::extract::ExtractState;
use crate::value::{ELazyValue, extract::ExtractError};

/// An attachment that extracts metadata from an [Item].
///
/// Metadata is exposed as an immutable map of {label: value},
/// much like a json object.
#[async_trait::async_trait]
pub trait ObjectExtractor: Send + Sync + std::any::Any {
	/// Get the field at `name` from `item`.
	/// - returns `None` if `name` is not a valid field
	/// - returns `Some(Null)` if `name` is not available
	///
	/// For extractors that parse binary, this fn should return
	/// an error only if we failed to obtain the data we need (permission denied, etc).
	///
	/// If the underlying data has an invalid format (e.g, running a pdf extractor on a non-pdf file),
	/// this fn should return `Ok(Some(None))`.
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, ExtractError>;

	/// Return all fields in this extractor.
	/// `Self::field` must return [Some] for all these keys
	/// and [None] for all others.
	///
	/// The output of this field is not constant, different items
	/// can produce different fields when processed by the same extractor.
	async fn fields(&self, state: &ExtractState) -> Result<Vec<Label>, ExtractError>;

	/// Return a list view of this extractor, if supported.
	fn as_list(&self) -> Option<std::sync::Arc<dyn ListExtractor>> {
		None
	}

	/// Convert this to a JSON value.
	async fn to_json(
		&self,
		state: &ExtractState,
	) -> Result<Option<serde_json::Value>, ExtractError> {
		let keys = self.fields(state).await?;
		let mut map = serde_json::Map::new();
		for k in &keys {
			let v = match self.field(state, k, None).await? {
				Some(x) => x,
				None => continue,
			};

			let j = Box::pin(v.to_json(state)).await?;
			if let Some(j) = j {
				map.insert(k.to_string(), j);
			}
		}

		Ok(Some(serde_json::Value::Object(map)))
	}
}

/// An attachment that extracts metadata from an [Item].
///
/// Metadata is exposed as an immutable list of values.
#[async_trait::async_trait]
pub trait ListExtractor: Send + Sync {
	/// Get the item at index `idx`.
	/// Indices start at zero, and must be consecutive.
	/// - returns `None` if `idx` is out of range
	/// - returns `Some(Null)` if `None` is at `idx`
	async fn get(
		&self,
		state: &ExtractState,
		idx: usize,
	) -> Result<Option<ELazyValue>, ExtractError>;

	async fn len(&self, state: &ExtractState) -> Result<usize, ExtractError>;

	/// Convert this list to a JSON value.
	async fn to_json(
		&self,
		state: &ExtractState,
	) -> Result<Option<serde_json::Value>, ExtractError> {
		let len = self.len(state).await?;
		let mut list = Vec::with_capacity(len);
		for i in 0..len {
			#[expect(clippy::expect_used)]
			let v = self
				.get(state, i)
				.await?
				.expect("value must be present according to length");

			let j = Box::pin(v.to_json(state)).await?;
			if let Some(j) = j {
				list.push(j);
			}
		}

		Ok(Some(serde_json::Value::Array(list)))
	}
}
