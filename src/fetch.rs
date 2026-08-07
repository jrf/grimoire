use anyhow::{Context, Result};
use regex::Regex;
use std::io::Read;
use std::time::Duration;

use crate::model::Reference;

const USER_AGENT: &str = "Grimoire/0.1 (reference manager; mailto:jrfetzer@gmail.com)";

/// Shared blocking HTTP client with connect and overall timeouts, so a slow or
/// half-open server can never hang the app indefinitely. Every network call
/// goes through this rather than `Client::new()`/`blocking::get`, which have no
/// timeout by default.
fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")
}

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
    let body = http_client()?
        .get(&url)
        .send()
        .context("Failed to reach arXiv API")?
        .error_for_status()
        .context("arXiv API returned an error")?
        .text()?;

    parse_arxiv_response(&body, id_clean)
}

pub fn download_arxiv_pdf(arxiv_id: &str, dest: &std::path::Path) -> Result<()> {
    let id_clean = arxiv_id.trim_end_matches(".pdf");
    let url = format!("https://arxiv.org/pdf/{}.pdf", id_clean);
    let bytes = http_client()?
        .get(&url)
        .send()
        .context("Failed to download PDF from arXiv")?
        .error_for_status()
        .context("arXiv PDF download returned an error")?
        .bytes()?;

    anyhow::ensure!(
        is_pdf_bytes(&bytes),
        "arXiv did not return a PDF (got {} bytes of non-PDF content)",
        bytes.len()
    );
    std::fs::write(dest, &bytes)?;
    Ok(())
}

pub fn search_crossref_by_title(title: &str) -> Result<Reference> {
    let url = format!(
        "https://api.crossref.org/works?query.title={}&rows=1",
        urlencoding::encode(title)
    );
    let body = http_client()?
        .get(&url)
        .send()
        .context("Failed to reach CrossRef API")?
        .error_for_status()
        .context("CrossRef API returned an error")?
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
    let body = http_client()?
        .get(&url)
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
    let response = http_client()?
        .get(url)
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
    Ok(http_client()?
        .get(&url)
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
    let body = http_client()?
        .get(&url)
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
    let bytes = http_client()?
        .get(&package_url)
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
    Ok(parse_csl_item(&v["message"]))
}

/// Parse a CSL-JSON item into a Reference. This is the shape shared by the
/// CrossRef `message` object and NCBI's citation exporter, so DOI and PubMed
/// imports run through the same mapping. Fields may be a string or a
/// single-element array depending on the source, so both are accepted.
fn parse_csl_item(item: &serde_json::Value) -> Reference {
    let str_or_first = |v: &serde_json::Value| -> Option<String> {
        v.as_str().map(str::to_string).or_else(|| {
            v.as_array()
                .and_then(|a| a.first())
                .and_then(|t| t.as_str())
                .map(str::to_string)
        })
    };

    let title = str_or_first(&item["title"]).unwrap_or_else(|| "Untitled".to_string());

    let authors: Vec<String> = item["author"]
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
                        Some(format!("{given} {family}"))
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // CSL uses `issued`; CrossRef adds published-print/-online/created fallbacks.
    let year = item["issued"]["date-parts"][0][0]
        .as_u64()
        .or_else(|| item["published-print"]["date-parts"][0][0].as_u64())
        .or_else(|| item["published-online"]["date-parts"][0][0].as_u64())
        .or_else(|| item["created"]["date-parts"][0][0].as_u64())
        .and_then(|y| u16::try_from(y).ok());

    let doi = item["DOI"].as_str().map(|s| s.to_string());
    let journal = str_or_first(&item["container-title"]);
    let abstract_text = item["abstract"].as_str().map(clean_abstract);

    Reference {
        title,
        authors,
        year,
        doi,
        arxiv: None,
        journal,
        tags: vec![],
        files: vec![],
        r#abstract: abstract_text,
    }
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

/// Extract a DOI embedded anywhere in a URL's path or query (e.g. a PLoS
/// `?id=10.1371/...` link), trimming URL separators and a trailing `.pdf`.
pub fn detect_doi_in_url(url: &str) -> Option<String> {
    let decoded = urlencoding::decode(url)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| url.to_string());
    let mut doi = detect_doi(&decoded)?;
    // detect_doi's `[^\s]+` greedily swallows query/fragment; cut them off.
    if let Some(i) = doi.find(['?', '#', '&']) {
        doi.truncate(i);
    }
    let doi = doi.trim_end_matches(['.', ',', ';', ')', '/']);
    let doi = doi.strip_suffix(".pdf").unwrap_or(doi);
    (doi.len() > 3).then(|| doi.to_string())
}

/// Detect a PubMed article, either as a `pubmed.ncbi.nlm.nih.gov/<pmid>` URL or
/// a `PMID:<n>` string. Returns the bare PMID.
pub fn detect_pmid(input: &str) -> Option<String> {
    let url_re =
        Regex::new(r"(?i)^https?://(?:www\.)?pubmed\.ncbi\.nlm\.nih\.gov/(\d{4,9})/?$").unwrap();
    if let Some(c) = url_re.captures(input) {
        return c.get(1).map(|m| m.as_str().to_string());
    }
    let prefix_re = Regex::new(r"(?i)^pmid:\s*(\d{4,9})$").unwrap();
    prefix_re
        .captures(input)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Resolve a PubMed ID to a Reference via NCBI's Literature Citation Exporter,
/// which returns CSL-JSON directly — so this works even for records that carry
/// no DOI (unlike a DOI-then-CrossRef round trip).
pub fn fetch_pubmed(pmid: &str) -> Result<Reference> {
    let url = format!(
        "https://api.ncbi.nlm.nih.gov/lit/ctxp/v1/pubmed/?format=csl&id={}",
        urlencoding::encode(pmid)
    );
    let body = http_client()?
        .get(&url)
        .send()
        .context("Failed to reach NCBI citation exporter")?
        .error_for_status()
        .context("NCBI citation exporter returned an error")?
        .text()?;
    let v: serde_json::Value = serde_json::from_str(&body).context("Invalid PubMed CSL JSON")?;
    // The exporter returns a single CSL object, or an array when given many ids.
    let item = v.as_array().and_then(|a| a.first()).unwrap_or(&v);
    let reference = parse_csl_item(item);
    anyhow::ensure!(
        reference.title != "Untitled" || reference.doi.is_some(),
        "No metadata found for PMID {pmid}"
    );
    Ok(reference)
}

/// Look up an open-access PDF URL for a DOI via Unpaywall. Best-effort: any
/// network/parse error or absence of free full text yields `None`.
pub fn unpaywall_pdf_url(doi: &str) -> Option<String> {
    let url = format!(
        "https://api.unpaywall.org/v2/{}?email=jrfetzer@gmail.com",
        urlencoding::encode(doi)
    );
    let body = http_client()
        .ok()?
        .get(&url)
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    v["best_oa_location"]["url_for_pdf"]
        .as_str()
        .map(str::to_string)
}

/// DOI and/or direct PDF URL scraped from a publisher landing page's Highwire
/// `<meta>` tags (`citation_doi`, `citation_pdf_url`) — the tags most academic
/// publishers emit for Google Scholar.
pub struct LandingInfo {
    pub doi: Option<String>,
    pub pdf_url: Option<String>,
}

pub fn resolve_landing_page(url: &str) -> Result<LandingInfo> {
    let html = http_client()?
        .get(url)
        .send()
        .with_context(|| format!("Failed to fetch page: {url}"))?
        .error_for_status()
        .with_context(|| format!("Page returned an error: {url}"))?
        .text()?;

    let doi = meta_content(&html, "citation_doi")
        .or_else(|| meta_content(&html, "prism.doi"))
        .or_else(|| meta_content(&html, "dc.identifier").filter(|s| s.contains("10.")))
        .map(|d| {
            d.trim()
                .trim_start_matches("doi:")
                .trim_start_matches("DOI:")
                .trim()
                .to_string()
        })
        .and_then(|d| detect_doi(&d));
    let pdf_url = meta_content(&html, "citation_pdf_url").map(|u| resolve_relative(url, &u));

    Ok(LandingInfo { doi, pdf_url })
}

/// Read a `<meta name="NAME" content="VALUE">` tag's content, tolerating either
/// attribute order.
fn meta_content(html: &str, name: &str) -> Option<String> {
    let name = regex::escape(name);
    let patterns = [
        format!(r#"(?is)<meta[^>]*\bname=["']{name}["'][^>]*\bcontent=["']([^"']*)["']"#),
        format!(r#"(?is)<meta[^>]*\bcontent=["']([^"']*)["'][^>]*\bname=["']{name}["']"#),
    ];
    for pat in patterns {
        if let Some(c) = Regex::new(&pat).ok()?.captures(html)
            && let Some(m) = c.get(1)
        {
            let val = m.as_str().trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn resolve_relative(base: &str, link: &str) -> String {
    reqwest::Url::parse(base)
        .and_then(|b| b.join(link))
        .map(|u| u.to_string())
        .unwrap_or_else(|_| link.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        detect_doi_in_url, detect_doi_url, detect_pmc_id, detect_pmid, extract_pdf_from_oa_package,
        is_pdf_bytes, meta_content, parse_crossref_pdf_url, parse_csl_item, parse_pmc_doi_response,
        parse_pmc_oa_package_url,
    };

    #[test]
    fn extracts_doi_embedded_in_a_url() {
        assert_eq!(
            detect_doi_in_url(
                "https://journals.plos.org/plosone/article?id=10.1371/journal.pone.0123456"
            ),
            Some("10.1371/journal.pone.0123456".to_string())
        );
        // Encoded slash in the query.
        assert_eq!(
            detect_doi_in_url("https://example.org/article?id=10.1371%2Fjournal.pone.0123456"),
            Some("10.1371/journal.pone.0123456".to_string())
        );
        // Trailing .pdf and fragment are trimmed.
        assert_eq!(
            detect_doi_in_url("https://host/pdf/10.1016/j.media.2022.102464.pdf#page=1"),
            Some("10.1016/j.media.2022.102464".to_string())
        );
        assert_eq!(detect_doi_in_url("https://arxiv.org/abs/2301.12345"), None);
    }

    #[test]
    fn detects_pmid_from_url_and_prefix() {
        assert_eq!(
            detect_pmid("https://pubmed.ncbi.nlm.nih.gov/35432197/"),
            Some("35432197".to_string())
        );
        assert_eq!(detect_pmid("PMID: 35432197"), Some("35432197".to_string()));
        assert_eq!(
            detect_pmid("https://pmc.ncbi.nlm.nih.gov/articles/PMC123/"),
            None
        );
        assert_eq!(detect_pmid("just some text"), None);
    }

    #[test]
    fn reads_meta_tags_in_either_attribute_order() {
        let html = r#"<meta name="citation_doi" content="10.1038/s41586-021-03819-2">"#;
        assert_eq!(
            meta_content(html, "citation_doi"),
            Some("10.1038/s41586-021-03819-2".to_string())
        );
        let reversed = r#"<meta content="https://x.org/a.pdf" name="citation_pdf_url" />"#;
        assert_eq!(
            meta_content(reversed, "citation_pdf_url"),
            Some("https://x.org/a.pdf".to_string())
        );
        assert_eq!(meta_content(html, "citation_pdf_url"), None);
    }

    #[test]
    fn parses_csl_item_with_string_or_array_fields() {
        // PubMed CSL shape: title/container-title are plain strings, date in `issued`.
        let csl = serde_json::json!({
            "title": "Deep learning",
            "container-title": "Nature",
            "author": [{"family": "LeCun", "given": "Yann"}, {"family": "Bengio", "given": "Yoshua"}],
            "issued": {"date-parts": [[2015, 5, 28]]},
            "DOI": "10.1038/nature14539"
        });
        let r = parse_csl_item(&csl);
        assert_eq!(r.title, "Deep learning");
        assert_eq!(r.journal.as_deref(), Some("Nature"));
        assert_eq!(r.authors, vec!["Yann LeCun", "Yoshua Bengio"]);
        assert_eq!(r.year, Some(2015));
        assert_eq!(r.doi.as_deref(), Some("10.1038/nature14539"));

        // CrossRef shape: title/container-title are arrays, date in published-print.
        let crossref = serde_json::json!({
            "title": ["Attention Is All You Need"],
            "container-title": ["NeurIPS"],
            "published-print": {"date-parts": [[2017]]}
        });
        let r2 = parse_csl_item(&crossref);
        assert_eq!(r2.title, "Attention Is All You Need");
        assert_eq!(r2.journal.as_deref(), Some("NeurIPS"));
        assert_eq!(r2.year, Some(2017));
    }

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
