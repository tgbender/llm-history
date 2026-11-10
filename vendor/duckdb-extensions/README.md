These sources compile FTS and ICU into llm-history-rs, against the headers from
the pinned libduckdb-sys dependency. Cargo caches the compiled extensions across
ordinary Rust edits. Builds need no extension downloads or CMake installation.

- `icu/`: DuckDB v2.0.0 (`d54cbbe3df17e8e5ce68312f483b4155836112aa`),
  https://github.com/duckdb/duckdb/tree/v2.0.0/extension/icu
- `fts/`: `2266e67d01c737cb65114e77c7c672e58869a635`, matching the previously
  used DuckDB 2.0 FTS release,
  https://github.com/duckdb/duckdb-fts/tree/2266e67d01c737cb65114e77c7c672e58869a635

FTS includes the three compatibility patches shipped in DuckDB v2.0.0 under
`.github/patches/extensions/fts/`.

Upstream license files are retained in each directory. The Rust build script
renders the upstream SQL templates into C++ literals without their CMake step.

The generated tables also incorporate Unicode data:

- The ICU collation tables use CLDR 48 and Unicode Character Database 17.0.0.
  See the [CLDR notice](LICENSE-CLDR.txt) and the [Unicode notice](LICENSE-UNICODE.txt).
- The FTS character tables use Unicode Character Database 17.0.0 and carry
  the same [Unicode notice](LICENSE-UNICODE.txt).
- The ICU time zone tables use the Unicode ICU Data distribution of IANA
  tzdb 2026e and its Windows time zone mappings. See the
  [ICU Data notice](LICENSE-ICU-DATA.txt). The underlying IANA tzdb is in the
  public domain.
