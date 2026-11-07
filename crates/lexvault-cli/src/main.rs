mod tui;

use std::{path::PathBuf, process::ExitCode};

use clap::{CommandFactory, Parser};
use lexvault_core::{
    DataError, DataManager, DataPaths, DatabaseError, Entry, LexVault, LookupKind,
};
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(
    name = "lexvault",
    version,
    about = "Offline Youdao dictionary",
    long_about = "Look up an English word, or use --tui for the interactive terminal interface."
)]
struct Cli {
    /// Start the interactive terminal interface.
    #[arg(long, conflicts_with = "word")]
    tui: bool,

    /// Word to look up.
    #[arg(value_name = "WORD")]
    word: Option<String>,
}

#[derive(Debug, Error)]
enum AppError {
    #[error(transparent)]
    Data(#[from] DataError),

    #[error(transparent)]
    Database(#[from] DatabaseError),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if !cli.tui && cli.word.is_none() {
        let mut command = Cli::command();
        if command.print_help().is_err() {
            return ExitCode::from(2);
        }
        println!();
        return ExitCode::SUCCESS;
    }

    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            let code = match error {
                AppError::Data(_) => 3,
                AppError::Database(_) => 4,
                AppError::Io(_) => 1,
            };
            ExitCode::from(code)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, AppError> {
    let database_path = prepare_dictionary()?;
    let dictionary = LexVault::open(database_path)?;

    if cli.tui {
        tui::run(&dictionary)?;
        return Ok(ExitCode::SUCCESS);
    }

    let word = cli.word.expect("word is required outside TUI mode");
    let Some(result) = dictionary.lookup(&word)? else {
        eprintln!("没有找到任何匹配词条：{word}");
        return Ok(ExitCode::from(1));
    };

    if let LookupKind::Closest { distance } = result.kind {
        eprintln!("未找到精确匹配：{word}");
        eprintln!("最接近的词：{}（编辑距离 {distance}）", result.entry.word);
    }
    println!("{}", format_entry(&result.entry));
    Ok(ExitCode::SUCCESS)
}

fn prepare_dictionary() -> Result<PathBuf, DataError> {
    let manager = DataManager::new(DataPaths::from_env());
    manager.ensure_ready(|_| {})
}

fn format_entry(entry: &Entry) -> String {
    let mut output = String::new();
    output.push_str(&entry.word);
    output.push('\n');

    if let Some(phonetic) = entry
        .phonetic
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        output.push('/');
        output.push_str(phonetic);
        output.push_str("/\n");
    }

    let mut metadata = Vec::new();
    if let Some(tags) = entry.tags.as_deref().filter(|value| !value.is_empty()) {
        metadata.push(tags.to_owned());
    }
    if entry.collins > 0 {
        metadata.push(format!("Collins {}/5", entry.collins));
    }
    if entry.oxford > 0 {
        metadata.push("Oxford 3000".to_owned());
    }
    if let Some(frq) = entry.frq.filter(|value| *value > 0) {
        metadata.push(format!("FRQ {frq}"));
    }
    if !metadata.is_empty() {
        output.push_str(&metadata.join(" · "));
        output.push('\n');
    }

    if let Some(translation) = entry
        .translation
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        output.push('\n');
        output.push_str(translation);
        output.push('\n');
    }

    if let Some(definition) = entry
        .definition
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if entry
            .translation
            .as_deref()
            .is_some_and(|translation| !translation.trim().is_empty())
        {
            output.push('\n');
        }
        output.push_str(definition);
        output.push('\n');
    }

    if let Some(exchange) = entry
        .exchange
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        output.push('\n');
        output.push_str("词形：");
        output.push_str(exchange);
        output.push('\n');
    }

    output.trim_end().to_owned()
}
