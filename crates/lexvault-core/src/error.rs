use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DataError {
    #[error("无法创建数据目录 {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("无法访问词典文件 {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "未找到有道词典数据库。\n\n\
         词典数据不随安装包分发。请从你合法持有的有道词典安装包中提取 localdicts，然后运行：\n\n\
         lexvault-youdao-convert --input <localdicts目录> --output \"{path}\"\n\n\
         也可以把已有的转换数据库放到：\n{path}\n\n\
         词典版本：6.3.69.8341\n文件名：youdao.db\n\n\
         完成后重新运行 LexVault。"
    )]
    MissingDatabase { path: PathBuf },

    #[error("词典数据库无效：{path}\n{reason}")]
    InvalidDatabase { path: PathBuf, reason: String },
}

impl DataError {
    pub fn database_path(&self) -> Option<&Path> {
        match self {
            Self::MissingDatabase { path } | Self::InvalidDatabase { path, .. } => Some(path),
            Self::CreateDirectory { path, .. } | Self::Io { path, .. } => Some(path),
        }
    }
}

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("无法打开词典数据库 {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    #[error("词典数据库查询失败: {0}")]
    Query(#[from] rusqlite::Error),

    #[error("词典数据库缺少兼容的 stardict 表")]
    MissingSchema,
}
