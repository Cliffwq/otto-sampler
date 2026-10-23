use super::*;

#[test]
fn common_character_search_regressions() {
    for (filename, positive, negative) in [
        ("我同意 对不起啊 可能是我膨胀了.mp3", "kenengshi", "naishi"),
        ("可能是我膨胀了.mp3", "neng shi", "nai shi"),
        ("你怎么能.mp3", "neng", "nai"),
        ("喂能听到吗.wav", "nengting", "naiting"),
        ("那是不是个车.wav", "nashibushi", "naishi"),
        ("哪有豆.mp3", "nayou", "naiyou"),
        ("你来重庆机票钱酒店钱我出.wav", "chongqing", "tongqing"),
        ("找乐队.wav", "yuedui", "yaodui"),
        ("你不是真正的快乐.wav", "kuaile", "kuailuo"),
        ("QQ音乐.wav", "qqyinyue", "qqyinyao"),
        ("不行不行 不行！.mp3", "buxing", "buheng"),
        ("银行行走.wav", "yinhangxingzou", "yinhengxingzou"),
        ("重要的重量.wav", "zhongyao", "tongyao"),
        ("重新重复.wav", "chongxin", "tongxin"),
        ("长大以后.wav", "zhangda", "zhangta"),
        ("长江.wav", "changjiang", "zhangjang"),
        ("睡着了.wav", "shuizhao", "shuiluo"),
        ("看着.wav", "kanzhe", "kanluo"),
        ("呃，我不和长得丑的玩.wav", "zhangdechou", "zhangdechu"),
        ("比它长得多.wav", "changdeduo", "changdedu_"),
        ("你看看你长这个B样.wav", "zhangzhegeb", "zhangzhegec"),
        ("重开一下直播.mp3", "chongkai", "tongkai"),
        ("这还要我更多地去解释吗.wav", "gengduode", "gengduoti"),
        ("谁能告诉我一下.mp3", "shuineng", "sheineng"),
        ("谁能告诉我一下.mp3", "shuineng", "shuinai"),
        ("女绿吕略.wav", "nv", "nu"),
        ("奶是.wav", "naishi", "nengshi"),
        ("nai shi.wav", "nai shi", "naishi"),
        ("naishi.wav", "nai shi", "nashi"),
        ("otto过分了otto.wav", "otto guofen otto", "otto otto otto"),
        ("一个EQ.mp3", "yigeeq", "yige_eq"),
        ("😀鼓Kick_100%.wav", "鼓kick_100%", "鼓kick*101%"),
    ] {
        assert!(
            FilenameQuery::compile(positive).matches(filename),
            "{positive:?}: {filename}"
        );
        assert!(
            !FilenameQuery::compile(negative).matches(filename),
            "{negative:?}: {filename}"
        );
    }

    // These alternatives are intentionally independent of neighbouring words.
    // Expectations come from kHanyuPinlu, rather than phrase pronunciation.
    for (filename, queries) in [
        ("长得丑.wav", ["zhangdechou", "changdechou"]),
        ("比它长得多.wav", ["changdeduo", "zhangdeduo"]),
        ("重开直播.wav", ["chongkai", "zhongkai"]),
        ("重要.wav", ["zhongyao", "chongyao"]),
        ("重庆.wav", ["chongqing", "zhongqing"]),
        ("音乐.wav", ["yinyue", "yinle"]),
        ("乐队.wav", ["yuedui", "ledui"]),
        ("快乐.wav", ["kuaile", "kuaiyue"]),
        ("银行.wav", ["yinhang", "yinxing"]),
        ("不行.wav", ["buxing", "buhang"]),
        ("看着.wav", ["kanzhe", "kanzhao"]),
        ("更多地.wav", ["gengduode", "gengduodi"]),
    ] {
        for query in queries {
            assert!(
                FilenameQuery::compile(query).matches(filename),
                "{query}: {filename}"
            );
        }
    }

    for query in ["neng", "能"] {
        assert!(FilenameQuery::compile(query).matches("能.wav"));
    }
    for query in ["nai", "tai", "xiong"] {
        assert!(!FilenameQuery::compile(query).matches("能.wav"), "{query}");
    }
    // Neither character exists in the frequency table; Rust's default is used.
    assert!(FilenameQuery::compile("cao zhu").matches("艹丶.wav"));
    assert!(FilenameQuery::compile("艹 丶").matches("艹丶.wav"));
}

/// Explicit opt-in; never writes or migrates the user's library. No private
/// filenames/paths are stored in the repository or printed by this audit.
#[test]
#[ignore = "set OTTO_CORPUS_DB to a local index to run the read-only corpus audit"]
fn audit_real_corpus() {
    let path = std::env::var("OTTO_CORPUS_DB").expect("OTTO_CORPUS_DB required");
    let conn =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut stmt = conn
        .prepare("SELECT filename FROM files ORDER BY id")
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let started = Instant::now();
    for query in [
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
    ] {
        let query_started = Instant::now();
        let matcher = FilenameQuery::compile(query);
        let count = names.iter().filter(|name| matcher.matches(name)).count();
        println!("{query:?}: {count}, {:?}", query_started.elapsed());
        if query == "nai shi" && std::env::var_os("OTTO_CORPUS_SHOW_MATCHES").is_some() {
            for name in names.iter().filter(|name| matcher.matches(name)) {
                let source = cached_search_name(name).unwrap();
                let chars: Vec<_> = name.to_lowercase().chars().collect();
                let spans: Vec<_> = matcher
                    .terms
                    .iter()
                    .map(|term| {
                        term.spans(&source)
                            .iter()
                            .map(|span| chars[span.start..span.end].iter().collect::<String>())
                            .collect::<Vec<_>>()
                    })
                    .collect();
                println!("matched source fragments: {spans:?}");
            }
        }
    }
    let mut checks = 0;
    let mut reviewed_checks = 0;
    for name in &names {
        for (phrase, query) in [
            ("可能是", "kenengshi"),
            ("能听", "nengting"),
            ("重庆", "chongqing"),
            ("音乐", "yinyue"),
            ("乐队", "yuedui"),
            ("快乐", "kuaile"),
            ("不行", "buxing"),
            ("长得丑", "zhangdechou"),
            ("长这个", "zhangzhege"),
            ("重开", "chongkai"),
            ("更多地", "gengduode"),
            ("谁", "shui"),
        ] {
            if name.contains(phrase) {
                assert!(
                    FilenameQuery::compile(query).matches(name),
                    "reviewed phrase {phrase}"
                );
                reviewed_checks += 1;
            }
        }
        let lower = name.to_lowercase();
        let readings: Vec<String> = filename_pinyin::readings(&lower)
            .unwrap()
            .into_iter()
            .zip(lower.chars())
            .map(|(values, c)| values.into_iter().next().unwrap_or_else(|| c.to_string()))
            .collect();
        // Exercise start/middle/end and long-pattern paths with actual Unicode,
        // punctuation and English names, beyond curated pronunciation examples.
        for start in [0, readings.len() / 2, readings.len().saturating_sub(5)] {
            let query = readings[start..(start + 5).min(readings.len())].concat();
            assert!(
                FilenameQuery::compile(&query).matches(name),
                "source span {start}"
            );
            checks += 1;
        }
        assert!(FilenameQuery::compile(&readings.concat()).matches(name));
        checks += 1;
        if name.contains("可能是") {
            assert!(FilenameQuery::compile("kenengshi").matches(name));
            checks += 1;
        }
    }
    println!(
        "Audited {} filenames, {checks} span checks, elapsed {:?}",
        names.len(),
        started.elapsed()
    );
    println!("Human-reviewed pronunciation checks across real files: {reviewed_checks}");
    assert!(reviewed_checks > 0);
}
