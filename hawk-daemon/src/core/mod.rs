//! 与 HTTP 无关的领域核心。依赖单向：api/ → core/。

pub mod color;
pub mod color_math;
pub mod config;
pub mod content_hash;
pub mod events;
pub mod fs_util;
pub mod global_filter;
pub mod index;
pub mod index_db;
pub mod item;
pub mod metadata;
pub mod metadata_store;
pub mod paths;
pub mod pipeline;
pub mod scanner;
pub mod startup;
pub mod taxonomy;
pub mod thumbnail;
pub mod thumbnail_worker;
pub mod view_prefs;
pub mod watcher;
