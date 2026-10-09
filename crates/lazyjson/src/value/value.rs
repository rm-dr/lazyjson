use mime::Mime;
use serde_json::{Map, Value, json};
use smartstring::{LazyCompact, SmartString};
use std::{
	any::Any,
	collections::HashMap,
	fmt::Debug,
	fs::File,
	io::Cursor,
	path::PathBuf,
	sync::{Arc, LazyLock, OnceLock},
};
use uuid::Uuid;

use crate::{
	Label,
	value::{
		ItemReader,
		extract::{
			ExtractError, ExtractState, JsonMode,
			blob::BinaryExtractor,
			core::{ListObjectExtractor, MapExtractor, VecExtractor},
			extra::{StringExtractor, UuidExtractor},
			traits::{ListExtractor, ObjectExtractor},
		},
	},
};

// Process-wide no-op extractors
static DEFAULT_OBJECT: LazyLock<Arc<dyn ObjectExtractor>> =
	LazyLock::new(|| Arc::new(MapExtractor::default()));
static DEFAULT_LIST: LazyLock<Arc<dyn ListExtractor>> =
	LazyLock::new(|| Arc::new(VecExtractor::default()));

#[derive(Clone)]
pub struct ArcBytes(pub Arc<Vec<u8>>);
impl Debug for ArcBytes {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("ArcBytes")
			.field("len()", &self.0.len())
			.finish()
	}
}

impl AsRef<[u8]> for ArcBytes {
	fn as_ref(&self) -> &[u8] {
		&self.0
	}
}

/// A source of binary data.
///
/// A [BinaryLazyValue] is _just_ a pile of bytes,
/// it never exposes metadata (file path, mod time, etc).
/// Use the special file types for that.
#[derive(Debug, Clone)]
pub enum BinaryLazyValue {
	/// A binary blob
	Blob { mime: Mime, bytes: ArcBytes },

	/// An pointer to a file
	File { mime: Mime, path: PathBuf },
}

impl BinaryLazyValue {
	/// Open the item for reading.
	pub async fn read(&self) -> Result<ItemReader, std::io::Error> {
		match self {
			Self::File { path, .. } => Ok(ItemReader::File(File::open(path)?)),
			Self::Blob { bytes, .. } => Ok(ItemReader::Vec(Cursor::new(bytes.clone()))),
		}
	}

	pub fn mime(&self) -> &Mime {
		match self {
			Self::Blob { mime, .. } => mime,
			Self::File { mime, .. } => mime,
		}
	}
}

/// An immutable, cheaply-cloneable, lazily-computed value.
/// Very similar to [serde_json::Value].
pub enum LazyValue {
	Null,

	U64(u64),
	I64(i64),
	F64(f64),
	Bool(bool),

	/// A unix timestamp, in seconds
	Timestamp(i64),

	/// A uuid.
	Uuid(Uuid),

	/// A string
	String(Arc<SmartString<LazyCompact>>),

	/// An array of values
	Array(Arc<Vec<ELazyValue>>),

	/// A lazily-computed map of {label: value}
	ObjectExtractor(Arc<dyn ObjectExtractor>),

	/// A lazily-computed array
	ListExtractor(Arc<dyn ListExtractor>),

	/// A file on the filesystem.
	/// Path is absolute and canonical.
	File(PathBuf),

	/// Binary data
	Binary(BinaryLazyValue),

	/// An item root.
	///
	/// Transparently delegates everything to its inner value,
	/// but exposes some extra functionality like `@.syn`.
	Root(Arc<ELazyValue>),
}

impl Eq for LazyValue {}
impl PartialEq for LazyValue {
	fn eq(&self, other: &Self) -> bool {
		match (self, other) {
			(LazyValue::Null, LazyValue::Null) => true,
			(LazyValue::Bool(l), LazyValue::Bool(r)) => l == r,
			(LazyValue::I64(l), LazyValue::I64(r)) => l == r,
			(LazyValue::Timestamp(l), LazyValue::Timestamp(r)) => l == r,
			(LazyValue::U64(l), LazyValue::U64(r)) => l == r,
			(LazyValue::U64(l), LazyValue::I64(r)) => {
				(*l).try_into().map(|l: i64| l == *r).unwrap_or(false)
			}
			(LazyValue::I64(l), LazyValue::U64(r)) => {
				(*l).try_into().map(|l: u64| l == *r).unwrap_or(false)
			}

			(LazyValue::String(l), LazyValue::String(r)) => l == r,

			(LazyValue::Uuid(l), LazyValue::Uuid(r)) => l == r,

			(LazyValue::Uuid(l), LazyValue::String(r))
			| (LazyValue::String(r), LazyValue::Uuid(l)) => {
				Uuid::parse_str(r).map(|r| *l == r).unwrap_or(false)
			}

			// root should never hold root, stack overflow should be impossible
			(LazyValue::Root(l), LazyValue::Root(r)) => l.inner == r.inner,

			(LazyValue::Array(l), LazyValue::Array(r)) => {
				l.len() == r.len() && l.iter().zip(r.iter()).all(|(a, b)| a.inner == b.inner)
			}

			_ => false,
		}
	}
}

impl Debug for LazyValue {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			LazyValue::Null => f.debug_struct("LazyValue::Null").finish(),
			LazyValue::Bool(x) => f
				.debug_struct("LazyValue::Bool")
				.field("value", &x)
				.finish(),
			LazyValue::U64(x) => f.debug_struct("LazyValue::U64").field("value", &x).finish(),
			LazyValue::I64(x) => f.debug_struct("LazyValue::I64").field("value", &x).finish(),
			LazyValue::F64(x) => f.debug_struct("LazyValue::F64").field("value", &x).finish(),
			LazyValue::Timestamp(x) => f
				.debug_struct("LazyValue::Timestamp")
				.field("unix_secs", &x)
				.finish(),
			LazyValue::Uuid(x) => f
				.debug_struct("LazyValue::Uuid")
				.field("value", &x)
				.finish(),
			LazyValue::String(x) => f
				.debug_struct("LazyValue::String")
				.field("value", &x)
				.finish(),
			LazyValue::Array(x) => f
				.debug_struct("LazyValue::Array")
				.field("value", &x)
				.finish(),
			LazyValue::ObjectExtractor(_) => f.debug_struct("LazyValue::ObjectExtractor").finish(),
			LazyValue::ListExtractor(_) => f.debug_struct("LazyValue::ListExtractor").finish(),
			LazyValue::File(p) => f.debug_struct("LazyValue::File").field("path", &p).finish(),
			LazyValue::Binary(b) => f
				.debug_struct("LazyValue::Binary")
				.field("len", &b.mime())
				.finish(),
			LazyValue::Root(inner) => f
				.debug_struct("LazyValue::Root")
				.field("inner", inner)
				.finish(),
		}
	}
}

impl Clone for LazyValue {
	fn clone(&self) -> Self {
		match self {
			LazyValue::Null => LazyValue::Null,
			LazyValue::Bool(x) => LazyValue::Bool(*x),
			LazyValue::U64(x) => LazyValue::U64(*x),
			LazyValue::I64(x) => LazyValue::I64(*x),
			LazyValue::F64(x) => LazyValue::F64(*x),
			LazyValue::Timestamp(x) => LazyValue::Timestamp(*x),
			LazyValue::Uuid(x) => LazyValue::Uuid(*x),
			LazyValue::String(x) => LazyValue::String(x.clone()),
			LazyValue::Array(x) => LazyValue::Array(x.clone()),
			LazyValue::ObjectExtractor(x) => LazyValue::ObjectExtractor(x.clone()),
			LazyValue::ListExtractor(x) => LazyValue::ListExtractor(x.clone()),
			LazyValue::File(f) => LazyValue::File(f.clone()),
			LazyValue::Binary(b) => LazyValue::Binary(b.clone()),
			LazyValue::Root(x) => LazyValue::Root(x.clone()),
		}
	}
}

impl From<Value> for LazyValue {
	fn from(value: Value) -> Self {
		match value {
			Value::String(s) => LazyValue::String(Arc::new(s.into())),
			Value::Null => LazyValue::Null,
			Value::Bool(b) => LazyValue::Bool(b),

			Value::Number(n) => {
				if let Some(u) = n.as_u64() {
					LazyValue::U64(u)
				} else if let Some(i) = n.as_i64() {
					LazyValue::I64(i)
				} else {
					// Floats have no dedicated variant; keep them as strings.
					LazyValue::String(Arc::new(n.to_string().into()))
				}
			}

			Value::Array(a) => LazyValue::Array(Arc::new(
				a.into_iter()
					.map(LazyValue::from)
					.map(|x| x.into())
					.collect(),
			)),

			Value::Object(map) => {
				let inner: HashMap<Label, ELazyValue> = map
					.into_iter()
					.filter_map(|(k, v)| {
						Label::new(&k).map(|label| (label, LazyValue::from(v).into()))
					})
					.collect();
				LazyValue::ObjectExtractor(Arc::new(MapExtractor {
					inner: inner.into(),
				}))
			}
		}
	}
}

impl From<Uuid> for LazyValue {
	fn from(value: Uuid) -> Self {
		LazyValue::Uuid(value)
	}
}

impl LazyValue {
	pub fn as_map(&self) -> Option<&MapExtractor> {
		match self {
			Self::ObjectExtractor(x) => {
				let a: &dyn Any = &**x;
				a.downcast_ref::<MapExtractor>()
			}
			_ => None,
		}
	}

	pub fn truthy(&self) -> bool {
		match self {
			LazyValue::Null => false,
			LazyValue::Bool(x) => *x,
			LazyValue::U64(_) => true,
			LazyValue::I64(_) => true,
			LazyValue::F64(_) => true,
			LazyValue::Timestamp(_) => true,
			LazyValue::Uuid(_) => true,
			LazyValue::String(_) => true,
			LazyValue::Array(_) => true,
			LazyValue::ObjectExtractor(_) => true,
			LazyValue::ListExtractor(_) => true,
			LazyValue::File(_) => true,
			LazyValue::Binary(_) => true,
			LazyValue::Root(x) => x.inner.truthy(),
		}
	}

	/// Coerce to an `f64` for numeric comparison (`<`, `<=`, `>`, `>=`).
	pub fn num(&self) -> Option<f64> {
		match self {
			LazyValue::U64(x) => Some(*x as f64),
			LazyValue::I64(x) => Some(*x as f64),
			LazyValue::Timestamp(x) => Some(*x as f64),
			LazyValue::String(s) => s.parse::<f64>().ok(),
			LazyValue::Root(x) => x.inner.num(),
			_ => None,
		}
	}
}

/// A [LazyValue] with cached extractors.
///
/// The `object` / `list` extractors are constructed on first access and
/// shared across all clones of this value (via `Arc<OnceLock<_>>`), so
/// walking many paths into the same value pays for each extractor exactly once.
#[derive(Clone)]
pub struct ELazyValue {
	pub inner: LazyValue,
	object_cell: Arc<OnceLock<Arc<dyn ObjectExtractor>>>,
	list_cell: Arc<OnceLock<Arc<dyn ListExtractor>>>,
}

impl Eq for ELazyValue {}
impl PartialEq for ELazyValue {
	fn eq(&self, other: &Self) -> bool {
		self.inner == other.inner
	}
}

impl Debug for ELazyValue {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("ELazyValue")
			.field("inner", &self.inner)
			.finish()
	}
}

impl From<LazyValue> for ELazyValue {
	fn from(value: LazyValue) -> Self {
		Self::new(value)
	}
}

impl ELazyValue {
	pub fn new(inner: LazyValue) -> Self {
		Self {
			inner,
			object_cell: Arc::new(OnceLock::new()),
			list_cell: Arc::new(OnceLock::new()),
		}
	}

	/// Get this value's object extractor, building it on first access.
	pub fn object(&self) -> &Arc<dyn ObjectExtractor> {
		self.object_cell.get_or_init(|| match &self.inner {
			LazyValue::Null
			| LazyValue::Bool(_)
			| LazyValue::U64(_)
			| LazyValue::I64(_)
			| LazyValue::F64(_)
			| LazyValue::Timestamp(_) => DEFAULT_OBJECT.clone(),
			LazyValue::Array(a) => Arc::new(ListObjectExtractor::new(Arc::new(VecExtractor::new(
				a.clone(),
			)))),
			LazyValue::ListExtractor(e) => Arc::new(ListObjectExtractor::new(e.clone())),
			LazyValue::String(s) => Arc::new(StringExtractor::new(s)),
			LazyValue::Uuid(u) => Arc::new(UuidExtractor::new(*u)),
			LazyValue::ObjectExtractor(e) => e.clone(),

			#[cfg(feature = "fs")]
			LazyValue::File(p) => Arc::new(super::extract::extra::FsExtractor::new(p)),
			#[cfg(not(feature = "fs"))]
			LazyValue::File(_) => DEFAULT_OBJECT.clone(),

			LazyValue::Binary(b) => Arc::new(BinaryExtractor::new(b)),
			LazyValue::Root(inner) => inner.object().clone(),
		})
	}

	/// Get this value's list extractor, building it on first access.
	pub fn list(&self) -> &Arc<dyn ListExtractor> {
		self.list_cell.get_or_init(|| match &self.inner {
			LazyValue::Null
			| LazyValue::Bool(_)
			| LazyValue::U64(_)
			| LazyValue::I64(_)
			| LazyValue::F64(_)
			| LazyValue::Timestamp(_)
			| LazyValue::Uuid(_)
			| LazyValue::String(_)
			| LazyValue::File(_)
			| LazyValue::Binary(_) => DEFAULT_LIST.clone(),
			LazyValue::Array(a) => Arc::new(VecExtractor::new(a.clone())),
			LazyValue::ListExtractor(e) => e.clone(),
			LazyValue::ObjectExtractor(e) => e.as_list().unwrap_or_else(|| DEFAULT_LIST.clone()),
			LazyValue::Root(inner) => inner.list().clone(), // a root is list-shaped iff its inner value is.
		})
	}
}

impl ELazyValue {
	/// Like `to_json`, but counts populated fields instead of collecting values.
	///
	/// - Leaf values (non-null scalars, arrays, blobs) contribute `Some(1)`.
	/// - `Null` contributes `None`.
	/// - `ObjectExtractor` is recursed into; returns `Some(Object(map))` with
	///   only the fields that had data, or `None` if all fields were absent.
	/// - `Array` / `ListExtractor` are treated as opaque leaf values (not descended into).
	pub async fn count_fields(&self, state: &ExtractState) -> Result<Option<Value>, ExtractError> {
		Ok(match &self.inner {
			LazyValue::Null => None,

			LazyValue::U64(_)
			| LazyValue::Bool(_)
			| LazyValue::I64(_)
			| LazyValue::F64(_)
			| LazyValue::Timestamp(_)
			| LazyValue::Uuid(_)
			| LazyValue::String(_)
			| LazyValue::Binary(BinaryLazyValue::Blob { .. }) => Some(Value::Number(1u64.into())),

			LazyValue::Array(x) => (!x.is_empty()).then(|| Value::Number(1u64.into())),
			LazyValue::ListExtractor(x) => {
				(x.len(state).await? > 0).then(|| Value::Number(1u64.into()))
			}

			LazyValue::ObjectExtractor(_)
			| LazyValue::File(_)
			| LazyValue::Root(_)
			| LazyValue::Binary(BinaryLazyValue::File { .. }) => {
				let object = self.object();
				let keys = object.fields(state).await?;
				let mut map = Map::new();
				for k in &keys {
					let v = match object.field(state, k, None).await? {
						Some(x) => x,
						None => continue,
					};
					if let Some(counted) = Box::pin(v.count_fields(state)).await? {
						map.insert(k.to_string(), counted);
					}
				}
				if map.is_empty() {
					None
				} else {
					Some(Value::Object(map))
				}
			}
		})
	}

	pub fn as_str(&self) -> Option<&str> {
		match &self.inner {
			LazyValue::String(x) => Some(x),
			LazyValue::Root(inner) => inner.as_str(),
			_ => None,
		}
	}

	pub async fn to_json(&self, state: &ExtractState) -> Result<Option<Value>, ExtractError> {
		Ok(match &self.inner {
			LazyValue::Null => Some(Value::Null),
			LazyValue::Bool(x) => Some(Value::Bool(*x)),
			LazyValue::U64(x) => Some(Value::Number((*x).into())),
			LazyValue::I64(x) => Some(Value::Number((*x).into())),
			LazyValue::F64(x) => Some(json!(x)),
			// A timestamp serializes as its Unix-seconds number.
			LazyValue::Timestamp(x) => Some(Value::Number((*x).into())),
			LazyValue::String(x) => Some(Value::String(x.to_string())),

			LazyValue::Uuid(x) => {
				let mut buf = [0u8; uuid::fmt::Hyphenated::LENGTH];
				Some(Value::String(
					x.hyphenated().encode_lower(&mut buf).to_owned(),
				))
			}

			// A blob has no JSON representation; how that is handled depends on
			// the configured [JsonMode].
			LazyValue::Binary(BinaryLazyValue::Blob { mime, bytes }) => match state.json_mode {
				JsonMode::None => None,
				JsonMode::Placeholder => Some(Value::String(format!(
					"<Blob ({mime}, {} bytes)>",
					bytes.0.len()
				))),
				JsonMode::Strict => {
					return Err(ExtractError::NotJsonable(format!(
						"blob ({mime}, {} bytes)",
						bytes.0.len()
					)));
				}
			},

			LazyValue::Array(_) | LazyValue::ListExtractor(_) => {
				return self.list().to_json(state).await;
			}

			LazyValue::ObjectExtractor(_)
			| LazyValue::File(_)
			| LazyValue::Root(_)
			| LazyValue::Binary(BinaryLazyValue::File { .. }) => {
				return self.object().to_json(state).await;
			}
		})
	}
}
