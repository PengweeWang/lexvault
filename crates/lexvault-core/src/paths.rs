use std::{
    env,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;

use crate::{DataError, data_manager::DICTIONARY_RELEASE};

#[derive(Clone, Debug)]
pub struct DataPaths {
    pub root: PathBuf,
    pub dictionary_dir: PathBuf,
    pub database: PathBuf,
}

impl DataPaths {
    pub fn from_env() -> Self {
        if let Some(root) = env::var_os("LEXVAULT_DATA_DIR") {
            return Self::from_root(PathBuf::from(root));
        }

        let root = ProjectDirs::from("", "", "LexVault")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".lexvault-data"));
        Self::from_root(root)
    }

    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let dictionary_dir = root
            .join("dictionaries")
            .join(format!("youdao-{DICTIONARY_RELEASE}"));
        Self {
            database: dictionary_dir.join("youdao.db"),
            root,
            dictionary_dir,
        }
    }

    pub fn create_directories(&self) -> Result<(), DataError> {
        std::fs::create_dir_all(&self.dictionary_dir).map_err(|source| DataError::CreateDirectory {
            path: self.dictionary_dir.clone(),
            source,
        })
    }

    pub fn data_root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::DataPaths;

    #[test]
    fn paths_put_youdao_database_in_versioned_directory() {
        let paths = DataPaths::from_root("/tmp/lexvault-test");
        assert_eq!(paths.database, paths.dictionary_dir.join("youdao.db"));
        assert!(paths.dictionary_dir.ends_with("youdao-6.3.69.8341"));
    }
}
