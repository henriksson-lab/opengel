//! Built-in ladder database.
//!
//! A seed set of common commercial DNA/RNA/protein ladders is compiled into the
//! binary from `ladders/ladders.json`. Sizes are authoritative; per-band masses
//! are left unset — users enter the loaded concentration for absolute
//! quantification and may override any template.

use crate::model::{GelType, LadderTemplate};
use std::sync::OnceLock;

const BUILTIN_JSON: &str = include_str!("../ladders/ladders.json");

fn builtins() -> &'static [LadderTemplate] {
    static CACHE: OnceLock<Vec<LadderTemplate>> = OnceLock::new();
    CACHE.get_or_init(|| {
        serde_json::from_str(BUILTIN_JSON).expect("bundled ladders.json is valid")
    })
}

/// All built-in ladder templates.
pub fn all() -> &'static [LadderTemplate] {
    builtins()
}

/// Built-in ladders applicable to a given gel type.
pub fn for_gel_type(gel_type: GelType) -> Vec<&'static LadderTemplate> {
    builtins()
        .iter()
        .filter(|t| t.gel_type == gel_type)
        .collect()
}

/// Look up a built-in ladder by exact name.
pub fn by_name(name: &str) -> Option<&'static LadderTemplate> {
    builtins().iter().find(|t| t.name == name)
}
