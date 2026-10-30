//! The crate's free-function entry points.

use std::path::Path;

use crate::error::Result;
use crate::store::GraphStore;
use crate::types::IndexReport;

use super::options::IndexOptions;

/// Extract the symbol definitions `pa-ast` yields for one source string.
///
/// Thin re-export so integration tests and the daemon can preview extraction
/// without opening a store.
#[must_use]
pub fn extract_symbols(
    source: &str,
    lang: pa_ast::SupportLang,
    module: &str,
) -> Vec<crate::extract::SymbolDef> {
    crate::extract::extract(source, lang, module).symbols
}

/// Convenience: index a project with default options.
///
/// # Errors
/// Propagates [`GraphStore::index_project`] errors.
pub fn index_project(
    store: &mut GraphStore,
    root: impl AsRef<Path>,
    project: &str,
) -> Result<IndexReport> {
    store.index_project(root, &IndexOptions::new(project))
}
