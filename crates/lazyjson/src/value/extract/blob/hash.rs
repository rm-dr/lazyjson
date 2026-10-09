use std::{io::Read, sync::Arc};
use tokio::sync::OnceCell;

use crate::Label;
use crate::SyncReadBridge;
use crate::value::extract::ExtractError;
use crate::value::extract::ExtractState;
use crate::value::extract::traits::ObjectExtractor;
use crate::value::{BinaryLazyValue, ELazyValue, LazyValue};

fn to_hex(bytes: &[u8]) -> String {
	bytes.iter().map(|b| format!("{b:02x}")).collect()
}

macro_rules! hash_algos {
	($($name:ident),* $(,)?) => {
		pub struct HashExtractor {
			item: BinaryLazyValue,
			$($name: OnceCell<String>,)*
		}

		impl HashExtractor {
			pub fn new(item: &BinaryLazyValue) -> Self {
				Self {
					item: item.clone(),
					$($name: OnceCell::new(),)*
				}
			}
		}

		static LABELS: std::sync::LazyLock<Vec<Label>> = std::sync::LazyLock::new(|| {
			vec![$(Label::new(stringify!($name)).unwrap()),*]
		});
	};
}

hash_algos!(blake3, md5, sha1, sha224, sha256, sha384, sha512);

impl HashExtractor {
	async fn compute(&self, name: &Label) -> Result<Option<String>, std::io::Error> {
		let name_str = name.as_ref();

		macro_rules! algo {
			($cell:ident, $compute:expr) => {
				if name_str == stringify!($cell) {
					return Ok(Some(
						self.$cell
							.get_or_try_init(|| async {
								let read = self.item.read().await?;
								let mut read = SyncReadBridge::new_current(read);
								tokio::task::spawn_blocking(move || {
									let mut bytes = Vec::new();
									read.read_to_end(&mut bytes)?;
									Ok::<String, std::io::Error>($compute(&bytes))
								})
								.await?
							})
							.await?
							.clone(),
					));
				}
			};
		}

		algo!(blake3, |b: &Vec<u8>| blake3::hash(b).to_hex().to_string());
		algo!(md5, |b: &Vec<u8>| format!("{:x}", md5::compute(b)));
		algo!(sha1, |b: &Vec<u8>| {
			use sha1::Digest;
			to_hex(sha1::Sha1::digest(b).as_ref())
		});
		algo!(sha224, |b: &Vec<u8>| {
			use sha2::Digest;
			to_hex(sha2::Sha224::digest(b).as_ref())
		});
		algo!(sha256, |b: &Vec<u8>| {
			use sha2::Digest;
			to_hex(sha2::Sha256::digest(b).as_ref())
		});
		algo!(sha384, |b: &Vec<u8>| {
			use sha2::Digest;
			to_hex(sha2::Sha384::digest(b).as_ref())
		});
		algo!(sha512, |b: &Vec<u8>| {
			use sha2::Digest;
			to_hex(sha2::Sha512::digest(b).as_ref())
		});

		Ok(None)
	}
}

#[async_trait::async_trait]
impl ObjectExtractor for HashExtractor {
	async fn field(
		&self,
		state: &ExtractState,
		name: &Label,
		args: Option<&str>,
	) -> Result<Option<ELazyValue>, ExtractError> {
		if args.is_some() {
			return state.unexpected_args(name);
		}

		let value = self
			.compute(name)
			.await?
			.map(|s| LazyValue::String(Arc::new(s.into())).into());
		state.resolve_field(name, value)
	}

	async fn fields(&self, _state: &ExtractState) -> Result<Vec<Label>, ExtractError> {
		Ok(LABELS.clone())
	}
}
