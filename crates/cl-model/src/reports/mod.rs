//! The report window's model (P28 plan §4): static report specs and the
//! catalog, the owner's saved options, the memorized-report definition
//! (U-118 `kind:'standard'`), date presets and the resolution of options
//! into a concrete [`cl_core::report::ReportRequest`].
//!
//! No egui, no I/O, no money arithmetic: the engine computes every amount.

pub mod ask;
pub mod catalog;
pub mod definition;
pub mod options;
pub mod present;
pub mod presets;
pub mod resolve;
pub mod spec;

pub use catalog::{
    BUILDER_AS_OF, BUILDER_AS_OF_TOTAL, BUILDER_RANGE, BUILDER_RANGE_TOTAL, CATALOG, NO_STARTER,
    REPORT_STARTERS, find, mapped_starters, search, starter_for,
};
pub use definition::{DefinitionError, SCHEMA_VERSION, StandardDefinition};
pub use options::{
    AmountFilter, ColumnBy, DateChoice, HeaderOptions, ReportOptions, Sort, TagLabel, TagSplit,
    tag_row_caption,
};
pub use present::{hidden, present, tag_caption};
pub use presets::DatePreset;
pub use resolve::{ResolveError, resolve};
pub use spec::{
    BasisMode, CatalogGroup, DateMode, DrillRule, FilterKind, Layout, ReportSpec, SortMode,
};
