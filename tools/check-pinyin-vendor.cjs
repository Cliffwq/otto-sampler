// Verify pinned frequency data offline; never depend on developer-local files.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
const expected = {
  'kHanyuPinlu.txt': '260ef08aef066ae5e798ed9933e305094d03a87a5d0bcbced6d740cb9084f2f8',
  'LICENSE': '9c048697be2502a16e8bcb282d5d465a07295b2def0ffb05a269c5d39dbe1586',
};
for (const [file, hash] of Object.entries(expected)) {
  const bytes = fs.readFileSync(path.join(root, 'src-tauri/vendor/pinyin-data', file));
  assert.equal(crypto.createHash('sha256').update(bytes).digest('hex'), hash, `${file} differs from the pinned source`);
}
const cargo = fs.readFileSync(path.join(root, 'src-tauri/Cargo.toml'), 'utf8');
assert.match(cargo, /pinyin\s*=\s*\{\s*version\s*=\s*"=0\.11\.0",\s*default-features\s*=\s*false,\s*features\s*=\s*\["plain"\]\s*\}/);
console.log('Pinned frequency dictionary and license verified; rust-pinyin 0.11.0 uses only its default reading.');
