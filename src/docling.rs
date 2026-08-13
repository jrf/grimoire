use std::collections::HashSet;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::{Captures, Regex};
use serde::Serialize;
use serde_json::Value;

const MAX_PASSAGE_CHARS: usize = 1_800;

#[derive(Debug, Serialize)]
pub struct ImportReport {
    pub passages: usize,
    pub body_blocks: usize,
    pub formulas: usize,
    pub pages: usize,
    pub skipped_furniture: usize,
    pub document_path: PathBuf,
    pub passages_path: PathBuf,
}

#[derive(Debug)]
struct Block {
    label: String,
    text: String,
    page: Option<u32>,
    level: Option<usize>,
}

#[derive(Debug, Serialize)]
struct Passage {
    chunk_index: usize,
    text: String,
    headings: Vec<String>,
    page_numbers: Vec<u32>,
    metadata: PassageMetadata,
}

#[derive(Debug, Serialize)]
struct PassageMetadata {
    source_format: &'static str,
    labels: Vec<String>,
}

#[derive(Debug, Default)]
struct PassageBuilder {
    text: String,
    headings: Vec<String>,
    pages: Vec<u32>,
    labels: Vec<String>,
}

impl PassageBuilder {
    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn append(&mut self, block: &Block) {
        if !self.text.is_empty() {
            self.text.push_str("\n\n");
        }
        match block.label.as_str() {
            "formula" => {
                self.text.push_str("\\[\n");
                self.text.push_str(&block.text);
                self.text.push_str("\n\\]");
            }
            "code" => {
                self.text.push_str("```\n");
                self.text.push_str(&block.text);
                self.text.push_str("\n```");
            }
            "list_item" => {
                self.text.push_str("- ");
                self.text.push_str(&block.text);
            }
            _ => self.text.push_str(&block.text),
        }
        if let Some(page) = block.page {
            self.pages.push(page);
        }
        self.labels.push(block.label.clone());
    }

    fn projected_chars(&self, block: &Block) -> usize {
        self.text.chars().count() + block.text.chars().count() + 8
    }

    fn finish(mut self, chunk_index: usize) -> Option<Passage> {
        let text = self.text.trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.pages.sort_unstable();
        self.pages.dedup();
        self.labels.sort();
        self.labels.dedup();
        Some(Passage {
            chunk_index,
            text,
            headings: self.headings,
            page_numbers: self.pages,
            metadata: PassageMetadata {
                source_format: "docling",
                labels: self.labels,
            },
        })
    }
}

pub fn import(reference_dir: &Path, source: &Path) -> Result<ImportReport> {
    anyhow::ensure!(
        source.is_file(),
        "Docling JSON not found: {}",
        source.display()
    );
    let bytes = std::fs::read(source)
        .with_context(|| format!("Failed to read Docling JSON from {}", source.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("Invalid Docling JSON in {}", source.display()))?;
    anyhow::ensure!(
        document.get("schema_name").and_then(Value::as_str) == Some("DoclingDocument"),
        "Expected a DoclingDocument JSON file"
    );

    let mut blocks = Vec::new();
    let mut seen = HashSet::new();
    let children = document
        .pointer("/body/children")
        .and_then(Value::as_array)
        .context("Docling document has no body children")?;
    for child in children {
        let reference = child
            .get("$ref")
            .and_then(Value::as_str)
            .context("Docling body child has no $ref")?;
        collect_blocks(&document, reference, &mut seen, &mut blocks)?;
    }

    let skipped_furniture = blocks
        .iter()
        .filter(|block| block.label == "__furniture__")
        .count();
    blocks.retain(|block| block.label != "__furniture__");
    let body_blocks = blocks
        .iter()
        .filter(|block| block.label != "section_header")
        .count();
    let formulas = blocks
        .iter()
        .filter(|block| block.label == "formula")
        .count();

    let passages = build_passages(&blocks);
    anyhow::ensure!(
        !passages.is_empty(),
        "Docling document has no indexable body text"
    );
    let pages = passages
        .iter()
        .flat_map(|passage| passage.page_numbers.iter().copied())
        .collect::<HashSet<_>>()
        .len();

    let output_dir = reference_dir.join("derived/docling");
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create {}", output_dir.display()))?;
    let document_path = output_dir.join("document.json");
    let passages_path = output_dir.join("passages.jsonl");
    atomic_write(&document_path, &bytes)?;
    atomic_write_jsonl(&passages_path, &passages)?;

    Ok(ImportReport {
        passages: passages.len(),
        body_blocks,
        formulas,
        pages,
        skipped_furniture,
        document_path,
        passages_path,
    })
}

fn collect_blocks(
    document: &Value,
    reference: &str,
    seen: &mut HashSet<String>,
    blocks: &mut Vec<Block>,
) -> Result<()> {
    if !seen.insert(reference.to_string()) {
        return Ok(());
    }
    // Docling pictures may contain one text child per plotted label, tick, or
    // glyph. Flattening those children into prose turns a diagram into dozens
    // of one-character paragraphs. Keep the preserved document as the source
    // for future media rendering, but do not index its internal OCR as text.
    if reference.starts_with("#/pictures/") {
        return Ok(());
    }
    let pointer = reference
        .strip_prefix('#')
        .context("Docling reference must start with #")?;
    let item = document
        .pointer(pointer)
        .with_context(|| format!("Docling reference does not resolve: {reference}"))?;

    if reference.starts_with("#/texts/") {
        let content_layer = item
            .get("content_layer")
            .and_then(Value::as_str)
            .unwrap_or("body");
        if content_layer != "body" {
            blocks.push(Block {
                label: "__furniture__".to_string(),
                text: String::new(),
                page: None,
                level: None,
            });
            return Ok(());
        }
        let label = item.get("label").and_then(Value::as_str).unwrap_or("text");
        if matches!(label, "page_header" | "page_footer") {
            return Ok(());
        }
        let text = item
            .get("text")
            .or_else(|| item.get("orig"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if !text.is_empty() {
            let text = if label == "formula" {
                text.to_string()
            } else {
                normalize_inline_math(text)
            };
            let page = item
                .pointer("/prov/0/page_no")
                .and_then(Value::as_u64)
                .and_then(|page| u32::try_from(page).ok());
            let level = item
                .get("level")
                .and_then(Value::as_u64)
                .and_then(|level| usize::try_from(level).ok());
            blocks.push(Block {
                label: label.to_string(),
                text,
                page,
                level,
            });
        }
        return Ok(());
    }

    if let Some(children) = item.get("children").and_then(Value::as_array) {
        for child in children {
            if let Some(reference) = child.get("$ref").and_then(Value::as_str) {
                collect_blocks(document, reference, seen, blocks)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn normalize_inline_math(text: &str) -> String {
    static ABSOLUTE_VALUE: OnceLock<Regex> = OnceLock::new();
    static OPEN_DELIMITER: OnceLock<Regex> = OnceLock::new();
    static CLOSE_DELIMITER: OnceLock<Regex> = OnceLock::new();
    static SPACED_PUNCTUATION: OnceLock<Regex> = OnceLock::new();
    static SPACED_COMMA: OnceLock<Regex> = OnceLock::new();
    static NUMBER_SET: OnceLock<Regex> = OnceLock::new();

    let text = normalize_indexed_symbols(text);
    let absolute_value = ABSOLUTE_VALUE.get_or_init(|| {
        Regex::new(r"\|\s*([^|\n]+?)\s*\|").expect("absolute value regex is valid")
    });
    let open_delimiter = OPEN_DELIMITER
        .get_or_init(|| Regex::new(r"([\(\[\{])\s+").expect("opening delimiter regex is valid"));
    let close_delimiter = CLOSE_DELIMITER
        .get_or_init(|| Regex::new(r"\s+([\)\]\}])").expect("closing delimiter regex is valid"));
    let spaced_punctuation = SPACED_PUNCTUATION
        .get_or_init(|| Regex::new(r"\s+([,.;:])").expect("spaced punctuation regex is valid"));
    let spaced_comma = SPACED_COMMA
        .get_or_init(|| Regex::new(r",\s*([[:alpha:]\-+])").expect("comma spacing regex is valid"));
    let number_set =
        NUMBER_SET.get_or_init(|| Regex::new(r"\b([NQRZ])\b").expect("number set regex is valid"));

    let text = absolute_value.replace_all(&text, |captures: &Captures<'_>| {
        format!("|{}|", captures[1].trim())
    });
    let text = open_delimiter.replace_all(&text, "$1");
    let text = close_delimiter.replace_all(&text, "$1");
    let text = spaced_punctuation.replace_all(&text, "$1");
    let text = spaced_comma.replace_all(&text, ", $1");
    number_set
        .replace_all(&text, |captures: &Captures<'_>| match &captures[1] {
            "N" => "ℕ",
            "Q" => "ℚ",
            "R" => "ℝ",
            "Z" => "ℤ",
            _ => unreachable!("number-set regex limits captures"),
        })
        .into_owned()
}

fn normalize_indexed_symbols(text: &str) -> String {
    static INDEXED_SYMBOL: OnceLock<Regex> = OnceLock::new();
    let indexed_symbol = INDEXED_SYMBOL.get_or_init(|| {
        Regex::new(r"\b[A-Za-z](?:\s+(?:[0-9]+|[aehijklmnoprstuvx])\b)+(?:\s*-\s*[0-9]+)?")
            .expect("indexed symbol regex is valid")
    });

    let mut normalized = String::with_capacity(text.len());
    let mut end = 0;
    for found in indexed_symbol.find_iter(text) {
        normalized.push_str(&text[end..found.start()]);
        let candidate = found.as_str();
        let compact = candidate
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>();
        let mut chars = compact.chars();
        let base = chars.next().expect("indexed symbol always has a base");
        let suffix = chars.collect::<String>();
        let roman_enumeration = matches!(base, 'i' | 'v' | 'x')
            && suffix.chars().all(|ch| matches!(ch, 'i' | 'v' | 'x'))
            && surrounded_by_parentheses(text, found.start(), found.end());
        let ambiguous_difference = suffix
            .split_once('-')
            .is_some_and(|(before, _)| before.chars().all(|ch| ch.is_ascii_digit()));

        if roman_enumeration || ambiguous_difference {
            normalized.push_str(candidate);
        } else if let Some(suffix) = subscript_token(&suffix) {
            normalized.push(base);
            normalized.push_str(&suffix);
        } else {
            normalized.push_str(candidate);
        }
        end = found.end();
    }
    normalized.push_str(&text[end..]);
    normalized
}

fn surrounded_by_parentheses(text: &str, start: usize, end: usize) -> bool {
    text[..start].trim_end().ends_with('(') && text[end..].trim_start().starts_with(')')
}

fn subscript_token(token: &str) -> Option<String> {
    token.chars().map(subscript_char).collect()
}

fn subscript_char(character: char) -> Option<char> {
    match character {
        '0' => Some('₀'),
        '1' => Some('₁'),
        '2' => Some('₂'),
        '3' => Some('₃'),
        '4' => Some('₄'),
        '5' => Some('₅'),
        '6' => Some('₆'),
        '7' => Some('₇'),
        '8' => Some('₈'),
        '9' => Some('₉'),
        'a' => Some('ₐ'),
        'e' => Some('ₑ'),
        'h' => Some('ₕ'),
        'i' => Some('ᵢ'),
        'j' => Some('ⱼ'),
        'k' => Some('ₖ'),
        'l' => Some('ₗ'),
        'm' => Some('ₘ'),
        'n' => Some('ₙ'),
        'o' => Some('ₒ'),
        'p' => Some('ₚ'),
        'r' => Some('ᵣ'),
        's' => Some('ₛ'),
        't' => Some('ₜ'),
        'u' => Some('ᵤ'),
        'v' => Some('ᵥ'),
        'x' => Some('ₓ'),
        '-' => Some('₋'),
        _ => None,
    }
}

fn build_passages(blocks: &[Block]) -> Vec<Passage> {
    let mut passages = Vec::new();
    let mut headings = Vec::new();
    let mut current = PassageBuilder::default();

    for block in blocks {
        if block.label == "section_header" {
            flush(&mut current, &mut passages);
            let level = block.level.unwrap_or(1).max(1);
            headings.truncate(level.saturating_sub(1).min(headings.len()));
            headings.push(block.text.clone());
            current.headings = headings.clone();
            continue;
        }

        if !current.is_empty()
            && block.label != "formula"
            && current.projected_chars(block) > MAX_PASSAGE_CHARS
        {
            flush(&mut current, &mut passages);
            current.headings = headings.clone();
        }
        if current.headings.is_empty() {
            current.headings = headings.clone();
        }
        current.append(block);
    }
    flush(&mut current, &mut passages);
    passages
}

fn flush(current: &mut PassageBuilder, passages: &mut Vec<Passage>) {
    let builder = std::mem::take(current);
    if let Some(passage) = builder.finish(passages.len()) {
        passages.push(passage);
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().context("Derived file has no parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(contents)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn atomic_write_jsonl(path: &Path, passages: &[Passage]) -> Result<()> {
    let parent = path.parent().context("Passage file has no parent")?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(temporary.as_file());
        for passage in passages {
            serde_json::to_writer(&mut writer, passage)?;
            writer.write_all(b"\n")?;
        }
        writer.flush()?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{import, normalize_inline_math};
    use serde_json::json;

    #[test]
    fn imports_structured_passages_and_skips_furniture() {
        let root = tempfile::tempdir().unwrap();
        let reference = root.path().join("synthetic-book");
        std::fs::create_dir_all(&reference).unwrap();
        let source = root.path().join("synthetic-docling.json");
        let mut document = json!({
            "schema_name": "DoclingDocument",
            "version": "1.10.0",
            "body": {"children": [
                {"$ref": "#/texts/0"},
                {"$ref": "#/groups/0"},
                {"$ref": "#/pictures/0"},
                {"$ref": "#/texts/4"}
            ]},
            "groups": [{"children": [
                {"$ref": "#/texts/1"},
                {"$ref": "#/texts/2"},
                {"$ref": "#/texts/3"}
            ]}],
            "texts": [
                {"content_layer": "furniture", "label": "page_header", "text": "Synthetic header"},
                {"content_layer": "body", "label": "section_header", "level": 1, "text": "1 Sequences", "prov": [{"page_no": 3}]},
                {"content_layer": "body", "label": "text", "text": "A sequence converges when...", "prov": [{"page_no": 3}]},
                {"content_layer": "body", "label": "formula", "text": "a_n \\to L", "prov": [{"page_no": 4}]},
                {"content_layer": "body", "label": "text", "text": "This is the conclusion.", "prov": [{"page_no": 4}]}
            ],
            "pictures": [{
                "self_ref": "#/pictures/0",
                "label": "picture",
                "children": [{"$ref": "#/texts/5"}],
                "prov": [{"page_no": 4}]
            }]
        });
        document["texts"].as_array_mut().unwrap().push(json!({
            "content_layer": "body",
            "label": "text",
            "text": "M",
            "parent": {"$ref": "#/pictures/0"},
            "prov": [{"page_no": 4}]
        }));
        std::fs::write(&source, serde_json::to_vec(&document).unwrap()).unwrap();

        let report = import(&reference, &source).unwrap();
        assert_eq!(report.passages, 1);
        assert_eq!(report.body_blocks, 3);
        assert_eq!(report.formulas, 1);
        assert_eq!(report.pages, 2);
        assert_eq!(report.skipped_furniture, 1);

        let output = std::fs::read_to_string(report.passages_path).unwrap();
        let passage: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(passage["headings"], json!(["1 Sequences"]));
        assert_eq!(passage["page_numbers"], json!([3, 4]));
        assert!(
            passage["text"]
                .as_str()
                .unwrap()
                .contains("\\[\na_n \\to L\n\\]")
        );
        assert!(!passage["text"].as_str().unwrap().contains("\n\nM\n\n"));
    }

    #[test]
    fn normalizes_common_docling_inline_math_without_rewriting_prose() {
        assert_eq!(
            normalize_inline_math(
                "If ( a n ) lies in [ c, d ], then ( a n k ) converges; keep ( f g ) and ( i i )."
            ),
            "If (aₙ) lies in [c, d], then (aₙₖ) converges; keep (f g) and (i i)."
        );
    }

    #[test]
    fn normalizes_inline_indices_absolute_values_sets_and_intervals() {
        assert_eq!(
            normalize_inline_math(
                "Proof. Let ( a n ) satisfy | a n | ≤ M for n ∈ N . Use [ -M,M ], I 1 , a n 1 ∈ I 1 , and I k -1."
            ),
            "Proof. Let (aₙ) satisfy |aₙ| ≤ M for n ∈ ℕ. Use [-M, M], I₁, aₙ₁ ∈ I₁, and Iₖ₋₁."
        );
    }

    #[test]
    fn normalizes_bolzano_weierstrass_proof_inline_math() {
        assert_eq!(
            normalize_inline_math(
                "Proof. Let ( a n ) be bounded with | a n | ≤ M for all n ∈ N . Bisect [ -M,M ] into [ -M, 0] and [0 , M ]. Label the interval I 1 . Then a n 1 ∈ I 1 ."
            ),
            "Proof. Let (aₙ) be bounded with |aₙ| ≤ M for all n ∈ ℕ. Bisect [-M, M] into [-M, 0] and [0, M]. Label the interval I₁. Then aₙ₁ ∈ I₁."
        );
    }

    #[test]
    fn leaves_ambiguous_powers_and_enumerations_alone() {
        assert_eq!(
            normalize_inline_math("Keep ( i v ), 1-1, and x 2 -1 unchanged."),
            "Keep (i v), 1-1, and x 2 -1 unchanged."
        );
    }
}
