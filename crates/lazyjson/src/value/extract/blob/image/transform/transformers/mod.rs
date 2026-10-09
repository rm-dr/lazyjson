//! Defines all transformation steps we can apply to an image

use image::DynamicImage;
use std::fmt::{Debug, Display};

mod crop;
pub use crop::*;

mod maxdim;
pub use maxdim::*;

/// A single transformation that may be applied to an image.
pub trait ImageTransformer
where
	Self: PartialEq,
	Self: Sized + Clone,
	Self: Display + Debug,
{
	/// Transform the given image in place
	fn transform(&self, input: &mut DynamicImage);

	/// Parse an arg string.
	///
	/// `name({arg_string})`
	fn parse_args(args: &str) -> Result<Self, String>;
}
