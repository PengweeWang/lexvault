//! Core data management and read-only dictionary search for LexVault.

mod data_manager;
mod dictionary;
mod error;
mod normalize;
mod paths;

pub use data_manager::{
    DATABASE_FILE_NAME, DICTIONARY_RELEASE, DataManager, DataProgress, MINIMUM_DATABASE_ROWS,
};
pub use dictionary::{
    Entry, LexVault, LookupKind, LookupResult, SearchCursor, SearchPage, Suggestion,
};
pub use error::{DataError, DatabaseError};
pub use normalize::normalize_query;
pub use paths::DataPaths;
