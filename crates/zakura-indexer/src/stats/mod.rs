//! Canonical-state analytics response adapters.

mod chart;
mod date;
mod query;

pub use chart::chart_data_from_state;
pub use query::stats_from_state;
