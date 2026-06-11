//! Individual filter stages. Order in PLAN.md §Phase 3 is canonical;
//! `Pipeline::new` is responsible for instantiating them in that order.

pub mod depth;
pub mod exclude;
pub mod extension;
pub mod hidden_by_attr;
pub mod hidden_by_name;
pub mod ignore_cache;
pub mod ignore_contain;
pub mod max_results;
pub mod owner;
pub mod prune;
pub mod same_filesystem;
pub mod size;
pub mod symlink_filter;
pub mod time_filter;
pub mod type_filter;
