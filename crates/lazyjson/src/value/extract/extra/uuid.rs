use smartstring::{LazyCompact, SmartString};
use std::sync::Arc;
use uuid::Uuid;

use crate::value::extract::{ExtractError, ExtractState};
use crate::{
	Label,
	value::{
		ELazyValue, LazyValue,
		extract::{extra::StringExtractor, traits::ObjectExtractor},
	},
};

const UUID_FIELDS: [&str; 4] = ["hex", "urn", "short", "version"];

pub struct UuidExtractor {
	uuid: Uuid,

	/// The canonical hyphenated form, formatted once at construction. Also the
	/// fallthrough for string methods.
	text: Arc<SmartString<LazyCompact>>,
	string: StringExtractor,
}

impl UuidExtractor {
	pub fn new(uuid: Uuid) -> Self {
		let mut buf = [0u8; uuid::fmt::Hyphenated::LENGTH];
		let text: Arc<SmartString<LazyCompact>> =
			Arc::new(uuid.hyphenated().encode_lower(&mut buf).into());
		Self {
			uuid,
			string: StringExtractor::new(&text),
			text,
		}
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for UuidExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, ExtractError> {
		// A uuid method takes no arguments; `.hex(x)` is a mistake, not a string
		// method, so it must not fall through to `StringExtractor`.
		if args.is_some() && UUID_FIELDS.contains(&name.as_str()) {
			return state.unexpected_args(name);
		}

		Ok(match (name.as_str(), args) {
			("hex", None) => {
				let mut buf = [0u8; uuid::fmt::Simple::LENGTH];
				Some(LazyValue::String(Arc::new(
					self.uuid.simple().encode_lower(&mut buf).into(),
				)))
			}

			("urn", None) => {
				let mut buf = [0u8; uuid::fmt::Urn::LENGTH];
				Some(LazyValue::String(Arc::new(
					self.uuid.urn().encode_lower(&mut buf).into(),
				)))
			}

			// The leading 8 hex characters, as a display prefix (git-style).
			//
			// NOT a unique key: in a v7 uuid these are the high bits of the
			// timestamp, so two ids minted in the same millisecond share them.
			("short", None) => Some(LazyValue::String(Arc::new(
				self.text.as_str().get(..8).unwrap_or_default().into(),
			))),

			("version", None) => Some(match self.uuid.get_version_num() {
				0 => LazyValue::Null,
				v => LazyValue::U64(v as u64),
			}),

			// v1 / v6 / v7 embed a timestamp; every other version has none.
			// Yielding a `Timestamp` means it composes with `as_of` arguments and
			// numeric comparison for free.
			("timestamp", None) => Some(match self.uuid.get_timestamp() {
				Some(ts) => {
					let (secs, _nanos) = ts.to_unix();
					match i64::try_from(secs) {
						Ok(secs) => LazyValue::Timestamp(secs),
						Err(_) => LazyValue::Null,
					}
				}
				None => LazyValue::Null,
			}),

			_ => return self.string.field(state, name, args).await,
		}
		.map(Into::into))
	}

	async fn fields(&self, state: &ExtractState) -> Result<Vec<Label>, ExtractError> {
		let mut fields: Vec<Label> = UUID_FIELDS
			.iter()
			.chain(std::iter::once(&"timestamp"))
			.filter_map(Label::new)
			.collect();
		fields.extend(self.string.fields(state).await?);
		Ok(fields)
	}
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
	use super::*;

	/// A v7 uuid whose embedded timestamp is exactly 2024-05-12T00:00:00Z
	/// (unix ms `0x018f6a196400`).
	fn v7() -> Uuid {
		Uuid::parse_str("018f6a19-6400-7000-8000-000000000000").unwrap()
	}

	fn v4() -> Uuid {
		Uuid::parse_str("f47ac10b-58cc-4372-a567-0e02b2c3d479").unwrap()
	}

	async fn field(uuid: Uuid, name: &str, args: Option<&str>) -> Option<LazyValue> {
		UuidExtractor::new(uuid)
			.field(&ExtractState::default(), &Label::new(name).unwrap(), args)
			.await
			.unwrap()
			.map(|x| x.inner)
	}

	fn string(v: Option<LazyValue>) -> String {
		match v {
			Some(LazyValue::String(s)) => s.as_str().to_owned(),
			other => panic!("expected a string, got {other:?}"),
		}
	}

	#[tokio::test]
	async fn hex_urn_and_short() {
		assert_eq!(
			string(field(v7(), "hex", None).await),
			"018f6a19640070008000000000000000"
		);
		assert_eq!(
			string(field(v7(), "urn", None).await),
			"urn:uuid:018f6a19-6400-7000-8000-000000000000"
		);
		assert_eq!(string(field(v7(), "short", None).await), "018f6a19");
	}

	#[tokio::test]
	async fn version_is_a_number() {
		assert_eq!(field(v7(), "version", None).await, Some(LazyValue::U64(7)));
		assert_eq!(field(v4(), "version", None).await, Some(LazyValue::U64(4)));
	}

	#[tokio::test]
	async fn v7_has_a_timestamp_and_v4_does_not() {
		assert_eq!(
			field(v7(), "timestamp", None).await,
			Some(LazyValue::Timestamp(1715472000))
		);
		assert_eq!(field(v4(), "timestamp", None).await, Some(LazyValue::Null));
	}

	#[tokio::test]
	async fn string_methods_fall_through() {
		assert_eq!(
			string(field(v7(), "upper", None).await),
			"018F6A19-6400-7000-8000-000000000000"
		);
		assert_eq!(
			match field(v7(), "split", Some("-")).await {
				Some(LazyValue::Array(a)) => a.len(),
				other => panic!("expected an array, got {other:?}"),
			},
			5
		);
	}

	/// The point of the variant: equality ignores formatting, in both operand
	/// orders, including against a plain string (which is all source JSON ever
	/// yields — `From<Value>` never mints a `Uuid`).
	#[test]
	fn equality_ignores_formatting() {
		let canonical = "018f6a19-6400-7000-8000-000000000000";
		for other in [
			canonical,
			"018f6a19640070008000000000000000",
			"018F6A19-6400-7000-8000-000000000000",
			"{018f6a19-6400-7000-8000-000000000000}",
			"urn:uuid:018f6a19-6400-7000-8000-000000000000",
		] {
			let uuid = LazyValue::Uuid(v7());
			let string = LazyValue::String(Arc::new(other.into()));
			assert_eq!(uuid, string, "uuid == {other:?}");
			assert_eq!(
				LazyValue::String(Arc::new(other.into())),
				LazyValue::Uuid(v7()),
				"{other:?} == uuid"
			);
		}

		for other in ["018f6a19-6400-7000-8000-000000000001", "not a uuid", ""] {
			assert_ne!(
				LazyValue::Uuid(v7()),
				LazyValue::String(Arc::new(other.into())),
				"uuid != {other:?}"
			);
		}
		assert_ne!(LazyValue::Uuid(v7()), LazyValue::Null);
	}

	/// However it was spelled, a uuid serializes as one canonical form, so a
	/// template substitution renders one spelling of an id.
	#[tokio::test]
	async fn serializes_canonically() {
		let value: ELazyValue =
			LazyValue::Uuid(Uuid::parse_str("{018F6A19-6400-7000-8000-000000000000}").unwrap())
				.into();

		assert_eq!(
			value.to_json(&ExtractState::default()).await.unwrap(),
			Some(serde_json::Value::String(
				"018f6a19-6400-7000-8000-000000000000".to_owned()
			))
		);
	}

	#[test]
	fn is_truthy_and_borrows_no_str() {
		assert!(LazyValue::Uuid(v7()).truthy());
		// No formatted form to borrow — callers coerce through `to_json`.
		assert_eq!(ELazyValue::from(LazyValue::Uuid(v7())).as_str(), None);
	}

	#[tokio::test]
	async fn a_uuid_method_rejects_arguments() {
		let err = UuidExtractor::new(v7())
			.field(
				&ExtractState {
					strict: true,
					..Default::default()
				},
				&Label::new("hex").unwrap(),
				Some("x"),
			)
			.await;
		assert!(err.is_err(), "expected an error, got {err:?}");
	}
}
