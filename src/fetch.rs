use anyhow::{Context, Result};
use regex::Regex;
use std::io::Read;

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

pub fn detect_doi_url(input: &str) -> Option<String> {
    let url = reqwest::Url::parse(input).ok()?;
    match url.host_str()? {
        "doi.org" | "dx.doi.org" => detect_doi(input),
        _ => None,
    }
}

pub fn detect_pmc_id(input: &str) -> Option<String> {
    let re =
        Regex::new(r"(?i)^https?://(?:www\.)?pmc\.ncbi\.nlm\.nih\.gov/articles/(PMC\d+)(?:/.*)?$")
            .unwrap();
    re.captures(input)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_ascii_uppercase())
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
    let body = fetch_crossref_body(doi)?;
    parse_crossref_response(&body)
}

pub fn fetch_pmc(pmc_id: &str) -> Result<(Reference, Vec<u8>)> {
    let url = format!(
        "https://www.ncbi.nlm.nih.gov/pmc/utils/idconv/v1.0/?ids={}&format=json&tool=grimoire",
        urlencoding::encode(pmc_id)
    );
    let body = reqwest::blocking::Client::new()
        .get(&url)
        .header("User-Agent", "Grimoire/0.1 (reference manager)")
        .send()
        .context("Failed to reach NCBI PMC ID Converter")?
        .error_for_status()
        .context("NCBI PMC ID Converter returned an error")?
        .text()?;
    let doi = parse_pmc_doi_response(&body, pmc_id)?;

    let crossref_body = fetch_crossref_body(&doi)?;
    let reference = parse_crossref_response(&crossref_body)?;
    let pdf_url = parse_crossref_pdf_url(&crossref_body)?;
    let pdf = match pdf_url {
        Some(url) => download_pdf(&url).or_else(|publisher_error| {
            download_pmc_oa_pdf(pmc_id).with_context(|| {
                format!("Publisher PDF failed ({publisher_error}); PMC Open Access fallback failed")
            })
        })?,
        None => download_pmc_oa_pdf(pmc_id)
            .context("CrossRef had no PDF and the PMC Open Access fallback failed")?,
    };

    Ok((reference, pdf))
}

pub fn download_pdf(url: &str) -> Result<Vec<u8>> {
    let response = reqwest::blocking::Client::new()
        .get(url)
        .header("User-Agent", "Grimoire/0.1 (reference manager)")
        .send()
        .with_context(|| format!("Failed to download PDF: {url}"))?
        .error_for_status()
        .with_context(|| format!("PDF download returned an error: {url}"))?;
    let bytes = response.bytes()?;
    anyhow::ensure!(is_pdf_bytes(&bytes), "Downloaded content is not a PDF");
    Ok(bytes.to_vec())
}

fn fetch_crossref_body(doi: &str) -> Result<String> {
    let url = format!("https://api.crossref.org/works/{}", doi);
    Ok(reqwest::blocking::Client::new()
        .get(&url)
        .header(
            "User-Agent",
            "Grimoire/0.1 (reference manager; mailto:jrfetzer@gmail.com)",
        )
        .send()
        .context("Failed to reach CrossRef API")?
        .error_for_status()
        .context("CrossRef API returned an error")?
        .text()?)
}

fn download_pmc_oa_pdf(pmc_id: &str) -> Result<Vec<u8>> {
    let url = format!(
        "https://www.ncbi.nlm.nih.gov/pmc/utils/oa/oa.fcgi?id={}",
        urlencoding::encode(pmc_id)
    );
    let body = reqwest::blocking::Client::new()
        .get(&url)
        .header("User-Agent", "Grimoire/0.1 (reference manager)")
        .send()
        .context("Failed to reach NCBI PMC Open Access API")?
        .error_for_status()
        .context("NCBI PMC Open Access API returned an error")?
        .text()?;
    let package_url = parse_pmc_oa_package_url(&body)?;
    let package_url = package_url
        .strip_prefix("ftp://")
        .map(|rest| format!("https://{rest}"))
        .unwrap_or(package_url);
    let bytes = reqwest::blocking::Client::new()
        .get(&package_url)
        .header("User-Agent", "Grimoire/0.1 (reference manager)")
        .send()
        .context("Failed to download PMC Open Access package")?
        .error_for_status()
        .context("PMC Open Access package download returned an error")?
        .bytes()?;
    extract_pdf_from_oa_package(&bytes)
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

    let abstract_text = msg["abstract"].as_str().map(clean_abstract);

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

fn parse_pmc_doi_response(json: &str, pmc_id: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(json).context("Invalid NCBI PMC JSON")?;
    v["records"]
        .as_array()
        .and_then(|records| {
            records.iter().find(|record| {
                record["pmcid"]
                    .as_str()
                    .is_some_and(|id| id.eq_ignore_ascii_case(pmc_id))
            })
        })
        .and_then(|record| record["doi"].as_str())
        .map(str::to_string)
        .with_context(|| format!("NCBI did not provide a DOI for {pmc_id}"))
}

fn parse_crossref_pdf_url(json: &str) -> Result<Option<String>> {
    let v: serde_json::Value = serde_json::from_str(json).context("Invalid CrossRef JSON")?;
    Ok(v["message"]["link"].as_array().and_then(|links| {
        links.iter().find_map(|link| {
            let url = link["URL"].as_str()?;
            let content_type = link["content-type"].as_str().unwrap_or("");
            if content_type.contains("pdf") || url.to_ascii_lowercase().ends_with(".pdf") {
                Some(
                    url.strip_prefix("http://")
                        .map(|rest| format!("https://{rest}"))
                        .unwrap_or_else(|| url.to_string()),
                )
            } else {
                None
            }
        })
    }))
}

fn parse_pmc_oa_package_url(xml: &str) -> Result<String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e))
                if e.name().as_ref() == b"link" =>
            {
                let mut format = None;
                let mut href = None;
                for attribute in e.attributes().flatten() {
                    match attribute.key.as_ref() {
                        b"format" => {
                            format = Some(String::from_utf8_lossy(&attribute.value).into_owned())
                        }
                        b"href" => {
                            href = Some(String::from_utf8_lossy(&attribute.value).into_owned())
                        }
                        _ => {}
                    }
                }
                if format.as_deref() == Some("tgz")
                    && let Some(href) = href
                {
                    return Ok(href);
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(error) => anyhow::bail!("Invalid NCBI PMC Open Access XML: {error}"),
            _ => {}
        }
        buf.clear();
    }

    anyhow::bail!("NCBI did not provide a PMC Open Access package")
}

fn extract_pdf_from_oa_package(bytes: &[u8]) -> Result<Vec<u8>> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive
        .entries()
        .context("Invalid PMC Open Access package")?
    {
        let mut entry = entry.context("Invalid entry in PMC Open Access package")?;
        let is_pdf = entry
            .path()
            .ok()
            .and_then(|path| path.extension().map(|extension| extension.to_owned()))
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));
        if is_pdf {
            let mut pdf = Vec::new();
            entry.read_to_end(&mut pdf)?;
            anyhow::ensure!(is_pdf_bytes(&pdf), "PMC package entry is not a PDF");
            return Ok(pdf);
        }
    }
    anyhow::bail!("PMC Open Access package did not contain a PDF")
}

fn is_pdf_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

#[cfg(test)]
mod tests {
    use super::{
        detect_doi_url, detect_pmc_id, extract_pdf_from_oa_package, is_pdf_bytes,
        parse_crossref_pdf_url, parse_pmc_doi_response, parse_pmc_oa_package_url,
    };

    fn oa_package(path: &str, contents: &[u8]) -> Vec<u8> {
        let mut compressed = Vec::new();
        {
            let encoder =
                flate2::write::GzEncoder::new(&mut compressed, flate2::Compression::default());
            let mut archive = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, path, contents).unwrap();
            archive.into_inner().unwrap().finish().unwrap();
        }
        compressed
    }

    #[test]
    fn detects_pmc_article_urls() {
        assert_eq!(
            detect_pmc_id("https://pmc.ncbi.nlm.nih.gov/articles/PMC1234567/"),
            Some("PMC1234567".to_string())
        );
        assert_eq!(
            detect_pmc_id("https://pmc.ncbi.nlm.nih.gov/articles/PMC1234567/pdf/synthetic.pdf"),
            Some("PMC1234567".to_string())
        );
        assert_eq!(detect_pmc_id("https://example.com/PMC1234567"), None);
    }

    #[test]
    fn distinguishes_doi_resolvers_from_publisher_urls() {
        assert_eq!(
            detect_doi_url("https://doi.org/10.1234/synthetic"),
            Some("10.1234/synthetic".to_string())
        );
        assert_eq!(
            detect_doi_url("https://publisher.example/10.1234/synthetic.pdf"),
            None
        );
    }

    #[test]
    fn parses_pmc_doi() {
        let json = r#"{"records":[{"pmcid":"PMC1234567","doi":"10.1234/synthetic"}]}"#;
        assert_eq!(
            parse_pmc_doi_response(json, "PMC1234567").unwrap(),
            "10.1234/synthetic"
        );
    }

    #[test]
    fn selects_and_secures_crossref_pdf_url() {
        let json = r#"{"message":{"link":[
            {"URL":"https://example.com/article.xml","content-type":"application/xml"},
            {"URL":"http://example.com/synthetic.pdf","content-type":"unspecified"}
        ]}}"#;
        assert_eq!(
            parse_crossref_pdf_url(json).unwrap(),
            Some("https://example.com/synthetic.pdf".to_string())
        );
    }

    #[test]
    fn validates_pdf_signature() {
        assert!(is_pdf_bytes(b"%PDF-1.7\nsynthetic"));
        assert!(!is_pdf_bytes(b"<html>synthetic</html>"));
    }

    #[test]
    fn parses_pmc_open_access_package_url() {
        let xml = r#"<OA><record><link format="tgz" href="ftp://example.com/synthetic.tar.gz" /></record></OA>"#;
        assert_eq!(
            parse_pmc_oa_package_url(xml).unwrap(),
            "ftp://example.com/synthetic.tar.gz"
        );
    }

    #[test]
    fn extracts_pdf_from_pmc_open_access_package() {
        let compressed = oa_package("synthetic/article.pdf", b"%PDF-1.7\nsynthetic");

        assert_eq!(
            extract_pdf_from_oa_package(&compressed).unwrap(),
            b"%PDF-1.7\nsynthetic"
        );
    }

    #[test]
    fn rejects_non_pdf_content_in_pmc_open_access_package() {
        let compressed = oa_package("synthetic/article.pdf", b"<html>synthetic</html>");

        assert_eq!(
            extract_pdf_from_oa_package(&compressed)
                .unwrap_err()
                .to_string(),
            "PMC package entry is not a PDF"
        );
    }
}
