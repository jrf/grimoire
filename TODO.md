# TODO

## Now

- [ ] `carina related` via Semantic Scholar API #feature

## Next

- [ ] APA/Chicago formatted citation output #feature
- [ ] Batch operations: `carina tag --all`, `carina bib --all` #improvement

## Later

- [ ] iCloud sync documentation and testing #docs
- [ ] Configurable browse keybindings #improvement

## Scrapped

## Done

- [x] Duplicate detection: add-time DOI/title skip (`--force` to override) + union-find dedup grouping #bug
- [x] `grimoire export` command: yaml/json/bibtex/hayagriva, `--tag` filter, `-o` file output #feature
- [x] Batch `add` (multiple inputs), `--version`, `completions <shell>` subcommand #feature
- [x] TUI Refined Modal Coexistence: search fallback prompts and seamless Tab/Alt-A toggle between Search and Add modes #improvement
- [x] `carina tag` command: `tag list`, `tag add`, `tag rm` #feature
- [x] PDF full-text indexing into FTS5 on `reindex` and `add` #feature
- [x] URL import: `carina add <url>` downloads PDF and imports #feature
- [x] Native TUI: replaced fzf with nucleo + ratatui for browse and cite #feature
- [x] Theme system: 6 built-in themes, custom TOML themes, configurable via config.toml #feature
- [x] Helix integration: `carina cite` works via `:insert-output` using `/dev/tty` #improvement
- [x] Duplicate detection: `carina duplicates` and interactive `carina dedup` #feature
- [x] Ad-hoc codesign on `just install` to prevent macOS SIGKILL #chore
- [x] Project scaffold: `cargo init`, dependencies, module structure #chore
- [x] Config: library path resolution (`$CARINA_LIBRARY`, config.toml, `~/Papers`) #feature
- [x] Storage: create reference directories, copy PDFs, write `info.toml` #feature
- [x] Import: `carina add <file>` with PDF metadata extraction #feature
- [x] Metadata fetch: arXiv API lookup by arXiv ID, CrossRef lookup by DOI #feature
- [x] arXiv auto-detection: recognize arXiv IDs and URLs in `carina add` #feature
- [x] List/filter: `carina list`, filter by tag #feature
- [x] Open: `carina open` with `$CARINA_READER` or `open` fallback, `--reader` flag #feature
- [x] Edit: `carina edit` opens `info.toml` in `$EDITOR` #feature
- [x] BibTeX export: `carina bib <query>` #feature
- [x] SQLite FTS5 index: schema, indexing, `carina reindex` #feature
- [x] Picker-agnostic: configurable via `$CARINA_PICKER` or config.toml #improvement
- [x] Shell completions: `carina completions {fish,bash,zsh}` #chore
