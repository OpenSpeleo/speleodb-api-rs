//! Async Rust SDK generated from the reviewed SpeleoDB OpenAPI snapshot.
//!
//! Consumers use the checked-in Rust sources without installing a generator.
//! See the crate README for configuration, authentication and maintenance.

#![doc = include_str!("../README.md")]

mod form;

// The generator owns formatting and idioms in this module.
#[rustfmt::skip]
#[allow(clippy::all, clippy::pedantic, unused_imports, dead_code, non_snake_case, rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags, rustdoc::bare_urls)]
pub mod generated;

pub use generated::client;
pub use generated::client::{ApiError, ApiOpError, HttpClient, HttpError};
pub use generated::types;
