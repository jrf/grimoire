use anyhow::{Context, Result};
use regex::Regex;

use crate::model::Reference;

fn clean_abstract(s: &str) -> String {
    let tag_re = Regex::new(r"<[^>]+>").unwrap();
    let s = tag_re.replace_all(s, "");
    let s = Regex::new(r"(?i)^\s*abstract[.:]\s*")
        .unwrap()
        .replace(&s, "");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn detect_arxiv_id(input: &str) -> Option<String> {
    let re = Regex::new(
        r"^(?:(?:https?://)?(?:www\.)?arxiv\.org/(?:abs|pdf)/)?(\d{4}\.\d{4,5})(v\d+)?(?:\.pdf)?$",
    )
    .unwrap();
    re.captures(input).map(|c| {
        let id = c.get(1).unwrap().as_str();
        match c.get(2) {
            Some(v) => format!("{}{}", id, v.as_str()),
            None => id.to_string(),
        }
    })
}

pub fn detect_doi(input: &str) -> Option<String> {
    let re = Regex::new(r"(10\.\d{4,9}/[^\s]+)").unwrap();
    re.captures(input)
        .map(|c| c.get(1).unwrap().as_str().to_string())
}

pub fn fetch_arxiv(arxiv_id: &str) -> Result<Reference> {
    let id_clean = arxiv_id.trim_end_matches(".pdf");
    let url = format!("https://export.arxiv.org/api/query?id_list={}", id_clean);
    let body = reqwest::blocking::get(&url)
        .context("Failed to reach arXiv API")?
        .text()?;

    parse_arxiv_response(&body, id_clean)
}

pub fn download_arxiv_pdf(arxiv_id: &str, dest: &std::path::Path) -> Result<()> {
    let id_clean = arxiv_id.trim_end_matches(".pdf");
    let url = format!("https://arxiv.org/pdf/{}.pdf", id_clean);
    let bytes = reqwest::blocking::get(&url)
        .context("Failed to download PDF from arXiv")?
        .bytes()?;

    std::fs::write(dest, &bytes)?;
    Ok(())
}

pub fn search_crossref_by_title(title: &str) -> Result<Reference> {
    let url = format!(
        "https://api.crossref.org/works?query.title={}&rows=1",
        urlencoding::encode(title)
    );
    let body = reqwest::blocking::Client::new()
        .get(&url)
        .header(
            "User-Agent",
            "Grimoire/0.1 (reference manager; mailto:jrfetzer@gmail.com)",
        )
        .send()
        .context("Failed to reach CrossRef API")?
        .text()?;

    let v: serde_json::Value = serde_json::from_str(&body).context("Invalid CrossRef JSON")?;
    let items = v["message"]["items"].as_array().context("No results")?;
    let item = items.first().context("No results")?;

    let result_title = item["title"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|t| t.as_str())
        .unwrap_or("");

    if !titles_match(title, result_title) {
        anyhow::bail!("No matching result");
    }

    let doi = item["DOI"].as_str().context("No DOI in result")?;
    fetch_crossref(doi)
}

fn titles_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| -> String {
        s.to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let na = norm(a);
    let nb = norm(b);
    if na.is_empty() || nb.is_empty() {
        return false;
    }
    na == nb || na.starts_with(&nb) || nb.starts_with(&na)
}

pub fn fetch_crossref(doi: &str) -> Result<Reference> {
    let url = format!("https://api.crossref.org/works/{}", doi);
    let body = reqwest::blocking::Client::new()
        .get(&url)
        .header(
            "User-Agent",
            "Grimoire/0.1 (reference manager; mailto:jrfetzer@gmail.com)",
        )
        .send()
        .context("Failed to reach CrossRef API")?
        .text()?;

    parse_crossref_response(&body)
}

fn parse_arxiv_response(xml: &str, arxiv_id: &str) -> Result<Reference> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();

    let mut title = None;
    let mut authors = Vec::new();
    let mut abstract_text = None;
    let mut published = None;
    let mut doi = None;

    let mut in_entry = false;
    let mut current_tag = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "entry" => in_entry = true,
                    "title" | "summary" | "published" | "name" if in_entry => {
                        current_tag = name;
                    }
                    "arxiv:doi" if in_entry => {
                        current_tag = "doi".to_string();
                    }
                    _ => current_tag.clear(),
                }
            }
            Ok(Event::Text(e)) if in_entry => {
                let text = e.unescape().unwrap_or_default().trim().to_string();
                if text.is_empty() {
                    continue;
                }
                match current_tag.as_str() {
                    "title" => title = Some(text),
                    "name" => authors.push(text),
                    "summary" => abstract_text = Some(text),
                    "published" => published = Some(text),
                    "doi" => doi = Some(text),
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "entry" {
                    break;
                }
                current_tag.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("Failed to parse arXiv XML: {}", e),
            _ => {}
        }
        buf.clear();
    }

    let year = published
        .as_deref()
        .and_then(|p| p.get(..4)?.parse::<u16>().ok());

    let title = title.context("No title found in arXiv response")?;
    let title = title
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let abstract_text = abstract_text.map(|a| clean_abstract(&a));

    Ok(Reference {
        title,
        authors,
        year,
        doi,
        arxiv: Some(arxiv_id.to_string()),
        journal: None,
        tags: vec![],
        files: vec![],
        r#abstract: abstract_text,
    })
}

fn parse_crossref_response(json: &str) -> Result<Reference> {
    let v: serde_json::Value = serde_json::from_str(json).context("Invalid CrossRef JSON")?;
    let msg = &v["message"];

    let title = msg["title"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|t| t.as_str())
        .unwrap_or("Untitled")
        .to_string();

    let authors: Vec<String> = msg["author"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    let given = a["given"].as_str().unwrap_or("");
                    let family = a["family"].as_str().unwrap_or("");
                    if family.is_empty() {
                        None
                    } else if given.is_empty() {
                        Some(family.to_string())
                    } else {
                        Some(format!("{} {}", given, family))
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let year = msg["published-print"]["date-parts"][0][0]
        .as_u64()
        .or_else(|| msg["published-online"]["date-parts"][0][0].as_u64())
        .or_else(|| msg["created"]["date-parts"][0][0].as_u64())
        .and_then(|y| u16::try_from(y).ok());

    let doi = msg["DOI"].as_str().map(|s| s.to_string());

    let journal = msg["container-title"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());

    let abstract_text = msg["abstract"].as_str().map(|s| clean_abstract(s));

    Ok(Reference {
        title,
        authors,
        year,
        doi,
        arxiv: None,
        journal,
        tags: vec![],
        files: vec![],
        r#abstract: abstract_text,
    })
}
