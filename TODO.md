# TODO

## Now

- [ ] Manage references: delete (CLI `rm` + TUI key), confirmation prompts on destructive actions, dedup *merge* (not just trash), `.trash` restore/empty #feature
- [ ] Wire the FTS index into TUI search so filtering matches abstract, tags, journal, DOI, and PDF full text (currently indexed but unused) #improvement
- [ ] PMC OA PDF download is broken: the `ftp://…/oa_package/…` mirror now 404s over http(s) and the per-article PDF endpoint sits behind a JS interstitial — `add <PMC…>` can no longer fetch the PDF. Needs a new source or a browser-grade fetch #bug

## Next

- [ ] Split `tui.rs` into a `tui/` module tree (dedup, draw, events, enrich, popups, layout) #refactor
- [ ] Expand unit tests (storage dir-naming, export rendering, dedup grouping) + CI hardening (`cargo audit`, `--locked`) #chore
- [ ] `validate`: detect orphan PDFs on disk, structured (deserialize→edit→serialize) `--fix` #improvement
- [ ] `grimoire related` via Semantic Scholar API #feature
- [ ] Math-textbook retrieval: preserve LaTeX, headings, pages, and surrounding prose during extraction/chunking; combine semantic similarity with FTS for symbols, theorem names, and exact expressions; distinguish indexed passage counts from similarity matches #feature
- [ ] APA/Chicago formatted citation output #feature

## Later

- [ ] iCloud sync documentation and testing #docs
- [ ] Configurable browse keybindings #improvement
- [ ] Batch tag operations #improvement

## Scrapped

- fzf-based picker / `$GRIM_PICKER` — replaced by the native ratatui TUI, so external pickers are no longer configurable.

## Done

- [x] `grimoire backfill`: fill missing PDFs (Unpaywall repo-first → CrossRef) and abstracts for existing entries; `--pdfs-only`/`--abstracts-only`/`--check`, additive-only #feature
- [x] Wrap long titles in the browse list instead of truncating; drop the `>` selection arrow (background highlight suffices) #improvement
- [x] Import coverage: publisher landing pages (`citation_*` meta tags), PubMed/PMID, and DOI-embedded-in-URL #feature
- [x] Robustness: network timeouts, atomic `info.toml` writes, resilient `validate`, enrich race fix, editor-crash guard #bug
- [x] Duplicate detection: add-time DOI/title skip (`--force` to override) + union-find dedup grouping (`d`) #bug
- [x] `grimoire export`: yaml/json/bibtex/hayagriva, `--tag` filter, `-o` file output #feature
- [x] Batch `add` (multiple inputs), `--version`, `completions <shell>` #feature
- [x] TUI add/search modal coexistence: fallback prompts and Tab/Alt-A toggle #improvement
- [x] PMC import: resolve metadata and download a publisher or PMC Open Access PDF #feature
- [x] Tag browsing in the TUI (`t`) #feature
- [x] PDF full-text indexing into FTS5 on `reindex` and `add` #feature
- [x] URL import: `grimoire add <url>` downloads a PDF and imports #feature
- [x] Native TUI: nucleo + ratatui for browse and cite (replaced fzf) #feature
- [x] Theme system: external TOML themes, configurable via config.toml #feature
- [x] Helix integration: `grimoire cite` via `:insert-output` using `/dev/tty` #improvement
- [x] Ad-hoc codesign on `just install` to prevent macOS SIGKILL #chore
- [x] Project scaffold: `cargo init`, dependencies, module structure #chore
- [x] Config: library path resolution (`$GRIM_LIBRARY`, config.toml, `~/Papers`) #feature
- [x] Storage: create reference directories, copy PDFs, write `info.toml` #feature
- [x] Local PDF import with metadata extraction #feature
- [x] Metadata fetch: arXiv API lookup by ID, CrossRef lookup by DOI #feature
- [x] arXiv auto-detection: recognize arXiv IDs and URLs in `add` #feature
- [x] TUI actions: open PDF (`enter`), edit `info.toml` (`e`), copy BibTeX (`y`), open DOI/arXiv (`o`) #feature
- [x] SQLite FTS5 index: schema, indexing, `grimoire reindex` #feature
- [x] Shell completions: `grimoire completions {fish,bash,zsh}` #chore
