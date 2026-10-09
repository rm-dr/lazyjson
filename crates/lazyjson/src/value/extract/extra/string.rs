use regex::Regex;
use smartstring::{LazyCompact, SmartString};
use std::sync::Arc;

use crate::value::extract::ExtractState;
use crate::value::extract::extra::RegexExtractor;
use crate::{
	Label,
	value::{
		ELazyValue, LazyValue,
		extract::{ExtractError, traits::ObjectExtractor},
	},
};

pub struct StringExtractor {
	item: Arc<SmartString<LazyCompact>>,
}

impl StringExtractor {
	pub fn new(item: &Arc<SmartString<LazyCompact>>) -> Self {
		Self { item: item.clone() }
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for StringExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, ExtractError> {
		Ok(match (name.as_str(), args) {
			("trim", None) => {
				Some(LazyValue::String(Arc::new(self.item.as_str().trim().into())).into())
			}

			("lower", None) => {
				Some(LazyValue::String(Arc::new(self.item.as_str().to_lowercase().into())).into())
			}

			("upper", None) => {
				Some(LazyValue::String(Arc::new(self.item.as_str().to_uppercase().into())).into())
			}

			("nonempty", None) => Some(
				match self.item.is_empty() {
					true => LazyValue::Null,
					false => LazyValue::String(self.item.clone()),
				}
				.into(),
			),

			("trimprefix", Some(prefix)) => Some(
				LazyValue::String(Arc::new(
					self.item
						.as_str()
						.strip_prefix(prefix)
						.unwrap_or(self.item.as_str())
						.into(),
				))
				.into(),
			),

			("trimsuffix", Some(suffix)) => Some(
				LazyValue::String(Arc::new(
					self.item
						.as_str()
						.strip_suffix(suffix)
						.unwrap_or(self.item.as_str())
						.into(),
				))
				.into(),
			),

			("split", Some(by)) => Some(
				LazyValue::Array(Arc::new(
					self.item
						.as_str()
						.split(by)
						.map(|s| LazyValue::String(Arc::new(s.into())).into())
						.collect(),
				))
				.into(),
			),

			("regex", Some(pattern)) => {
				let Ok(re) = Regex::new(pattern) else {
					return Ok(None);
				};
				Some(
					match RegexExtractor::new(Arc::new(re), self.item.as_str()) {
						Some(ext) => LazyValue::ObjectExtractor(Arc::new(ext)).into(),
						None => LazyValue::Null.into(),
					},
				)
			}

			_ => return state.unknown_field(name),
		})
	}

	#[expect(clippy::unwrap_used)]
	async fn fields(&self, _state: &ExtractState) -> Result<Vec<Label>, ExtractError> {
		return Ok(vec![
			Label::new("trim").unwrap(),
			Label::new("upper").unwrap(),
			Label::new("lower").unwrap(),
			Label::new("nonempty").unwrap(),
			Label::new("trimprefix").unwrap(),
			Label::new("trimsuffix").unwrap(),
			Label::new("split").unwrap(),
		]);
	}
}

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
	use super::*;

	fn extractor(s: &str) -> StringExtractor {
		StringExtractor::new(&Arc::new(s.into()))
	}

	#[expect(clippy::unwrap_used)]
	async fn field(ext: &StringExtractor, name: &str, args: Option<&str>) -> Option<LazyValue> {
		let state = ExtractState {
			ignore_mime: false,
			..Default::default()
		};
		ext.field(&state, &Label::new(name).unwrap(), args)
			.await
			.unwrap()
			.map(|x| x.inner)
	}

	fn string(v: Option<LazyValue>) -> Option<String> {
		match v? {
			LazyValue::String(s) => Some(s.as_str().to_owned()),
			_ => panic!("expected string"),
		}
	}

	fn array(v: Option<LazyValue>) -> Vec<String> {
		match v.expect("expected Some") {
			LazyValue::Array(arr) => arr
				.iter()
				.map(|v| match &v.inner {
					LazyValue::String(s) => s.as_str().to_owned(),
					_ => panic!("expected string element"),
				})
				.collect(),
			_ => panic!("expected array"),
		}
	}

	#[tokio::test]
	async fn trim() {
		assert_eq!(
			string(field(&extractor("  hi  "), "trim", None).await),
			Some("hi".into())
		);
	}

	#[tokio::test]
	async fn trim_no_args() {
		assert!(field(&extractor("x"), "trim", Some("foo")).await.is_none());
	}

	#[tokio::test]
	async fn nonempty_with_content() {
		assert!(matches!(
			field(&extractor("hello"), "nonempty", None).await,
			Some(LazyValue::String(_))
		));
	}

	#[tokio::test]
	async fn nonempty_empty_string() {
		assert!(matches!(
			field(&extractor(""), "nonempty", None).await,
			Some(LazyValue::Null)
		));
	}

	#[tokio::test]
	async fn trimprefix_present() {
		assert_eq!(
			string(field(&extractor("foobar"), "trimprefix", Some("foo")).await),
			Some("bar".into())
		);
	}

	#[tokio::test]
	async fn trimprefix_absent() {
		assert_eq!(
			string(field(&extractor("foobar"), "trimprefix", Some("baz")).await),
			Some("foobar".into())
		);
	}

	#[tokio::test]
	async fn trimprefix_no_args() {
		assert!(
			field(&extractor("foobar"), "trimprefix", None)
				.await
				.is_none()
		);
	}

	#[tokio::test]
	async fn trimsuffix_present() {
		assert_eq!(
			string(field(&extractor("foobar"), "trimsuffix", Some("bar")).await),
			Some("foo".into())
		);
	}

	#[tokio::test]
	async fn trimsuffix_absent() {
		assert_eq!(
			string(field(&extractor("foobar"), "trimsuffix", Some("baz")).await),
			Some("foobar".into())
		);
	}

	#[tokio::test]
	async fn split_basic() {
		assert_eq!(
			array(field(&extractor("a,b,c"), "split", Some(",")).await),
			vec!["a", "b", "c"]
		);
	}

	#[tokio::test]
	async fn split_no_match() {
		assert_eq!(
			array(field(&extractor("abc"), "split", Some(",")).await),
			vec!["abc"]
		);
	}

	#[tokio::test]
	async fn split_no_args() {
		assert!(field(&extractor("abc"), "split", None).await.is_none());
	}

	#[tokio::test]
	async fn unknown_field() {
		assert!(field(&extractor("abc"), "bogus", None).await.is_none());
	}
}
