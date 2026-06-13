#![allow(clippy::needless_question_mark)]

pub mod config;
pub mod error;
pub mod utils;

pub use config::volume_gain;
pub use error::Result;
