use std::{
	fs::File,
	io::{Cursor, Seek},
};

use crate::{AsyncReader, AsyncSeekReader, value::ArcBytes};

pub enum ItemReader {
	File(File),
	Vec(Cursor<ArcBytes>),
}

impl AsyncReader for ItemReader {
	async fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
		match self {
			Self::File(x) => std::io::Read::read(x, buf),
			Self::Vec(x) => std::io::Read::read(x, buf),
		}
	}
}

impl AsyncSeekReader for ItemReader {
	async fn seek(&mut self, pos: std::io::SeekFrom) -> Result<u64, std::io::Error> {
		match self {
			Self::File(x) => x.seek(pos),
			Self::Vec(x) => x.seek(pos),
		}
	}
}
