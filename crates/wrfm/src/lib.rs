#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod error;
mod model;
mod parse;
#[cfg(test)]
mod tests;

#[cfg(feature = "std")]
pub use error::LoadError;
pub use error::{CountKind, EdgeError, ParseError, VertexError};
#[cfg(feature = "std")]
pub use model::write_canonical;
pub use model::{Group, WrfmModel};
