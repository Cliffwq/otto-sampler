# Character pronunciation data

`kHanyuPinlu.txt` and `LICENSE` are unchanged upstream files from
[mozillazg/pinyin-data](https://github.com/mozillazg/pinyin-data), commit
`9e5e9bdfe8add694300e0cc93a27b803b68377f9` (MIT).

The source is the Unihan kHanyuPinlu field, derived from the Modern Chinese
Frequency Dictionary. OTTO Sampler retains every listed reading, removes tones,
normalizes ü to v, and deduplicates equal spellings. This source has absolute
precedence. Characters absent from it use only the default single reading from
rust-pinyin 0.11.0. There are no phrase rules or individual-character overrides.

SHA-256:

- kHanyuPinlu.txt: `260ef08aef066ae5e798ed9933e305094d03a87a5d0bcbced6d740cb9084f2f8`
- LICENSE: `9c048697be2502a16e8bcb282d5d465a07295b2def0ffb05a269c5d39dbe1586`

Run `node tools/check-pinyin-vendor.cjs` to verify the checked-in copies without
network access. On an upstream update, pin a new commit and file hashes, review
the policy invariants, increment PINYIN_INDEX_VERSION, and run search tests.
