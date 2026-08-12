//! A small application with real seams in it.
//!
//! Every task in the v1.5 alignment catalog refers to something in here. The
//! code is deliberately imperfect — duplicated setup, an off-by-one, two
//! modules that disagree about an id — because a fixture with nothing wrong
//! with it gives every benchmark task the same answer and measures nothing.

pub mod accounts;
pub mod config;
pub mod dates;
pub mod health;
pub mod http;
pub mod items;
pub mod pagination;
pub mod parser;
pub mod reporter;
pub mod response;
pub mod retry;
pub mod scheduler;
pub mod ui;
