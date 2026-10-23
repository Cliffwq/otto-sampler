//! Offline single-character readings. Frequency data takes absolute precedence;
//! characters absent from it use only rust-pinyin's default reading.
use pinyin::ToPinyin;
use std::collections::HashMap;
use std::sync::OnceLock;

#[cfg(test)]
thread_local! { static CALL_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn thread_call_count() -> usize {
    CALL_COUNT.with(|count| count.get())
}

const FREQUENCY_DATA: &str = include_str!("../vendor/pinyin-data/kHanyuPinlu.txt");
// Checked against every entry in the pinned source after tone deduplication.
pub(crate) const MAX_READINGS: usize = 4;

fn untoned(reading: &str) -> String {
    reading
        .chars()
        .filter(|c| !matches!(c, '\u{0300}'..='\u{036f}'))
        .map(|c| match c {
            'ā' | 'á' | 'ǎ' | 'à' => 'a',
            'ē' | 'é' | 'ě' | 'è' | 'ê' | 'ế' | 'ề' => 'e',
            'ī' | 'í' | 'ǐ' | 'ì' => 'i',
            'ō' | 'ó' | 'ǒ' | 'ò' => 'o',
            'ū' | 'ú' | 'ǔ' | 'ù' => 'u',
            'ü' | 'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => 'v',
            'ń' | 'ň' | 'ǹ' => 'n',
            'ḿ' => 'm',
            _ => c,
        })
        .collect()
}

fn parse_frequency_data() -> Result<HashMap<char, Vec<String>>, String> {
    let mut entries = HashMap::new();
    for (line_number, line) in FREQUENCY_DATA.lines().enumerate() {
        let content = line.split('#').next().unwrap_or("").trim();
        if content.is_empty() {
            continue;
        }
        let invalid = || {
            format!(
                "invalid embedded frequency dictionary line {}",
                line_number + 1
            )
        };
        let (code, values) = content.split_once(':').ok_or_else(invalid)?;
        let code = code.strip_prefix("U+").ok_or_else(invalid)?;
        let scalar = u32::from_str_radix(code, 16).map_err(|_| invalid())?;
        let character = char::from_u32(scalar).ok_or_else(invalid)?;
        let mut readings = Vec::new();
        for value in values.split(',') {
            let value = untoned(value.trim());
            if value.is_empty() || !value.bytes().all(|c| c.is_ascii_lowercase()) {
                return Err(invalid());
            }
            if !readings.contains(&value) {
                readings.push(value);
            }
        }
        if entries.insert(character, readings).is_some() {
            return Err(invalid());
        }
    }
    Ok(entries)
}

/// One alternatives vector per Unicode scalar, aligned with the input. No
/// phrase matching, cross-character combinations, or character-specific rules.
pub(crate) fn readings(text: &str) -> Result<Vec<Vec<String>>, String> {
    #[cfg(test)]
    if !text.is_ascii() {
        CALL_COUNT.with(|count| count.set(count.get() + 1));
    }
    static FREQUENCY: OnceLock<Result<HashMap<char, Vec<String>>, String>> = OnceLock::new();
    let frequency = FREQUENCY
        .get_or_init(parse_frequency_data)
        .as_ref()
        .map_err(Clone::clone)?;
    Ok(text
        .chars()
        .map(|character| {
            frequency.get(&character).cloned().unwrap_or_else(|| {
                vec![character
                    .to_pinyin()
                    .map(|value| untoned(value.plain()))
                    .unwrap_or_else(|| character.to_string())]
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(text: &str) -> Vec<Vec<String>> {
        readings(text).unwrap()
    }

    #[test]
    fn frequency_readings_have_absolute_precedence() {
        assert_eq!(
            values("能长重谁"),
            vec![
                vec!["neng"],
                vec!["zhang", "chang"],
                vec!["zhong", "chong"],
                vec!["shui"]
            ]
        );
        assert_eq!(values("嗯"), vec![vec!["n", "ng"]]);
        assert_eq!(values("女绿略"), vec![vec!["nv"], vec!["lv"], vec!["lve"]]);
    }

    #[test]
    fn missing_characters_use_only_the_default_rust_reading() {
        let dictionary = parse_frequency_data().unwrap();
        for (character, expected) in [('艹', "cao"), ('丶', "zhu")] {
            assert!(!dictionary.contains_key(&character));
            assert_eq!(values(&character.to_string()), vec![vec![expected]]);
        }
        assert_eq!(values("😀\u{301}"), vec![vec!["😀"], vec!["\u{301}"]]);
    }

    #[test]
    fn readings_depend_only_on_the_character() {
        let text = "长得丑重开直播重庆银行音乐女绿吕略艹丶𠮷😀e\u{301}";
        let expected = text
            .chars()
            .map(|character| values(&character.to_string()).remove(0))
            .collect::<Vec<_>>();
        assert_eq!(values(text), expected);
        assert_eq!(values(text).len(), text.chars().count());
    }

    #[test]
    fn complete_embedded_dictionary_is_valid_and_deduplicated() {
        let dictionary = parse_frequency_data().unwrap();
        assert_eq!(dictionary.len(), 3799);
        assert_eq!(dictionary.values().map(Vec::len).max(), Some(MAX_READINGS));
        for (character, alternatives) in &dictionary {
            assert!(!alternatives.is_empty());
            let unique = alternatives
                .iter()
                .collect::<std::collections::HashSet<_>>();
            assert_eq!(unique.len(), alternatives.len());
            assert!(alternatives
                .iter()
                .all(|value| value.len() <= 8 && value.bytes().all(|c| c.is_ascii_lowercase())));
            assert_eq!(values(&character.to_string())[0], *alternatives);
        }
    }

    #[test]
    fn fallback_dictionary_readings_have_valid_search_encoding() {
        for character in (0..=0x10ffff).filter_map(char::from_u32) {
            if let Some(value) = character.to_pinyin() {
                let value = untoned(value.plain());
                assert!(
                    !value.is_empty()
                        && value.len() <= 8
                        && value.bytes().all(|c| c.is_ascii_lowercase()),
                    "invalid fallback spelling for {character}: {value}"
                );
            }
        }
    }
}
