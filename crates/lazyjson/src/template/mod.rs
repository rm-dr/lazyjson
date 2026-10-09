mod expand;
mod parse;

#[expect(clippy::module_inception)]
mod template;
pub use template::*;

mod error;
pub use error::*;
