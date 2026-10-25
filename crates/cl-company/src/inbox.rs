//! The receipt inbox folder (P17, plan §3 WP5, Q-P17p).
//!
//! # Handoff (WP0 skeleton, WP5 fills this file)
//!
//! WP5 adds `scan(dir) -> Vec<InboxFile>` (non-recursive, no symlink
//! following, `.pdf/.png/.jpg/.jpeg`, a file is ready after two equal polls
//! ≥ 2 s apart, over 10 MB listed as too large, Windows online-only files
//! never read, at most 500 listed) and `file_away(path) -> PathBuf` (move to
//! `<inbox>/Filed/<YYYY-MM>/<name>`, never overwriting, never deleting).
//! No file names in logs.
