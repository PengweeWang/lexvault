use std::{fs, path::PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::{DataError, DataPaths};

pub const DICTIONARY_RELEASE: &str = "6.3.69.8341";
pub const DATABASE_FILE_NAME: &str = "youdao.db";
pub const MINIMUM_DATABASE_ROWS: u64 = 500_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataProgress {
    Checking,
    VerifyingDatabase,
    Ready(PathBuf),
}

#[derive(Debug)]
pub struct DataManager {
    paths: DataPaths,
}

impl DataManager {
    pub fn new(paths: DataPaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &DataPaths {
        &self.paths
    }

    pub fn ensure_ready<F>(&self, mut progress: F) -> Result<PathBuf, DataError>
    where
        F: FnMut(DataProgress),
    {
        progress(DataProgress::Checking);
        self.paths.create_directories()?;

        if !self.paths.database.exists() {
            return Err(DataError::MissingDatabase {
                path: self.paths.database.clone(),
            });
        }

        progress(DataProgress::VerifyingDatabase);
        self.validate_database()?;
        progress(DataProgress::Ready(self.paths.database.clone()));
        Ok(self.paths.database.clone())
    }

    fn validate_database(&self) -> Result<(), DataError> {
        let metadata = fs::metadata(&self.paths.database).map_err(|source| DataError::Io {
            path: self.paths.database.clone(),
            source,
        })?;
        if metadata.len() < 64 * 1024 * 1024 {
            return Err(DataError::InvalidDatabase {
                path: self.paths.database.clone(),
                reason: format!("文件仅 {} 字节，不像是完整转换库", metadata.len()),
            });
        }

        let connection =
            Connection::open_with_flags(&self.paths.database, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| DataError::InvalidDatabase {
                    path: self.paths.database.clone(),
                    reason: error.to_string(),
                })?;
        let count: i64 = connection
            .query_row("SELECT count(*) FROM stardict", [], |row| row.get(0))
            .map_err(|error| DataError::InvalidDatabase {
                path: self.paths.database.clone(),
                reason: error.to_string(),
            })?;
        if count < MINIMUM_DATABASE_ROWS as i64 {
            return Err(DataError::InvalidDatabase {
                path: self.paths.database.clone(),
                reason: format!("仅有 {count} 条词目，至少需要 {MINIMUM_DATABASE_ROWS} 条"),
            });
        }
        connection
            .prepare(
                "SELECT id, word, phonetic, definition, translation, pos, collins, oxford,
                        tag, bnc, frq, exchange, detail, audio
                 FROM stardict LIMIT 0",
            )
            .map_err(|error| DataError::InvalidDatabase {
                path: self.paths.database.clone(),
                reason: error.to_string(),
            })?;
        Ok(())
    }
}
