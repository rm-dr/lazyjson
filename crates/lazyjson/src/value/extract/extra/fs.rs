use std::{
	collections::HashMap,
	path::{Component, PathBuf},
	sync::Arc,
};
use tokio::sync::Mutex;
use tracing::warn;

use crate::value::extract::ExtractState;
use crate::{
	Label,
	value::{
		BinaryLazyValue, ELazyValue, LazyValue,
		extract::{EvalError, traits::ObjectExtractor},
	},
};

pub struct FsExtractor {
	/// Absolute, canonicalized path to file
	path: PathBuf,
	output: Mutex<Option<HashMap<Label, ELazyValue>>>,
}

impl FsExtractor {
	pub fn new(path: impl Into<PathBuf>) -> Self {
		Self {
			path: path.into(),
			output: Mutex::new(None),
		}
	}

	fn compute_inner(path: PathBuf) -> Result<HashMap<Label, ELazyValue>, std::io::Error> {
		let dir = std::env::current_dir()?;
		let relpath = path.strip_prefix(&dir).ok();

		let components = path
			.components()
			.filter_map(|x| match x {
				Component::CurDir => {
					warn!(message = "Path must be canonical, but contains .", ?path);
					debug_assert!(false, "Path must be canonical, but contains .: {path:?}");
					None
				}

				Component::ParentDir => {
					warn!(message = "Path must be canonical, but contains ..", ?path);
					debug_assert!(false, "Path must be canonical, but contains ..: {path:?}");
					None
				}

				Component::RootDir => None,
				Component::Normal(x) => Some(x.to_str().map(|x| x.to_owned())),
				Component::Prefix(x) => Some(x.as_os_str().to_str().map(|x| x.to_owned())),
			})
			.collect::<Option<Vec<_>>>();

		let rel_components = relpath.and_then(|x| {
			x.components()
				.filter_map(|x| match x {
					Component::CurDir => {
						warn!(message = "Path must be canonical, but contains .", ?path);
						debug_assert!(false, "Path must be canonical, but contains .: {path:?}");
						None
					}

					Component::ParentDir => {
						warn!(message = "Path must be canonical, but contains ..", ?path);
						debug_assert!(false, "Path must be canonical, but contains ..: {path:?}");
						None
					}

					Component::RootDir => None,
					Component::Normal(x) => Some(x.to_str().map(|x| x.to_owned())),
					Component::Prefix(x) => Some(x.as_os_str().to_str().map(|x| x.to_owned())),
				})
				.collect::<Option<Vec<_>>>()
		});

		let path_str = components
			.as_ref()
			.map(|x| x.join("/"))
			.map(|x| format!("/{x}"));
		let rel_path_str = rel_components.as_ref().map(|x| x.join("/"));

		#[expect(clippy::unwrap_used)]
		let output: HashMap<Label, ELazyValue> = HashMap::from([
			(
				Label::new("extension").unwrap(),
				ELazyValue::from(
					path.extension()
						.and_then(|x| x.to_str())
						.map(|x| LazyValue::String(Arc::new(x.into())))
						.unwrap_or(LazyValue::Null),
				),
			),
			(
				Label::new("path").unwrap(),
				rel_path_str
					.map(|x| LazyValue::String(Arc::new(x.into())))
					.unwrap_or(LazyValue::Null)
					.into(),
			),
			(
				Label::new("segments").unwrap(),
				rel_components
					.clone()
					.map(|x| {
						LazyValue::Array(Arc::new(
							x.iter()
								.map(|x| LazyValue::String(Arc::new(x.into())).into())
								.collect(),
						))
					})
					.unwrap_or(LazyValue::Null)
					.into(),
			),
			(
				Label::new("full_path").unwrap(),
				path_str
					.map(|x| LazyValue::String(Arc::new(x.into())))
					.unwrap_or(LazyValue::Null)
					.into(),
			),
			(
				Label::new("full_segments").unwrap(),
				components
					.clone()
					.map(|x| {
						LazyValue::Array(Arc::new(
							x.iter()
								.map(|x| LazyValue::String(Arc::new(x.into())).into())
								.collect(),
						))
					})
					.unwrap_or(LazyValue::Null)
					.into(),
			),
			(
				Label::new("name").unwrap(),
				components
					.and_then(|x| x.last().map(|x| LazyValue::String(Arc::new(x.into()))))
					.unwrap_or(LazyValue::Null)
					.into(),
			),
			(
				Label::new("content").unwrap(),
				LazyValue::Binary(BinaryLazyValue::File {
					mime: mime_guess::from_path(&path).first_or_octet_stream(),
					path: path.clone(),
				})
				.into(),
			),
		]);

		return Ok(output);
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for FsExtractor {
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
			let path = self.path.clone();
			*out = Some(tokio::task::spawn_blocking(|| Self::compute_inner(path)).await??);
		}

		#[expect(clippy::unwrap_used)]
		let value = out.as_ref().unwrap().get(name).cloned();
		state.resolve_field(name, value)
	}

	async fn fields(&self, _state: &ExtractState) -> Result<Vec<Label>, EvalError> {
		let mut out = self.output.lock().await;
		if out.is_none() {
			let path = self.path.clone();
			*out = Some(tokio::task::spawn_blocking(|| Self::compute_inner(path)).await??);
		}

		#[expect(clippy::unwrap_used)]
		return Ok(out.as_ref().unwrap().keys().cloned().collect());
	}
}
