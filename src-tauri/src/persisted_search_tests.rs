use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    format!(
        "{}_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
    )
}

struct DiskFixture(PathBuf);

impl DiskFixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("otto-persisted-search-{}.db", unique_suffix())))
    }
}

impl Drop for DiskFixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn fixture(path: &Path) -> Connection {
    let conn = init_db(path).unwrap();
    conn.execute_batch(
        "INSERT INTO favorites(id, name) VALUES (1, 'library');
         INSERT INTO favorite_paths(favorite_id, path) VALUES (1, '/samples');
         INSERT INTO folders(id, path) VALUES (1, '/samples');",
    )
    .unwrap();
    conn
}

fn insert(conn: &Connection, path: &str, filename: &str, version: i32, index: &str) {
    conn.execute(
        "INSERT INTO files(folder_id, path, filename, filename_lower, pinyin_index,
             pinyin_index_version, size, mtime, file_type)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, 123, 456, 'audio')",
        params![path, filename, filename.to_lowercase(), index, version],
    )
    .unwrap();
}

fn stored(conn: &Connection, path: &str) -> (String, i32) {
    conn.query_row(
        "SELECT pinyin_index, pinyin_index_version FROM files WHERE path=?1",
        params![path],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

// Build persisted fixtures without populating SEARCH_NAME_CACHE. Cold-search
// assertions would otherwise pass even if the database blob were ignored.
fn uncached_index(filename: &str) -> String {
    serde_json::to_string(&character_variants(&filename.to_lowercase()).unwrap()).unwrap()
}

fn paths(conn: &Connection, query: &str) -> Vec<String> {
    search_file_page(conn, "library", query, "all", "name", "asc", 0)
        .unwrap()
        .records
        .into_iter()
        .map(|record| record.path)
        .collect()
}

#[test]
fn reopened_current_index_searches_unicode_and_extension_without_pinyin_conversion() {
    let disk = DiskFixture::new();
    let conn = fixture(&disk.0);
    let filename = format!("İ重庆_谁手{}.WAV", unique_suffix());
    let index = uncached_index(&filename);
    insert(
        &conn,
        "/samples/cold.wav",
        &filename,
        PINYIN_INDEX_VERSION,
        &index,
    );
    drop(conn);

    let conn = Connection::open(&disk.0).unwrap();
    let calls = filename_pinyin::thread_call_count();
    for query in [
        "chongqing",
        "i\u{307} 重庆",
        "shui",
        "_",
        ".wav",
        "重庆 shou",
        "chongq",
    ] {
        assert_eq!(paths(&conn, query), ["/samples/cold.wav"], "query={query}");
    }
    assert!(
        paths(&conn, "ou shou").is_empty(),
        "one source span cannot satisfy two terms"
    );
    assert!(
        paths(&conn, "shei").is_empty(),
        "frequency reading of 谁 is only shui"
    );
    assert_eq!(
        filename_pinyin::thread_call_count(),
        calls,
        "persisted data must serve a cold search"
    );
    assert_eq!(
        stored(&conn, "/samples/cold.wav"),
        (index, PINYIN_INDEX_VERSION)
    );
}

#[test]
fn legacy_and_corrupt_rows_are_repaired_once_and_persist_the_full_filename() {
    let conn = fixture(Path::new(":memory:"));
    let cases = [
        (5, "[]"),
        (6, "[]"),
        (PINYIN_INDEX_VERSION, "not-json"),
        (PINYIN_INDEX_VERSION, "[]"),
        (PINYIN_INDEX_VERSION, "[[\"错\",\"cuo\"]]"),
    ];
    let mut expected = Vec::new();
    for (i, (version, old_index)) in cases.iter().enumerate() {
        let filename = format!("修复重开{}{}.WAV", i, unique_suffix());
        let path = format!("/samples/repair-{i}.wav");
        let index = uncached_index(&filename);
        insert(&conn, &path, &filename, *version, old_index);
        expected.push((path, index));
    }
    let calls = filename_pinyin::thread_call_count();
    assert_eq!(paths(&conn, "xiufu chongkai .wav").len(), cases.len());
    assert_eq!(filename_pinyin::thread_call_count() - calls, cases.len());
    for (path, index) in expected {
        assert_eq!(stored(&conn, &path), (index, PINYIN_INDEX_VERSION));
    }
    let calls = filename_pinyin::thread_call_count();
    assert_eq!(paths(&conn, "修复 .wav").len(), cases.len());
    assert_eq!(filename_pinyin::thread_call_count(), calls);
}

#[test]
fn contextual_v6_indexes_gain_common_alternatives_and_lose_extra_readings() {
    let conn = fixture(Path::new(":memory:"));
    let filename = format!("长重能谁艹丶{}.wav", unique_suffix());
    let old_units: Vec<Vec<String>> = filename
        .to_lowercase()
        .chars()
        .map(|c| match c {
            '长' => vec!["长".to_owned(), "zhang".to_owned()],
            '重' => vec!["重".to_owned(), "zhong".to_owned()],
            '能' => vec!["能".to_owned(), "neng".to_owned()],
            '谁' => vec!["谁".to_owned(), "shei".to_owned(), "shui".to_owned()],
            '艹' => vec!["艹".to_owned(), "cao".to_owned()],
            '丶' => vec!["丶".to_owned(), "zhu".to_owned()],
            _ => vec![c.to_string()],
        })
        .collect();
    let old_index = serde_json::to_string(&old_units).unwrap();
    insert(
        &conn,
        "/samples/contextual-v6.wav",
        &filename,
        6,
        &old_index,
    );
    for query in [
        "chang chong neng shui cao zhu",
        "zhang zhong",
        "长重能谁艹丶",
    ] {
        assert_eq!(
            paths(&conn, query),
            ["/samples/contextual-v6.wav"],
            "{query}"
        );
    }
    for query in ["shei", "nai", "tai", "xiong"] {
        assert!(
            paths(&conn, query).is_empty(),
            "removed pronunciation {query}"
        );
    }
    assert_eq!(
        stored(&conn, "/samples/contextual-v6.wav"),
        (uncached_index(&filename), PINYIN_INDEX_VERSION)
    );
}

#[test]
fn saved_character_indexes_roundtrip_characters_with_several_common_readings() {
    // kHanyuPinlu contains four distinct untoned readings for 著 and 着.
    // This specifically catches the old decoder's limit of two pronunciations.
    let filename = "著着嗯女能艹丶😀İ.wav";
    let lower = filename.to_lowercase();
    let expected = character_variants(&lower).unwrap();
    let encoded = serde_json::to_string(&expected).unwrap();
    assert_eq!(
        decode_pinyin_index(&lower, &encoded, PINYIN_INDEX_VERSION),
        Some(expected)
    );
    for query in ["zhe", "zhao", "zhu", "zhuo"] {
        assert!(FilenameQuery::compile(query).matches("著.wav"), "{query}");
    }
    for query in ["zhe", "zhao", "zhuo"] {
        assert!(FilenameQuery::compile(query).matches("着.wav"), "{query}");
    }

    // Validate the storage format against every character in the vendored
    // source, including entries with more alternatives than curated examples.
    for line in include_str!("../vendor/pinyin-data/kHanyuPinlu.txt").lines() {
        let Some((code, _)) = line.split_once(':') else {
            continue;
        };
        let Some(hex) = code.strip_prefix("U+") else {
            continue;
        };
        let c = char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap();
        let text = c.to_string();
        let expected = character_variants(&text).unwrap();
        let encoded = serde_json::to_string(&expected).unwrap();
        assert_eq!(
            decode_pinyin_index(&text, &encoded, PINYIN_INDEX_VERSION),
            Some(expected),
            "cannot reload index for {code}"
        );
    }
}

#[test]
fn unfiltered_listing_does_not_build_or_rewrite_legacy_indexes() {
    let conn = fixture(Path::new(":memory:"));
    let filename = format!("列表不转换{}.mp3", unique_suffix());
    insert(&conn, "/samples/list.mp3", &filename, 5, "legacy");
    let calls = filename_pinyin::thread_call_count();
    assert_eq!(paths(&conn, "  "), ["/samples/list.mp3"]);
    assert_eq!(filename_pinyin::thread_call_count(), calls);
    assert_eq!(stored(&conn, "/samples/list.mp3"), ("legacy".to_owned(), 5));
}

#[test]
fn repair_does_not_overwrite_a_renamed_updated_or_deleted_record() {
    let conn = fixture(Path::new(":memory:"));
    for path in ["renamed", "updated", "deleted", "unchanged"] {
        insert(&conn, path, "原文件.wav", 5, "old");
    }
    let repairs: Vec<_> = ["renamed", "updated", "deleted", "unchanged"]
        .into_iter()
        .map(|path| SearchIndexRepair {
            path: path.to_owned(),
            filename: "原文件.wav".to_owned(),
            old_version: 5,
            old_index: "old".to_owned(),
            new_index: "new".to_owned(),
        })
        .collect();
    conn.execute(
        "UPDATE files SET filename='改名.wav' WHERE path='renamed'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE files SET pinyin_index='scanner', pinyin_index_version=?1 WHERE path='updated'",
        params![PINYIN_INDEX_VERSION],
    )
    .unwrap();
    conn.execute("DELETE FROM files WHERE path='deleted'", [])
        .unwrap();
    persist_search_repairs(&conn, &repairs).unwrap();
    assert_eq!(stored(&conn, "renamed"), ("old".to_owned(), 5));
    assert_eq!(
        stored(&conn, "updated"),
        ("scanner".to_owned(), PINYIN_INDEX_VERSION)
    );
    assert_eq!(
        stored(&conn, "unchanged"),
        ("new".to_owned(), PINYIN_INDEX_VERSION)
    );
    let deleted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path='deleted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deleted, 0);
}

#[test]
fn busy_repair_preserves_search_results_and_can_retry_after_writer_finishes() {
    for version in [5, PINYIN_INDEX_VERSION] {
        let disk = DiskFixture::new();
        let conn = fixture(&disk.0);
        let filename = format!("忙碌银行{}.wav", unique_suffix());
        let expected = uncached_index(&filename);
        insert(&conn, "/samples/busy.wav", &filename, version, "legacy");
        let writer = Connection::open(&disk.0).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.busy_timeout(Duration::from_millis(1250)).unwrap();
        let started = Instant::now();
        assert_eq!(paths(&conn, "manglu yinhang"), ["/samples/busy.wav"]);
        assert!(
            started.elapsed() < Duration::from_millis(1000),
            "optional repair must skip a busy writer without waiting for the connection's timeout"
        );
        let timeout: u32 = conn
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(
            timeout, 1250,
            "busy repair must restore the caller's timeout"
        );
        assert_eq!(
            stored(&conn, "/samples/busy.wav"),
            ("legacy".to_owned(), version)
        );
        writer.execute_batch("ROLLBACK").unwrap();
        assert_eq!(paths(&conn, "manglu yinhang"), ["/samples/busy.wav"]);
        let timeout: u32 = conn
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(
            timeout, 1250,
            "successful repair must restore the caller's timeout"
        );
        assert_eq!(
            stored(&conn, "/samples/busy.wav"),
            (expected, PINYIN_INDEX_VERSION)
        );
    }
}

#[test]
fn read_only_legacy_search_returns_results_without_repairing_the_database() {
    let disk = DiskFixture::new();
    let conn = fixture(&disk.0);
    let filename = format!("只读重庆{}.wav", unique_suffix());
    insert(&conn, "/samples/read-only.wav", &filename, 5, "legacy");
    drop(conn);
    let conn =
        Connection::open_with_flags(&disk.0, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(conn.is_readonly("main").unwrap());
    assert_eq!(paths(&conn, "zhidu chongqing"), ["/samples/read-only.wav"]);
    assert_eq!(paths(&conn, "只读 重庆"), ["/samples/read-only.wav"]);
    assert_eq!(
        stored(&conn, "/samples/read-only.wav"),
        ("legacy".to_owned(), 5)
    );
    drop(conn);
    let reopened = Connection::open(&disk.0).unwrap();
    assert_eq!(
        stored(&reopened, "/samples/read-only.wav"),
        ("legacy".to_owned(), 5)
    );
}

/// Run this test alone: it deliberately resets the process-wide filename cache.
/// The user's index is opened read-only; migration is exercised on a disposable
/// fixture containing copies of filenames, never on the source database.
#[test]
#[ignore = "set OTTO_CORPUS_DB and run only audit_persisted_corpus with --ignored --nocapture"]
fn audit_persisted_corpus() {
    let source_path = std::env::var("OTTO_CORPUS_DB").expect("OTTO_CORPUS_DB required");
    let source =
        Connection::open_with_flags(source_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let names: Vec<String> = source
        .prepare("SELECT filename FROM files ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    drop(source);
    assert!(!names.is_empty(), "corpus must contain filenames");
    let queries = [
        "nai shi",
        "naishi",
        "nengshi",
        "neng shi",
        "可能是",
        "nashi",
        "chongqing",
        "yinyue",
        "yuedui",
        "kuaile",
        "buxing",
        "otto guofen",
        "ou shou",
        "_",
        "%",
        "鼓kick",
        "shei",
        "shui",
    ];
    let synthetic_path = |i: usize| format!("/samples/corpus-{i:08}.wav");
    *SEARCH_NAME_CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap() = SearchNameCache::default();
    let mut expected = Vec::new();
    for query in queries {
        let matcher = FilenameQuery::compile(query);
        let started = Instant::now();
        let matches: Vec<String> = names
            .iter()
            .enumerate()
            .filter_map(|(i, name)| {
                matcher
                    .try_matches(name)
                    .unwrap()
                    .then(|| synthetic_path(i))
            })
            .collect();
        println!(
            "dynamic {query:?}: {} results, {:.3} ms",
            matches.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );
        expected.push(matches);
    }

    let disk = DiskFixture::new();
    let conn = fixture(&disk.0);
    conn.execute_batch("BEGIN").unwrap();
    for (i, name) in names.iter().enumerate() {
        insert(
            &conn,
            &synthetic_path(i),
            name,
            PINYIN_INDEX_VERSION,
            &uncached_index(name),
        );
    }
    conn.execute_batch("COMMIT").unwrap();
    drop(conn);
    let conn = Connection::open(&disk.0).unwrap();
    *SEARCH_NAME_CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap() = SearchNameCache::default();
    let calls = filename_pinyin::thread_call_count();
    for (query, expected) in queries.into_iter().zip(expected) {
        let started = Instant::now();
        let mut actual = Vec::new();
        let mut offset = 0;
        loop {
            let page =
                search_file_page(&conn, "library", query, "all", "name", "asc", offset).unwrap();
            actual.extend(page.records.into_iter().map(|record| record.path));
            if !page.has_more {
                break;
            }
            offset += SEARCH_PAGE_SIZE as u32;
        }
        actual.sort();
        assert_eq!(actual, expected, "persisted search differs for {query:?}");
        println!(
            "persisted {query:?}: {} results, {:.3} ms",
            actual.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    assert_eq!(
        filename_pinyin::thread_call_count(),
        calls,
        "reopened current database must need no pinyin conversion"
    );
    println!(
        "Reopened {} corpus rows: all {} query result sets equal; zero pinyin conversions",
        names.len(),
        queries.len()
    );
}
