//! The batch model's plain values (`cl_model::forms::batch`) mapped 1:1
//! onto the engine types (`cl_api` batch), and the engine's answers back:
//! `cl_mcp::forms::batch` (A11 D0, shared with the MCP tools), re-exported
//! here.

pub(crate) use cl_mcp::forms::batch::{
    batch_kind, cells, column_mapping, groups, refused, save_caps, verdicts,
};
