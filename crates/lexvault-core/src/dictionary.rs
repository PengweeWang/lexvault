use std::{
    cmp::Reverse,
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params, params_from_iter};
use serde::Serialize;

use crate::{DatabaseError, normalize_query};

const ENTRY_COLUMNS: &str = "
    id, word, phonetic, definition, translation, pos,
    coalesce(collins, 0), coalesce(oxford, 0),
    tag, bnc, frq, exchange, detail, audio
";

const CANDIDATE_COLUMNS: &str = "
    word, sw, frq, bnc, coalesce(collins, 0), coalesce(oxford, 0), tag
";

const SUGGESTION_COLUMNS: &str = "
    id, word, sw, frq, bnc, coalesce(collins, 0), coalesce(oxford, 0),
    tag, phonetic, translation
";

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub id: i64,
    pub word: String,
    pub phonetic: Option<String>,
    pub definition: Option<String>,
    pub translation: Option<String>,
    pub pos: Option<String>,
    pub collins: i64,
    pub oxford: i64,
    pub tags: Option<String>,
    pub bnc: Option<i64>,
    pub frq: Option<i64>,
    pub exchange: Option<String>,
    pub detail: Option<String>,
    pub audio: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupKind {
    RawExact,
    NormalizedExact,
    Closest { distance: usize },
}

#[derive(Clone, Debug)]
pub struct LookupResult {
    pub kind: LookupKind,
    pub entry: Entry,
}

#[derive(Clone, Debug)]
pub struct Suggestion {
    pub id: i64,
    pub word: String,
    pub phonetic: Option<String>,
    pub brief: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchPhase {
    RawExact,
    NormalizedExact,
    Prefix,
    Done,
}

#[derive(Clone, Debug)]
pub struct SearchCursor {
    phase: SearchPhase,
    raw_exact_id: Option<i64>,
    last_sw: Option<String>,
    last_word: Option<String>,
    last_id: Option<i64>,
}

impl SearchCursor {
    fn start() -> Self {
        Self {
            phase: SearchPhase::RawExact,
            raw_exact_id: None,
            last_sw: None,
            last_word: None,
            last_id: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchPage {
    pub items: Vec<Suggestion>,
    pub next_cursor: Option<SearchCursor>,
    pub exhausted: bool,
}

#[derive(Debug)]
pub struct LexVault {
    connection: Connection,
    path: PathBuf,
}

#[derive(Clone, Debug)]
struct Candidate {
    word: String,
    sw: String,
    frq: Option<i64>,
    bnc: Option<i64>,
    collins: i64,
    oxford: i64,
    tags: Option<String>,
}

#[derive(Clone, Debug)]
struct RankedCandidate {
    candidate: Candidate,
    distance: usize,
    length_difference: usize,
    common_prefix: usize,
    same_first: bool,
}

#[derive(Clone, Debug)]
struct SuggestionCandidate {
    id: i64,
    candidate: Candidate,
    phonetic: Option<String>,
    translation: Option<String>,
}

impl SuggestionCandidate {
    fn is_better_than(&self, other: &Self, query: &str, normalized: &str) -> bool {
        (
            self.candidate.word != query,
            !self.candidate.word.eq_ignore_ascii_case(query),
            self.candidate.sw != normalized,
            dictionary_quality(&self.candidate),
            self.candidate
                .sw
                .chars()
                .count()
                .abs_diff(normalized.chars().count()),
            known_frequency(self.candidate.frq),
            known_frequency(self.candidate.bnc),
            self.candidate.oxford <= 0,
            Reverse(self.candidate.collins),
            self.candidate.word.as_str(),
        ) < (
            other.candidate.word != query,
            !other.candidate.word.eq_ignore_ascii_case(query),
            other.candidate.sw != normalized,
            dictionary_quality(&other.candidate),
            other
                .candidate
                .sw
                .chars()
                .count()
                .abs_diff(normalized.chars().count()),
            known_frequency(other.candidate.frq),
            known_frequency(other.candidate.bnc),
            other.candidate.oxford <= 0,
            Reverse(other.candidate.collins),
            other.candidate.word.as_str(),
        )
    }

    fn into_suggestion(self) -> Suggestion {
        Suggestion {
            id: self.id,
            word: self.candidate.word,
            phonetic: self.phonetic,
            brief: first_line(self.translation.as_deref()),
        }
    }
}

impl RankedCandidate {
    fn new(candidate: Candidate, wanted: &str) -> Self {
        let distance = strsim::damerau_levenshtein(wanted, &candidate.sw);
        let length_difference = wanted
            .chars()
            .count()
            .abs_diff(candidate.sw.chars().count());
        let common_prefix = wanted
            .chars()
            .zip(candidate.sw.chars())
            .take_while(|(left, right)| left == right)
            .count();
        let same_first = wanted.chars().next() == candidate.sw.chars().next();
        Self {
            candidate,
            distance,
            length_difference,
            common_prefix,
            same_first,
        }
    }

    fn is_better_than(&self, other: &Self) -> bool {
        (
            self.distance,
            dictionary_quality(&self.candidate),
            Reverse(self.common_prefix),
            known_frequency(self.candidate.frq),
            self.candidate.oxford <= 0,
            Reverse(self.candidate.collins),
            self.length_difference,
            !self.same_first,
            known_frequency(self.candidate.bnc),
            self.candidate.word.as_str(),
        ) < (
            other.distance,
            dictionary_quality(&other.candidate),
            Reverse(other.common_prefix),
            known_frequency(other.candidate.frq),
            other.candidate.oxford <= 0,
            Reverse(other.candidate.collins),
            other.length_difference,
            !other.same_first,
            known_frequency(other.candidate.bnc),
            other.candidate.word.as_str(),
        )
    }
}

fn dictionary_quality(candidate: &Candidate) -> u8 {
    if candidate.collins > 0
        || candidate.oxford > 0
        || candidate.frq.unwrap_or(0) > 0
        || candidate
            .tags
            .as_deref()
            .is_some_and(|tags| !tags.is_empty())
    {
        0
    } else if candidate.bnc.unwrap_or(0) > 0 {
        1
    } else {
        2
    }
}

fn known_frequency(value: Option<i64>) -> (u8, i64) {
    match value.filter(|value| *value > 0) {
        Some(value) => (0, value),
        None => (1, i64::MAX),
    }
}

impl LexVault {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|source| DatabaseError::Open {
                path: path.clone(),
                source,
            })?;
        connection
            .pragma_update(None, "query_only", true)
            .map_err(DatabaseError::Query)?;
        connection
            .pragma_update(None, "cache_size", -2000_i64)
            .map_err(DatabaseError::Query)?;
        connection
            .pragma_update(None, "mmap_size", 0_i64)
            .map_err(DatabaseError::Query)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(DatabaseError::Query)?;

        let table: Option<String> = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'stardict'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(DatabaseError::Query)?;
        if table.is_none() {
            return Err(DatabaseError::MissingSchema);
        }

        connection
            .prepare(&format!("SELECT {ENTRY_COLUMNS} FROM stardict LIMIT 0"))
            .map_err(DatabaseError::Query)?;

        Ok(Self { connection, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn lookup(&self, query: &str) -> Result<Option<LookupResult>, DatabaseError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(None);
        }

        if let Some(entry) = self.lookup_raw_exact(query)? {
            return Ok(Some(LookupResult {
                kind: LookupKind::RawExact,
                entry,
            }));
        }
        if let Some(entry) = self.lookup_raw_ignore_case(query)? {
            return Ok(Some(LookupResult {
                kind: LookupKind::RawExact,
                entry,
            }));
        }

        let normalized = normalize_query(query);
        if normalized.is_empty() {
            return Ok(None);
        }
        if let Some(entry) = self.lookup_normalized_exact(&normalized)? {
            return Ok(Some(LookupResult {
                kind: LookupKind::NormalizedExact,
                entry,
            }));
        }

        let Some(nearest) = self.nearest(&normalized, true)? else {
            return Ok(None);
        };
        let entry = self
            .lookup_raw_exact(&nearest.candidate.word)?
            .or(self.lookup_normalized_exact(&nearest.candidate.sw)?)
            .ok_or(DatabaseError::MissingSchema)?;
        Ok(Some(LookupResult {
            kind: LookupKind::Closest {
                distance: nearest.distance,
            },
            entry,
        }))
    }

    pub fn lookup_exact_word(&self, word: &str) -> Result<Option<Entry>, DatabaseError> {
        if let Some(entry) = self.lookup_raw_exact(word)? {
            return Ok(Some(entry));
        }
        if let Some(entry) = self.lookup_raw_ignore_case(word)? {
            return Ok(Some(entry));
        }
        self.lookup_normalized_exact(&normalize_query(word))
    }

    pub fn lookup_by_id(&self, id: i64) -> Result<Option<Entry>, DatabaseError> {
        self.connection
            .query_row(
                &format!("SELECT {ENTRY_COLUMNS} FROM stardict WHERE id = ?1 LIMIT 1"),
                params![id],
                row_to_entry,
            )
            .optional()
            .map_err(DatabaseError::Query)
    }

    pub fn suggest(&self, query: &str, limit: usize) -> Result<Vec<Suggestion>, DatabaseError> {
        let trimmed = query.trim();
        let normalized = normalize_query(query);
        if normalized.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let limit = limit.min(50);
        let upper = format!("{normalized}\u{10ffff}");
        let mut statement = self.connection.prepare(&format!(
            "SELECT {SUGGESTION_COLUMNS}
                 FROM stardict
                 WHERE sw >= ?1 AND sw < ?2
                 ORDER BY sw, word COLLATE NOCASE
                 LIMIT 200"
        ))?;
        let rows = statement.query_map(params![normalized, upper], row_to_suggestion_candidate)?;
        let mut candidates = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Query)?;
        candidates.sort_by(|left, right| {
            if left.is_better_than(right, trimmed, &normalized) {
                std::cmp::Ordering::Less
            } else if right.is_better_than(left, trimmed, &normalized) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        Ok(candidates
            .into_iter()
            .take(limit)
            .map(SuggestionCandidate::into_suggestion)
            .collect())
    }

    pub fn search_page(
        &self,
        query: &str,
        after: Option<&SearchCursor>,
        page_size: usize,
    ) -> Result<SearchPage, DatabaseError> {
        let page_size = page_size.clamp(1, 1_000);
        let trimmed = query.trim();
        let normalized = normalize_query(trimmed);
        if normalized.is_empty() {
            return Ok(SearchPage {
                items: Vec::new(),
                next_cursor: None,
                exhausted: true,
            });
        }

        let mut state = after.cloned().unwrap_or_else(SearchCursor::start);
        let mut items = Vec::with_capacity(page_size);
        loop {
            match state.phase {
                SearchPhase::RawExact => {
                    let raw = self
                        .connection
                        .query_row(
                            &format!(
                                "SELECT {SUGGESTION_COLUMNS}
                                 FROM stardict
                                 WHERE word = ?1 COLLATE BINARY
                                 LIMIT 1"
                            ),
                            params![trimmed],
                            row_to_suggestion_candidate,
                        )
                        .optional()?;
                    if let Some(raw) = raw {
                        state.raw_exact_id = Some(raw.id);
                        if items.len() < page_size {
                            items.push(raw.into_suggestion());
                        }
                    }
                    state.phase = SearchPhase::NormalizedExact;
                    state.last_sw = None;
                    state.last_word = None;
                    state.last_id = None;
                }
                SearchPhase::NormalizedExact => {
                    if items.len() >= page_size {
                        return Ok(SearchPage {
                            items,
                            next_cursor: Some(state),
                            exhausted: false,
                        });
                    }
                    let remaining = page_size - items.len();
                    let raw_exact_id = state.raw_exact_id.unwrap_or(0);
                    let mut candidates = if let (Some(last_word), Some(last_id)) =
                        (state.last_word.as_deref(), state.last_id)
                    {
                        let mut statement = self.connection.prepare(&format!(
                            "SELECT {SUGGESTION_COLUMNS}
                             FROM stardict
                             WHERE sw = ?1
                               AND id <> ?2
                               AND (
                                   word COLLATE NOCASE > ?3
                                   OR (word COLLATE NOCASE = ?3 AND id > ?4)
                               )
                             ORDER BY word COLLATE NOCASE, id
                             LIMIT ?5"
                        ))?;
                        statement
                            .query_map(
                                params![
                                    normalized,
                                    raw_exact_id,
                                    last_word,
                                    last_id,
                                    (remaining + 1) as i64
                                ],
                                row_to_suggestion_candidate,
                            )?
                            .collect::<Result<Vec<_>, _>>()?
                    } else {
                        let mut statement = self.connection.prepare(&format!(
                            "SELECT {SUGGESTION_COLUMNS}
                             FROM stardict
                             WHERE sw = ?1
                               AND id <> ?2
                             ORDER BY word COLLATE NOCASE, id
                             LIMIT ?3"
                        ))?;
                        statement
                            .query_map(
                                params![normalized, raw_exact_id, (remaining + 1) as i64],
                                row_to_suggestion_candidate,
                            )?
                            .collect::<Result<Vec<_>, _>>()?
                    };
                    let page_done = candidates.len() <= remaining;
                    for candidate in candidates.drain(..).take(remaining) {
                        state.last_word = Some(candidate.candidate.word.clone());
                        state.last_id = Some(candidate.id);
                        items.push(candidate.into_suggestion());
                    }
                    if !page_done {
                        return Ok(SearchPage {
                            items,
                            next_cursor: Some(state),
                            exhausted: false,
                        });
                    }
                    state.phase = SearchPhase::Prefix;
                    state.last_sw = None;
                    state.last_word = None;
                    state.last_id = None;
                }
                SearchPhase::Prefix => {
                    if items.len() >= page_size {
                        return Ok(SearchPage {
                            items,
                            next_cursor: Some(state),
                            exhausted: false,
                        });
                    }
                    let remaining = page_size - items.len();
                    let upper = format!("{normalized}\u{10ffff}");
                    let mut candidates = if let (Some(last_sw), Some(last_word), Some(last_id)) = (
                        state.last_sw.as_deref(),
                        state.last_word.as_deref(),
                        state.last_id,
                    ) {
                        let mut statement = self.connection.prepare(&format!(
                            "SELECT {SUGGESTION_COLUMNS}
                                 FROM stardict
                                 WHERE sw >= ?1
                                   AND sw < ?2
                                   AND sw <> ?1
                                   AND (
                                       sw > ?3
                                       OR (sw = ?3 AND word COLLATE NOCASE > ?4)
                                       OR (
                                           sw = ?3
                                           AND word COLLATE NOCASE = ?4
                                           AND id > ?5
                                       )
                                   )
                                 ORDER BY sw, word COLLATE NOCASE, id
                                 LIMIT ?6"
                        ))?;
                        statement
                            .query_map(
                                params![
                                    normalized,
                                    upper,
                                    last_sw,
                                    last_word,
                                    last_id,
                                    (remaining + 1) as i64
                                ],
                                row_to_suggestion_candidate,
                            )?
                            .collect::<Result<Vec<_>, _>>()?
                    } else {
                        let mut statement = self.connection.prepare(&format!(
                            "SELECT {SUGGESTION_COLUMNS}
                                 FROM stardict
                                 WHERE sw >= ?1
                                   AND sw < ?2
                                   AND sw <> ?1
                                 ORDER BY sw, word COLLATE NOCASE, id
                                 LIMIT ?3"
                        ))?;
                        statement
                            .query_map(
                                params![normalized, upper, (remaining + 1) as i64],
                                row_to_suggestion_candidate,
                            )?
                            .collect::<Result<Vec<_>, _>>()?
                    };
                    let page_done = candidates.len() <= remaining;
                    for candidate in candidates.drain(..).take(remaining) {
                        state.last_sw = Some(candidate.candidate.sw.clone());
                        state.last_word = Some(candidate.candidate.word.clone());
                        state.last_id = Some(candidate.id);
                        items.push(candidate.into_suggestion());
                    }
                    if !page_done {
                        return Ok(SearchPage {
                            items,
                            next_cursor: Some(state),
                            exhausted: false,
                        });
                    }
                    state.phase = SearchPhase::Done;
                }
                SearchPhase::Done => {
                    return Ok(SearchPage {
                        items,
                        next_cursor: None,
                        exhausted: true,
                    });
                }
            }
        }
    }

    fn lookup_raw_exact(&self, word: &str) -> Result<Option<Entry>, DatabaseError> {
        self.connection
            .query_row(
                &format!(
                    "SELECT {ENTRY_COLUMNS}
                     FROM stardict
                     WHERE word = ?1 COLLATE BINARY
                     ORDER BY collins DESC, oxford DESC, coalesce(frq, 2147483647), id
                     LIMIT 1"
                ),
                params![word],
                row_to_entry,
            )
            .optional()
            .map_err(DatabaseError::Query)
    }

    fn lookup_raw_ignore_case(&self, word: &str) -> Result<Option<Entry>, DatabaseError> {
        self.connection
            .query_row(
                &format!(
                    "SELECT {ENTRY_COLUMNS}
                     FROM stardict
                     WHERE word = ?1 COLLATE NOCASE
                     ORDER BY collins DESC, oxford DESC, coalesce(frq, 2147483647), id
                     LIMIT 1"
                ),
                params![word],
                row_to_entry,
            )
            .optional()
            .map_err(DatabaseError::Query)
    }

    fn lookup_normalized_exact(&self, normalized: &str) -> Result<Option<Entry>, DatabaseError> {
        self.connection
            .query_row(
                &format!(
                    "SELECT {ENTRY_COLUMNS}
                     FROM stardict
                     WHERE sw = ?1
                     ORDER BY collins DESC, oxford DESC, coalesce(frq, 2147483647), word
                     LIMIT 1"
                ),
                params![normalized],
                row_to_entry,
            )
            .optional()
            .map_err(DatabaseError::Query)
    }

    fn nearest(
        &self,
        normalized: &str,
        allow_global_scan: bool,
    ) -> Result<Option<RankedCandidate>, DatabaseError> {
        if normalized.is_empty() {
            return Ok(None);
        }

        let mut best = self.scan_variant_candidates(normalized)?;
        let neighborhood = self.scan_neighborhood(normalized, 5_000)?;
        if better_candidate(&neighborhood, &best) {
            best = neighborhood;
        }
        self.scan_prefix_candidates(normalized, 200, &mut best)?;
        if best
            .as_ref()
            .is_some_and(|candidate| candidate.distance <= nearest_threshold(normalized))
        {
            return Ok(best);
        }

        let expanded = self.scan_neighborhood(normalized, 20_000)?;
        if better_candidate(&expanded, &best) {
            best = expanded;
        }
        if !allow_global_scan
            || best
                .as_ref()
                .is_some_and(|candidate| candidate.distance <= nearest_threshold(normalized))
        {
            return Ok(best);
        }

        let scanned = self.scan_all_candidates(normalized)?;
        if better_candidate(&scanned, &best) {
            best = scanned;
        }
        Ok(best)
    }

    fn scan_neighborhood(
        &self,
        normalized: &str,
        radius: usize,
    ) -> Result<Option<RankedCandidate>, DatabaseError> {
        let mut best = None;
        for lower in [true, false] {
            let sql = if lower {
                CANDIDATE_LOWER_SQL
            } else {
                CANDIDATE_UPPER_SQL
            };
            let mut statement = self.connection.prepare(sql)?;
            let rows = statement.query_map(params![normalized, radius as i64], row_to_candidate)?;
            for candidate in rows {
                consider_candidate(candidate?, normalized, &mut best);
            }
        }
        Ok(best)
    }

    fn scan_prefix_candidates(
        &self,
        normalized: &str,
        limit: usize,
        best: &mut Option<RankedCandidate>,
    ) -> Result<(), DatabaseError> {
        let upper = format!("{normalized}\u{10ffff}");
        let mut statement = self.connection.prepare(
            "SELECT word, sw, frq, bnc, coalesce(collins, 0), coalesce(oxford, 0), tag
             FROM stardict
             WHERE sw >= ?1 AND sw < ?2
             ORDER BY sw, word COLLATE NOCASE
             LIMIT ?3",
        )?;
        let rows =
            statement.query_map(params![normalized, upper, limit as i64], row_to_candidate)?;
        for candidate in rows {
            consider_candidate(candidate?, normalized, best);
        }
        Ok(())
    }

    fn scan_variant_candidates(
        &self,
        normalized: &str,
    ) -> Result<Option<RankedCandidate>, DatabaseError> {
        let variants = edit_distance_one_variants(normalized);
        if variants.is_empty() {
            return Ok(None);
        }
        let placeholders = std::iter::repeat_n("?", variants.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT {CANDIDATE_COLUMNS}
             FROM stardict
             WHERE sw IN ({placeholders})
             LIMIT 10000"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(variants.iter()), row_to_candidate)?;
        let mut best = None;
        for candidate in rows {
            consider_candidate(candidate?, normalized, &mut best);
        }
        Ok(best)
    }

    fn scan_all_candidates(
        &self,
        normalized: &str,
    ) -> Result<Option<RankedCandidate>, DatabaseError> {
        let mut statement = self
            .connection
            .prepare(&format!("SELECT {CANDIDATE_COLUMNS} FROM stardict"))?;
        let rows = statement.query_map([], row_to_candidate)?;
        let mut best = None;
        for candidate in rows {
            consider_candidate(candidate?, normalized, &mut best);
        }
        Ok(best)
    }
}

const CANDIDATE_LOWER_SQL: &str = "
    SELECT word, sw, frq, bnc, coalesce(collins, 0), coalesce(oxford, 0), tag
    FROM stardict
    WHERE sw < ?1
    ORDER BY sw DESC
    LIMIT ?2
";

const CANDIDATE_UPPER_SQL: &str = "
    SELECT word, sw, frq, bnc, coalesce(collins, 0), coalesce(oxford, 0), tag
    FROM stardict
    WHERE sw >= ?1
    ORDER BY sw ASC
    LIMIT ?2
";

fn nearest_threshold(normalized: &str) -> usize {
    (normalized.chars().count() / 3).max(2)
}

fn edit_distance_one_variants(normalized: &str) -> BTreeSet<String> {
    let chars: Vec<char> = normalized.chars().collect();
    let mut variants = BTreeSet::new();
    if chars.len() < 2 || chars.len() > 24 {
        return variants;
    }

    for index in 0..chars.len() {
        let mut deleted = chars.clone();
        deleted.remove(index);
        if !deleted.is_empty() {
            variants.insert(deleted.iter().collect());
        }
    }

    for index in 0..chars.len().saturating_sub(1) {
        let mut swapped = chars.clone();
        swapped.swap(index, index + 1);
        variants.insert(swapped.iter().collect());
    }

    for index in 0..=chars.len() {
        for replacement in 'a'..='z' {
            let mut inserted = chars.clone();
            inserted.insert(index, replacement);
            variants.insert(inserted.iter().collect());
        }
    }

    for index in 0..chars.len() {
        for replacement in 'a'..='z' {
            if chars[index] == replacement {
                continue;
            }
            let mut substituted = chars.clone();
            substituted[index] = replacement;
            variants.insert(substituted.iter().collect());
        }
    }

    variants
}

fn consider_candidate(candidate: Candidate, normalized: &str, best: &mut Option<RankedCandidate>) {
    let ranked = RankedCandidate::new(candidate, normalized);
    if best
        .as_ref()
        .is_none_or(|current| ranked.is_better_than(current))
    {
        *best = Some(ranked);
    }
}

fn better_candidate(
    candidate: &Option<RankedCandidate>,
    current: &Option<RankedCandidate>,
) -> bool {
    match (candidate, current) {
        (Some(candidate), Some(current)) => candidate.is_better_than(current),
        (Some(_), None) => true,
        _ => false,
    }
}

fn row_to_candidate(row: &Row<'_>) -> rusqlite::Result<Candidate> {
    Ok(Candidate {
        word: row.get(0)?,
        sw: row.get(1)?,
        frq: row.get(2)?,
        bnc: row.get(3)?,
        collins: row.get(4)?,
        oxford: row.get(5)?,
        tags: row.get(6)?,
    })
}

fn row_to_suggestion_candidate(row: &Row<'_>) -> rusqlite::Result<SuggestionCandidate> {
    Ok(SuggestionCandidate {
        id: row.get(0)?,
        candidate: Candidate {
            word: row.get(1)?,
            sw: row.get(2)?,
            frq: row.get(3)?,
            bnc: row.get(4)?,
            collins: row.get(5)?,
            oxford: row.get(6)?,
            tags: row.get(7)?,
        },
        phonetic: row.get(8)?,
        translation: row.get(9)?,
    })
}

fn row_to_entry(row: &Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: row.get(0)?,
        word: row.get(1)?,
        phonetic: row.get(2)?,
        definition: row.get(3)?,
        translation: row.get(4)?,
        pos: row.get(5)?,
        collins: row.get(6)?,
        oxford: row.get(7)?,
        tags: row.get(8)?,
        bnc: row.get(9)?,
        frq: row.get(10)?,
        exchange: row.get(11)?,
        detail: row.get(12)?,
        audio: row.get(13)?,
    })
}

fn first_line(text: Option<&str>) -> String {
    text.unwrap_or_default()
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use tempfile::TempDir;

    use super::{LexVault, LookupKind};

    fn fixture() -> (TempDir, LexVault) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("stardict.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE stardict (
                    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL UNIQUE,
                    word VARCHAR(64) COLLATE NOCASE NOT NULL,
                    sw VARCHAR(64) COLLATE NOCASE NOT NULL,
                    phonetic VARCHAR(64),
                    definition TEXT,
                    translation TEXT,
                    pos VARCHAR(16),
                    collins INTEGER DEFAULT 0,
                    oxford INTEGER DEFAULT 0,
                    tag VARCHAR(64),
                    bnc INTEGER DEFAULT NULL,
                    frq INTEGER DEFAULT NULL,
                    exchange TEXT,
                    detail TEXT,
                    audio TEXT
                );
                CREATE INDEX stardict_3 ON stardict (sw, word COLLATE NOCASE);
                CREATE INDEX sd_1 ON stardict (word COLLATE NOCASE);
                ",
            )
            .unwrap();
        let rows = [
            ("able", "able", "能够的", 385, 295, 5, 1),
            ("apple", "apple", "苹果", 2695, 2446, 3, 1),
            ("take", "take", "拿；取", 63, 51, 1, 1),
            ("tka", "tka", "缩写", 0, 0, 0, 0),
            ("kiss", "kiss", "亲吻", 0, 0, 0, 0),
            ("long-time", "longtime", "长期的", 0, 0, 0, 0),
        ];
        for (word, sw, translation, bnc, frq, collins, oxford) in rows {
            connection
                .execute(
                    "INSERT INTO stardict
                     (word, sw, phonetic, definition, translation, pos, collins, oxford,
                      tag, bnc, frq, exchange, detail, audio)
                     VALUES (?1, ?2, ?3, NULL, ?4, NULL, ?5, ?6, NULL, ?7, ?8, NULL, NULL, NULL)",
                    rusqlite::params![
                        word,
                        sw,
                        if word == "apple" {
                            Some("ˈæpl")
                        } else {
                            None
                        },
                        translation,
                        collins,
                        oxford,
                        bnc,
                        frq
                    ],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO stardict (word, sw, translation) VALUES ('nulls', 'nulls', '空字段')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO stardict (word, sw, translation, collins, oxford, bnc, frq)
                 VALUES ('K.I.S.S.', 'kiss', '保持简单', 0, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO stardict (word, sw, translation, collins, oxford, bnc, frq)
                 VALUES ('kiss-', 'kiss', '吻；亲吻', 0, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO stardict (word, sw, translation)
                 VALUES ('Test', 'test', '大写测试')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO stardict (word, sw, translation)
                 VALUES ('test', 'test', '小写测试')",
                [],
            )
            .unwrap();
        for index in 1..=25 {
            let word = format!("page{index:03}");
            connection
                .execute(
                    "INSERT INTO stardict (word, sw, translation) VALUES (?1, ?1, ?2)",
                    rusqlite::params![word, format!("分页词条 {index}")],
                )
                .unwrap();
        }
        drop(connection);
        let dictionary = LexVault::open(&path).unwrap();
        (dir, dictionary)
    }

    #[test]
    fn raw_and_normalized_exact_queries_work() {
        let (_dir, dictionary) = fixture();
        assert_eq!(
            dictionary.lookup("apple").unwrap().unwrap().kind,
            LookupKind::RawExact
        );
        assert_eq!(
            dictionary.lookup("LONG TIME").unwrap().unwrap().kind,
            LookupKind::NormalizedExact
        );
        let with_nulls = dictionary.lookup("nulls").unwrap().unwrap().entry;
        assert_eq!(with_nulls.collins, 0);
        assert_eq!(with_nulls.oxford, 0);
    }

    #[test]
    fn nearest_ranking_prefers_common_prefix_and_equal_length() {
        let (_dir, dictionary) = fixture();
        let aple = dictionary.lookup("aple").unwrap().unwrap();
        assert_eq!(aple.entry.word, "apple");
        assert!(matches!(aple.kind, LookupKind::Closest { distance: 1 }));

        let tkae = dictionary.lookup("tkae").unwrap().unwrap();
        assert_eq!(tkae.entry.word, "take");
        assert!(matches!(tkae.kind, LookupKind::Closest { distance: 1 }));
    }

    #[test]
    fn suggestions_use_the_normalized_prefix() {
        let (_dir, dictionary) = fixture();
        let suggestions = dictionary.suggest("app", 8).unwrap();
        assert_eq!(suggestions[0].word, "apple");
    }

    #[test]
    fn suggestions_prefer_raw_exact_before_punctuated_variants() {
        let (_dir, dictionary) = fixture();
        let suggestions = dictionary.suggest("kiss", 8).unwrap();
        assert_eq!(suggestions[0].word, "kiss");
    }

    #[test]
    fn search_pages_cover_every_prefix_match_without_duplicates() {
        let (_dir, dictionary) = fixture();
        let mut cursor = None;
        let mut words = Vec::new();
        let mut pages = 0;
        loop {
            let page = dictionary.search_page("page", cursor.as_ref(), 4).unwrap();
            assert!(page.items.len() <= 4);
            words.extend(page.items.into_iter().map(|item| item.word));
            pages += 1;
            if page.exhausted {
                break;
            }
            cursor = page.next_cursor;
            assert!(cursor.is_some());
            assert!(pages < 20);
        }
        assert_eq!(words.len(), 25);
        let mut unique = words.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 25);
        assert_eq!(words.first().unwrap(), "page001");
        assert_eq!(words.last().unwrap(), "page025");
    }

    #[test]
    fn search_pages_put_raw_exact_match_first() {
        let (_dir, dictionary) = fixture();
        let first = dictionary.search_page("kiss", None, 1).unwrap();
        assert_eq!(first.items[0].word, "kiss");
        let second = dictionary
            .search_page("kiss", first.next_cursor.as_ref(), 10)
            .unwrap();
        let words = second
            .items
            .into_iter()
            .map(|item| item.word)
            .collect::<Vec<_>>();
        assert!(words.contains(&"K.I.S.S.".to_owned()), "{words:?}");
        assert!(words.contains(&"kiss-".to_owned()), "{words:?}");
    }

    #[test]
    fn exact_lookup_prefers_the_matching_case() {
        let (_dir, dictionary) = fixture();

        let lowercase = dictionary.lookup("test").unwrap().unwrap();
        assert_eq!(lowercase.kind, LookupKind::RawExact);
        assert_eq!(lowercase.entry.word, "test");
        assert_eq!(lowercase.entry.translation.as_deref(), Some("小写测试"));

        let uppercase = dictionary.lookup("Test").unwrap().unwrap();
        assert_eq!(uppercase.kind, LookupKind::RawExact);
        assert_eq!(uppercase.entry.word, "Test");
        assert_eq!(uppercase.entry.translation.as_deref(), Some("大写测试"));
    }

    #[test]
    fn tui_suggestions_keep_each_case_variant_distinct() {
        let (_dir, dictionary) = fixture();
        let page = dictionary.search_page("test", None, 10).unwrap();
        assert_eq!(page.items[0].word, "test");

        let lowercase = page
            .items
            .iter()
            .find(|suggestion| suggestion.word == "test")
            .unwrap();
        let uppercase = page
            .items
            .iter()
            .find(|suggestion| suggestion.word == "Test")
            .unwrap();
        assert_ne!(lowercase.id, uppercase.id);
        assert_eq!(
            dictionary
                .lookup_by_id(lowercase.id)
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("小写测试")
        );
        assert_eq!(
            dictionary
                .lookup_by_id(uppercase.id)
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("大写测试")
        );
    }
}
