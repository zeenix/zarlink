#![cfg_attr(not(feature = "std"), no_std)]
#![deny(
    missing_debug_implementations,
    nonstandard_style,
    rust_2018_idioms,
    missing_docs
)]
#![warn(unreachable_pub, clippy::std_instead_of_core)]
#![doc = include_str!("../../README.md")]

#[cfg(all(not(feature = "std"), not(feature = "embedded")))]
compile_error!("Either 'std' or 'embedded' feature must be enabled.");

pub mod connection;
pub use connection::Connection;
mod error;
pub use error::{Error, Result};
