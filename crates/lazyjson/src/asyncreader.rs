use std::io::{Read, Seek, SeekFrom};
use tokio::runtime::Handle;

//
// MARK: asyncreader
//

/// An `async` equivalent of [std::io::Read].
pub trait AsyncReader: Send {
	/// Read a chunk of bytes.
	fn read(
		&mut self,
		buf: &mut [u8],
	) -> impl Future<Output = Result<usize, std::io::Error>> + Send;

	/// Read all remaining bytes into a `Vec`.
	fn read_to_end(&mut self) -> impl Future<Output = Result<Vec<u8>, std::io::Error>> + Send {
		async {
			let mut buf = Vec::new();
			let mut chunk = vec![0u8; 65536];
			loop {
				let n = self.read(&mut chunk).await?;
				if n == 0 {
					break;
				}
				buf.extend_from_slice(&chunk[..n]);
			}
			Ok(buf)
		}
	}
}

/// An `async` equivalent of [std::io::Read] + [std::io::Seek].
pub trait AsyncSeekReader: AsyncReader {
	fn seek(&mut self, pos: SeekFrom) -> impl Future<Output = Result<u64, std::io::Error>> + Send;
}

//
// MARK: sync bridge
//

/// Turn an async [Reader] into a sync [Read] + [Seek].
///
/// Never use outside of [tokio::task::spawn_blocking],
/// this struct will block the runtime.
pub struct SyncReadBridge<R: AsyncReader> {
	inner: R,
	handle: Handle,
}

impl<R: AsyncReader> SyncReadBridge<R> {
	/// Creates a new adapter using a handle to the current runtime.
	/// Panics if called outside of a tokio context.
	pub fn new_current(inner: R) -> Self {
		Self::new(inner, Handle::current())
	}

	/// Creates a new adapter using a handle to an existing runtime.
	pub fn new(inner: R, handle: Handle) -> Self {
		Self { inner, handle }
	}
}

impl<R: AsyncReader> Read for SyncReadBridge<R> {
	fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
		self.handle.block_on(self.inner.read(buf))
	}
}

impl<R: AsyncReader + AsyncSeekReader> Seek for SyncReadBridge<R> {
	fn seek(&mut self, pos: SeekFrom) -> Result<u64, std::io::Error> {
		self.handle.block_on(self.inner.seek(pos))
	}
}
