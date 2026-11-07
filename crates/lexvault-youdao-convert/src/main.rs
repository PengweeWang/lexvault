use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::Parser;
use rusqlite::{Connection, params};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(
    name = "lexvault-youdao-convert",
    about = "Convert extracted Youdao localdicts to a LexVault-readable SQLite database"
)]
struct Cli {
    /// Directory containing EC.ifo/CE.ifo, indexes and .ydic files.
    #[arg(long, value_name = "DIR")]
    input: PathBuf,

    /// Output SQLite database.
    #[arg(long, value_name = "FILE")]
    output: PathBuf,
}

#[derive(Debug, Error)]
enum ConvertError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("无效的 IFO 文件 {0}")]
    InvalidIfo(PathBuf),
    #[error("索引大小不正确：{0}")]
    InvalidIndex(PathBuf),
    #[error("索引记录数据越界：{0}")]
    InvalidRecord(String),
    #[error("输出数据库已存在：{0}")]
    OutputExists(PathBuf),
}

#[derive(Debug, Clone)]
struct Ifo {
    word_count: usize,
    lang: String,
    files: Vec<String>,
}

#[derive(Debug)]
struct ParsedBody {
    phonetic: Option<String>,
    definitions: Vec<(String, String)>,
    forms: Vec<String>,
    phrases: Vec<String>,
    raw: Value,
    supplemental: Vec<Value>,
}

#[derive(Debug, Clone)]
struct ExRef {
    dictionary: usize,
    offset: usize,
    length: usize,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<(), ConvertError> {
    if cli.output.exists() {
        return Err(ConvertError::OutputExists(cli.output));
    }
    let mut connection = Connection::open(&cli.output)?;
    create_schema(&connection)?;
    let transaction = connection.transaction()?;
    let ifo_paths = discover_ifos(&cli.input)?;
    if ifo_paths.is_empty() {
        return Err(ConvertError::InvalidIfo(cli.input));
    }

    let mut total_rows = 0_usize;
    let mut total_failures = 0_usize;
    for ifo_path in ifo_paths {
        let ifo = parse_ifo(&ifo_path)?;
        let index_path = ifo_path.with_extension("idx");
        let index = fs::read(&index_path)?;
        let expected_metadata = ifo
            .word_count
            .checked_mul(12)
            .ok_or_else(|| ConvertError::InvalidRecord("词条数导致索引长度溢出".to_owned()))?;
        if index.len() < expected_metadata {
            return Err(ConvertError::InvalidIndex(index_path));
        }
        let word_pool = xor_bytes(&index[expected_metadata..], 0x10);
        let records = parse_index_records(&index[..expected_metadata], ifo.word_count)?;
        let bodies = load_dictionary_files(&ifo_path, &ifo.files)?;

        for (record_index, record) in records.iter().enumerate() {
            let word_end = records
                .get(record_index + 1)
                .map(|next| next.word_offset)
                .unwrap_or(word_pool.len());
            if record.word_offset > word_end || word_end > word_pool.len() {
                return Err(ConvertError::InvalidRecord(format!(
                    "{} 第 {record_index} 个词头越界",
                    ifo_path.display()
                )));
            }
            let word = String::from_utf8_lossy(&word_pool[record.word_offset..word_end])
                .trim()
                .to_owned();
            if word.is_empty() {
                continue;
            }
            let body = slice_body(&bodies[0], record.data_offset, record.data_length)?;
            match parse_body_with_supplements(&ifo, &bodies, &body, 0, &mut HashSet::new()) {
                Ok(parsed) => {
                    insert_entry(&transaction, &ifo, &word, &parsed)?;
                    total_rows += 1;
                }
                Err(error) => {
                    total_failures += 1;
                    if total_failures <= 20 {
                        eprintln!("跳过 {word:?}: {error}");
                    }
                }
            }
        }
    }
    transaction.commit()?;
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS sd_1 ON stardict(word COLLATE NOCASE);
         CREATE INDEX IF NOT EXISTS stardict_3 ON stardict(sw, word COLLATE NOCASE);",
    )?;
    println!(
        "转换完成：{} 条词目，{} 条无法解析，输出 {}",
        total_rows,
        total_failures,
        cli.output.display()
    );
    Ok(())
}

fn create_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "CREATE TABLE stardict (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            word TEXT COLLATE NOCASE NOT NULL,
            sw TEXT COLLATE NOCASE NOT NULL,
            phonetic TEXT,
            definition TEXT,
            translation TEXT,
            pos TEXT,
            collins INTEGER DEFAULT 0,
            oxford INTEGER DEFAULT 0,
            tag TEXT,
            bnc INTEGER,
            frq INTEGER,
            exchange TEXT,
            detail TEXT,
            audio TEXT,
            source TEXT,
            lang TEXT
         );",
    )
}

fn discover_ifos(input: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut result = Vec::new();
    for entry in fs::read_dir(input)? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "ifo") {
            result.push(path);
        }
    }
    result.sort();
    Ok(result)
}

fn parse_ifo(path: &Path) -> Result<Ifo, ConvertError> {
    let bytes = fs::read(path)?;
    let (decoded, _, _) = encoding_rs::GBK.decode(&bytes);
    let text = decoded.as_ref();
    if !text.starts_with("YodaoDict's dict ifo file")
        && !text.starts_with("StarDict's dict ifo file")
    {
        return Err(ConvertError::InvalidIfo(path.to_path_buf()));
    }
    let mut word_count = None;
    let mut lang = None;
    let mut files = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("WordCount:") {
            word_count = value.trim().parse::<usize>().ok();
        } else if let Some(value) = line.strip_prefix("Lang:") {
            lang = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("DictFileName:") {
            files.push(value.trim().to_owned());
        }
    }
    Ok(Ifo {
        word_count: word_count
            .filter(|count| *count > 0)
            .ok_or_else(|| ConvertError::InvalidIfo(path.to_path_buf()))?,
        lang: lang.ok_or_else(|| ConvertError::InvalidIfo(path.to_path_buf()))?,
        files: files.into_iter().filter(|name| !name.is_empty()).collect(),
    })
}

#[derive(Debug, Clone, Copy)]
struct IndexRecord {
    word_offset: usize,
    data_offset: usize,
    data_length: usize,
}

fn parse_index_records(bytes: &[u8], count: usize) -> Result<Vec<IndexRecord>, ConvertError> {
    let mut records = Vec::with_capacity(count);
    for chunk in bytes.as_chunks::<12>().0.iter().take(count) {
        records.push(IndexRecord {
            word_offset: u32::from_le_bytes(chunk[0..4].try_into().unwrap()) as usize,
            data_offset: u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as usize,
            data_length: u32::from_le_bytes(chunk[8..12].try_into().unwrap()) as usize,
        });
    }
    if records.len() != count {
        return Err(ConvertError::InvalidRecord("索引记录数量不足".to_owned()));
    }
    Ok(records)
}

fn load_dictionary_files(ifo_path: &Path, names: &[String]) -> Result<Vec<Vec<u8>>, ConvertError> {
    let directory = ifo_path.parent().unwrap_or_else(|| Path::new("."));
    names
        .iter()
        .map(|name| {
            let bytes = fs::read(directory.join(name))?;
            Ok(xor_bytes(&bytes, 0x10))
        })
        .collect()
}

fn slice_body(body: &[u8], offset: usize, length: usize) -> Result<Vec<u8>, ConvertError> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| ConvertError::InvalidRecord("正文偏移溢出".to_owned()))?;
    body.get(offset..end)
        .map(ToOwned::to_owned)
        .ok_or_else(|| ConvertError::InvalidRecord(format!("正文越界 {offset}+{length}")))
}

fn parse_body_with_supplements(
    ifo: &Ifo,
    bodies: &[Vec<u8>],
    body: &[u8],
    depth: usize,
    visited: &mut HashSet<(usize, usize, usize)>,
) -> Result<ParsedBody, ConvertError> {
    let text = String::from_utf8_lossy(body).to_string();
    let references = parse_ex_references(&text);
    let raw = parse_tolerant_json(&text)?;
    let mut parsed = ParsedBody {
        phonetic: raw.get("p").and_then(Value::as_str).map(ToOwned::to_owned),
        definitions: extract_definitions(&raw),
        forms: extract_forms(&raw),
        phrases: extract_phrases(&raw),
        raw: raw.clone(),
        supplemental: Vec::new(),
    };

    if depth < 4 {
        for reference in references {
            if reference.dictionary == 0 || reference.dictionary >= bodies.len() {
                continue;
            }
            if !visited.insert((reference.dictionary, reference.offset, reference.length)) {
                continue;
            }
            let body = slice_body(
                &bodies[reference.dictionary],
                reference.offset,
                reference.length,
            )?;
            let Ok(supplemental) =
                parse_body_with_supplements(ifo, bodies, &body, depth + 1, visited)
            else {
                continue;
            };
            if parsed.phonetic.is_none() {
                parsed.phonetic = supplemental.phonetic.clone();
            }
            for definition in supplemental.definitions {
                if !parsed.definitions.contains(&definition) {
                    parsed.definitions.push(definition);
                }
            }
            for form in supplemental.forms {
                if !parsed.forms.contains(&form) {
                    parsed.forms.push(form);
                }
            }
            for phrase in supplemental.phrases {
                if !parsed.phrases.contains(&phrase) {
                    parsed.phrases.push(phrase);
                }
            }
            parsed.supplemental.push(supplemental.raw);
            parsed.supplemental.extend(supplemental.supplemental);
        }
    }

    let _ = ifo;
    Ok(parsed)
}

fn parse_tolerant_json(text: &str) -> Result<Value, ConvertError> {
    let mut owned = escape_control_characters(text.trim());
    if !owned.ends_with('}') {
        owned.push('}');
    }
    serde_json::from_str(&owned).map_err(ConvertError::Json)
}

fn escape_control_characters(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + 16);
    let mut in_string = false;
    let mut escaped = false;
    for ch in text.chars() {
        if in_string {
            if escaped {
                output.push(ch);
                escaped = false;
            } else if ch == '\\' {
                output.push(ch);
                escaped = true;
            } else if ch == '"' {
                output.push(ch);
                in_string = false;
            } else if ch < ' ' {
                match ch {
                    '\n' => output.push_str("\\n"),
                    '\r' => output.push_str("\\r"),
                    '\t' => output.push_str("\\t"),
                    _ => output.push_str(&format!("\\u{:04x}", ch as u32)),
                }
            } else {
                output.push(ch);
            }
        } else {
            output.push(ch);
            if ch == '"' {
                in_string = true;
            }
        }
    }
    output
}

fn parse_ex_references(text: &str) -> Vec<ExRef> {
    let mut refs = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("\"ex\":\"") {
        rest = &rest[start + 6..];
        let Some(end) = rest.find('"') else {
            break;
        };
        let value = &rest[..end];
        let parts = value
            .split('#')
            .map(str::parse::<usize>)
            .collect::<Result<Vec<_>, _>>();
        if let Ok(parts) = parts
            && parts.len() == 3
        {
            refs.push(ExRef {
                dictionary: parts[0],
                offset: parts[1],
                length: parts[2],
            });
        }
        rest = &rest[end + 1..];
    }
    refs
}

fn extract_definitions(value: &Value) -> Vec<(String, String)> {
    let mut definitions = Vec::new();
    if let Some(items) = value.get("e").and_then(Value::as_array) {
        for item in items.iter().filter_map(Value::as_str) {
            let (pos, text) = item
                .split_once("##")
                .map_or(("", item), |(pos, text)| (pos.trim(), text.trim()));
            if !text.is_empty() {
                definitions.push((pos.to_owned(), text.to_owned()));
            }
        }
    }
    definitions
}

fn extract_forms(value: &Value) -> Vec<String> {
    value
        .get("c")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|item| {
            item.trim()
                .trim_end_matches("$$")
                .replace("##", "：")
                .replace("$$", "、")
        })
        .filter(|item| !item.is_empty())
        .collect()
}

fn extract_phrases(value: &Value) -> Vec<String> {
    value
        .get("pr")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|item| item.trim().replace("$$", "："))
        .filter(|item| !item.is_empty())
        .collect()
}

fn insert_entry(
    transaction: &rusqlite::Transaction<'_>,
    ifo: &Ifo,
    word: &str,
    parsed: &ParsedBody,
) -> Result<(), ConvertError> {
    let positions = parsed
        .definitions
        .iter()
        .map(|(pos, _)| pos.as_str())
        .filter(|pos| !pos.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join("/");
    let definitions = parsed
        .definitions
        .iter()
        .map(|(pos, text)| {
            if pos.is_empty() {
                text.clone()
            } else if pos.ends_with('.') {
                format!("{pos} {text}")
            } else {
                format!("{pos}. {text}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let (definition, translation): (String, String) = if ifo.lang == "Cn2En" {
        (definitions, String::new())
    } else {
        (String::new(), definitions)
    };
    let detail = serde_json::json!({
        "source": ifo.files.first(),
        "raw": parsed.raw,
        "supplemental": parsed.supplemental,
        "phrases": parsed.phrases,
    });
    transaction.execute(
        "INSERT INTO stardict
         (word, sw, phonetic, definition, translation, pos, collins, oxford,
          tag, bnc, frq, exchange, detail, audio, source, lang)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, NULL, NULL, NULL, ?7, ?8, NULL, ?9, ?10)",
        params![
            word,
            normalize(word),
            parsed.phonetic,
            definition,
            translation,
            positions,
            parsed.forms.join("；"),
            detail.to_string(),
            ifo.files.first(),
            ifo.lang,
        ],
    )?;
    Ok(())
}

fn normalize(word: &str) -> String {
    word.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn xor_bytes(bytes: &[u8], key: u8) -> Vec<u8> {
    bytes.iter().map(|byte| byte ^ key).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        escape_control_characters, extract_forms, parse_ex_references, parse_index_records,
        xor_bytes,
    };

    #[test]
    fn ydao_xor_round_trips() {
        let bytes = b"{\"w\":\"test\"}";
        assert_eq!(xor_bytes(&xor_bytes(bytes, 0x10), 0x10), bytes);
    }

    #[test]
    fn escapes_control_characters_only_inside_strings() {
        assert_eq!(
            escape_control_characters("{\"e\":\"line\tone\"}\n"),
            "{\"e\":\"line\\tone\"}\n"
        );
    }

    #[test]
    fn reads_all_duplicate_ex_references() {
        let refs = parse_ex_references(r#"{"ex":"1#10#20","ex":"2#30#40"}"#);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].dictionary, 1);
        assert_eq!(refs[0].offset, 10);
        assert_eq!(refs[1].dictionary, 2);
        assert_eq!(refs[1].length, 40);
    }

    #[test]
    fn reads_index_triplets() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&6_u32.to_le_bytes());
        bytes.extend_from_slice(&100_u32.to_le_bytes());
        bytes.extend_from_slice(&20_u32.to_le_bytes());
        let records = parse_index_records(&bytes, 1).unwrap();
        assert_eq!(records[0].word_offset, 6);
        assert_eq!(records[0].data_offset, 100);
        assert_eq!(records[0].data_length, 20);
    }

    #[test]
    fn formats_word_forms_for_display() {
        let value = json!({
            "c": [
                "过去式##catabolized$$",
                "过去分词##catabolized$$",
                "现在分词##catabolizing$$",
                "vt., vi.##catabolized$$catabolizing$$"
            ]
        });
        assert_eq!(
            extract_forms(&value),
            [
                "过去式：catabolized",
                "过去分词：catabolized",
                "现在分词：catabolizing",
                "vt., vi.：catabolized、catabolizing"
            ]
        );
    }
}
