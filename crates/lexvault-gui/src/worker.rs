use std::{
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
};

use lexvault_core::{Entry, LexVault, SearchCursor, SearchPage};

pub(crate) enum Request {
    Search {
        query_epoch: u64,
        query: String,
        cursor: Option<SearchCursor>,
        page_size: usize,
        append: bool,
    },
    Lookup {
        selection_epoch: u64,
        id: i64,
    },
}

pub(crate) enum Response {
    Search {
        query_epoch: u64,
        append: bool,
        result: Result<SearchPage, String>,
    },
    Lookup {
        selection_epoch: u64,
        result: Result<Option<Entry>, String>,
    },
}

pub(crate) struct DictionaryWorker {
    requests: Sender<Request>,
    _thread: JoinHandle<()>,
}

impl DictionaryWorker {
    pub(crate) fn spawn(
        dictionary: LexVault,
        responses: Sender<Response>,
    ) -> std::io::Result<Self> {
        let (requests, receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("lexvault-dictionary".to_owned())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    let response = match request {
                        Request::Search {
                            query_epoch,
                            query,
                            cursor,
                            page_size,
                            append,
                        } => Response::Search {
                            query_epoch,
                            append,
                            result: dictionary
                                .search_page(&query, cursor.as_ref(), page_size)
                                .map_err(|error| error.to_string()),
                        },
                        Request::Lookup {
                            selection_epoch,
                            id,
                        } => Response::Lookup {
                            selection_epoch,
                            result: dictionary
                                .lookup_by_id(id)
                                .map_err(|error| error.to_string()),
                        },
                    };
                    if responses.send(response).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests,
            _thread: thread,
        })
    }

    pub(crate) fn send(&self, request: Request) -> bool {
        self.requests.send(request).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rusqlite::Connection;
    use tempfile::TempDir;

    use super::{DictionaryWorker, Request, Response};
    use lexvault_core::LexVault;

    fn dictionary() -> (TempDir, LexVault) {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("stardict.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE stardict (
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
                    audio TEXT
                );
                CREATE INDEX sd_1 ON stardict(word COLLATE NOCASE);
                CREATE INDEX stardict_3 ON stardict(sw, word COLLATE NOCASE);
                INSERT INTO stardict (word, sw, translation)
                VALUES ('apple', 'apple', '苹果');
                ",
            )
            .unwrap();
        drop(connection);
        let dictionary = LexVault::open(path).unwrap();
        (directory, dictionary)
    }

    #[test]
    fn searches_and_reads_entries_off_the_gui_thread() {
        let (_directory, dictionary) = dictionary();
        let (responses, received) = std::sync::mpsc::channel();
        let worker = DictionaryWorker::spawn(dictionary, responses).unwrap();

        assert!(worker.send(Request::Search {
            query_epoch: 7,
            query: "app".to_owned(),
            cursor: None,
            page_size: 10,
            append: false,
        }));
        let id = match received.recv_timeout(Duration::from_secs(2)).unwrap() {
            Response::Search {
                query_epoch,
                append,
                result,
            } => {
                assert_eq!(query_epoch, 7);
                assert!(!append);
                result.unwrap().items[0].id
            }
            Response::Lookup { .. } => panic!("expected a search response"),
        };

        assert!(worker.send(Request::Lookup {
            selection_epoch: 11,
            id,
        }));
        match received.recv_timeout(Duration::from_secs(2)).unwrap() {
            Response::Lookup {
                selection_epoch,
                result,
            } => {
                assert_eq!(selection_epoch, 11);
                assert_eq!(
                    result.unwrap().unwrap().translation.as_deref(),
                    Some("苹果")
                );
            }
            Response::Search { .. } => panic!("expected a lookup response"),
        }
    }
}
