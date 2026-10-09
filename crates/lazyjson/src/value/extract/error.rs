/// An error produced while extracting fields from an item.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
	#[error(transparent)]
	IoError(#[from] std::io::Error),

	#[error("{message}")]
	Other {
		message: String,
		#[source]
		source: Box<dyn std::error::Error + Send + Sync + 'static>,
	},

	#[error("bad field access: {0}")]
	BadField(String),

	/// A method call received invalid arguments
	#[error("invalid arguments: {0}")]
	InvalidArgs(String),

	/// A value that was expected to be representable as JSON was not
	/// (e.g. a binary blob when placeholders are disabled).
	#[error("value is not representable as json: {0}")]
	NotJsonable(String),
}

impl ExtractError {
	pub fn other(
		message: impl Into<String>,
		source: impl std::error::Error + Send + Sync + 'static,
	) -> Self {
		Self::Other {
			message: message.into(),
			source: Box::new(source),
		}
	}
}

impl From<tokio::task::JoinError> for ExtractError {
	fn from(err: tokio::task::JoinError) -> Self {
		Self::other("background task failed", err)
	}
}

impl From<ExtractError> for std::io::Error {
	fn from(err: ExtractError) -> Self {
		match err {
			ExtractError::IoError(io) => io,
			other => std::io::Error::other(other),
		}
	}
}
