# Grimoire

A fast TUI reference manager.

![Grimoire paper browser](grimoire.png)

## Install

Requires Rust.

```
cargo install --path .
```

Or with just:

```
just install
```

## Development

Run the same formatting, compilation, lint, and test checks used by CI:

```sh
just check-all
```

## Usage

```
grimoire                          # browse library
grimoire jepa                     # browse with "jepa" pre-filled
grimoire add 1706.03762           # import by arXiv ID (fetches metadata + PDF)
grimoire add 10.1038/nature14539  # import by DOI (fetches metadata)
grimoire add paper.pdf            # import local PDF
grimoire add 1706.03762 2201.1234 # batch import (each input handled independently)
grimoire add --force 1706.03762   # import even if a matching DOI/title already exists
grimoire cite --format typst      # pick a reference, output @cite-key
grimoire export --format yaml     # dump all references to stdout as YAML
grimoire export --format hayagriva # ...or json / bibtex / hayagriva (Typst)
grimoire export -f bibtex --tag video -o refs.bib  # filter by tag, write to a file
grimoire backfill                 # fetch missing PDFs + abstracts for existing entries
grimoire backfill --check         # report how many entries are missing a PDF / abstract
grimoire backfill --pdfs-only     # only download missing PDFs (open-access)
grimoire reindex                  # rebuild search index from filesystem
grimoire validate                 # check library integrity
grimoire validate --fix           # auto-fix issues (rename temp files, remove junk)
grimoire completions fish         # emit a shell completion script
```

### TUI keybindings

| Key | Action |
|-----|--------|
| `j / k` | Move down / up |
| `g / G` | Jump to top / bottom |
| `/ or i` | Enter search mode |
| `enter` | Open PDF (browse), confirm search (search) |
| `e` | Edit info.toml |
| `y` | Copy BibTeX |
| `o` | Open DOI / arXiv in browser |
| `a` | Add paper (path, DOI, arXiv ID, URL) |
| `r` | Enrich selected (fetch metadata) |
| `R` | Enrich all with missing fields |
| `s` | Cycle sort (name/author/year/title) |
| `d` | Deduplicate library |
| `I` | Reindex library |
| `V` | Validate library (auto-fix) |
| `t` | Browse tags |
| `T` | Switch theme |
| `space` | Toggle full-screen abstract (Quick Look) |
| `L` | Cycle layout (full/wide/tall) |
| `?` | Help |
| `q` | Quit |

## Library layout

```
~/Papers/
  vaswani-2017-attention/
    info.toml
    vaswani-2017-attention.pdf
  lecun-2015-deep/
    info.toml
```

Directory naming: `{first-author}-{year}-{first-title-word}`.

### info.toml

```toml
title = "Attention Is All You Need"
authors = ["Ashish Vaswani", "Noam Shazeer", "Niki Parmar"]
year = 2017
arxiv = "1706.03762"
tags = ["transformers", "nlp"]
files = ["vaswani-2017-attention.pdf"]
abstract = """
The dominant sequence transduction models are based on complex recurrent or
convolutional neural networks...
"""
```

## Configuration

Optional. Grimoire works without any config file.

`~/.config/grimoire/config.toml`:

```toml
library = "~/Papers"       # default
editor = ["hx"]             # string or command plus arguments; defaults to $EDITOR or "vi"
reader = ["open"]           # PDF opener; defaults to $GRIM_READER or the OS opener
browser = ["open"]          # URL opener; defaults to $BROWSER or the OS opener
theme = "tokyo-night-moon" # default
layout = "full"            # full (default), wide, tall, or auto; auto detects wide/tall
```

Command values accept either a string (`reader = "zathura"`) or an argument array
(`reader = ["open", "-a", "Preview"]`). Grimoire appends the file path or URL.

Environment variables: `$GRIM_LIBRARY`, `$GRIM_READER`, `$GRIM_EDITOR` (or
`$EDITOR`), `$GRIM_BROWSER` (or `$BROWSER`). The `GRIM_`-prefixed names take
precedence.

## Helix integration

Add to `~/.config/helix/config.toml`:

```toml
[keys.normal.space.r]
r = [":insert-output grimoire cite", ":redraw"]
t = [":insert-output grimoire cite --format typst", ":redraw"]
l = [":insert-output grimoire cite --format latex", ":redraw"]
```

`Space r t` in normal mode opens Grimoire and inserts a Typst citation at the cursor.

## Smart import

`grimoire add` detects the input type automatically:

- **arXiv ID** (`1706.03762`) — fetches metadata from arXiv API, downloads PDF
- **arXiv URL** (`https://arxiv.org/abs/1706.03762`) — same
- **PMC URL** (`https://pmc.ncbi.nlm.nih.gov/articles/PMC1234567/`) — resolves metadata and downloads an available publisher or PMC Open Access PDF
- **PubMed URL / PMID** (`https://pubmed.ncbi.nlm.nih.gov/26017442/`, `PMID:26017442`) — resolves the DOI via NCBI, then fetches metadata from CrossRef
- **DOI** (`10.1038/nature14539`) — fetches metadata from CrossRef; if no PDF is otherwise available, tries Unpaywall for an open-access copy
- **DOI or doi.org URL** — a DOI embedded anywhere in a URL (e.g. a PLoS `?id=10.1371/…` link) is extracted automatically
- **Publisher landing page** (`https://www.nature.com/articles/…`, ScienceDirect, Springer, etc.) — scrapes the page's `citation_doi` / `citation_pdf_url` meta tags to resolve metadata and, when the PDF is openly available, download it
- **Direct PDF URL** — downloads only when the response contains PDF data
- **Local PDF** (`paper.pdf`) — extracts metadata from PDF; if filename looks like an arXiv ID, fetches metadata from arXiv

> **Prefer the DOI for publisher pages.** Some sites can't be imported from
> their article URL: JavaScript-rendered pages (e.g. IEEE Xplore) expose no DOI
> or PDF link in the HTML we fetch, and bot-protected pages (e.g. MDPI behind
> Cloudflare) refuse the request outright. No tool that doesn't run a full
> browser can scrape these. Add them by **DOI** instead — metadata always
> resolves via CrossRef, and an open-access PDF is fetched via Unpaywall when a
> reachable copy exists.

If an incoming reference matches an existing entry by DOI or normalized title,
`add` warns and skips it; pass `--force` to import anyway. The interactive
deduplicator (`d` in the TUI) groups existing references that share a title
**or** DOI so you can merge them after the fact.

## Backfill

`grimoire backfill` fills gaps in entries you already have — it never touches an
entry that already has the thing, so it is safe to run (and re-run) anytime:

- **Missing PDFs** — for entries with a DOI but no PDF, it looks up an
  open-access copy via Unpaywall (preferring repository copies, which rarely
  block automated downloads) and falls back to any PDF link CrossRef lists.
- **Missing abstracts** — fetches metadata (arXiv / CrossRef / title search) and
  fills the abstract and any other empty fields, additively.

```
grimoire backfill --check       # preview: how many entries are missing what
grimoire backfill               # fetch missing PDFs and abstracts
grimoire backfill --pdfs-only   # or --abstracts-only
```

Only **open-access** PDFs are retrievable. Paywalled papers, and publishers that
block non-browser requests (Wiley, MDPI/Cloudflare, some society journals),
won't yield a PDF — expect to recover roughly a quarter of a paywall-heavy
library. Because backfill is idempotent, running it again later picks up entries
that failed on a transient network error or that become open-access over time.
