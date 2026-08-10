use std::fs::File;
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fastembed::{
    InitOptionsUserDefined, OutputKey, Pooling, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};
use rusqlite::{Connection, params};
use serde_json::{Map, Value};

use crate::config::{EmbeddingConfig, EmbeddingOutput};
use crate::{metadata, storage};

const DEFAULT_MODEL_ID: &str =
    "onnx-community/embeddinggemma-300m-ONNX@5090578d9565bb06545b4552f76e6bc2c93e4a66#q4";

#[derive(Debug, Clone)]
struct ChunkRecord {
    dir_name: String,
    paper_title: String,
    source_path: String,
    chunk_index: i64,
    text: String,
    headings: Vec<String>,
    pages: Vec<u32>,
    metadata_json: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct IndexReport {
    files: usize,
    chunks: usize,
    skipped: usize,
    malformed: usize,
}

#[derive(Debug)]
pub struct SearchHit {
    pub dir_name: String,
    pub paper_title: String,
    pub source_path: String,
    pub chunk_index: i64,
    pub text: String,
    pub headings: Vec<String>,
    pub pages: Vec<u32>,
    pub similarity: f32,
}

pub fn build(library: &Path, config: &EmbeddingConfig) -> Result<()> {
    validate_embedding_config(config)?;
    let model_id = model_id(config)?;
    let (chunks, report) = collect_chunks(library)?;
    if chunks.is_empty() {
        anyhow::bail!(
            "No indexable JSONL rows found below paper derived/ directories ({} files, {} skipped, {} malformed)",
            report.files,
            report.skipped,
            report.malformed
        );
    }

    println!(
        "Embedding {} passages from {} JSONL files with {}...",
        chunks.len(),
        report.files,
        config.repo
    );
    let mut model = embedding_model(config, true)?;
    let texts: Vec<String> = chunks
        .iter()
        .map(|chunk| document_input(config, &chunk.paper_title, &chunk.text))
        .collect();
    let embeddings = embed_passages(&mut model, &texts, config.batch_size, true)?;
    anyhow::ensure!(
        embeddings.len() == chunks.len(),
        "Embedding model returned {} vectors for {} passages",
        embeddings.len(),
        chunks.len()
    );
    let dimension = embeddings.first().map(Vec::len).unwrap_or_default();
    anyhow::ensure!(dimension > 0, "Embedding model returned empty vectors");
    anyhow::ensure!(
        embeddings
            .iter()
            .all(|embedding| embedding.len() == dimension),
        "Embedding model returned inconsistent vector dimensions"
    );

    replace_index(library, &chunks, &embeddings, dimension, &model_id)?;
    println!(
        "Indexed {} passages from {} files ({} skipped, {} malformed; {} dimensions).",
        report.chunks, report.files, report.skipped, report.malformed, dimension
    );
    Ok(())
}

fn embed_passages(
    model: &mut TextEmbedding,
    texts: &[String],
    batch_size: usize,
    show_progress: bool,
) -> Result<Vec<Vec<f32>>> {
    let started = std::time::Instant::now();
    let interactive = show_progress && std::io::stderr().is_terminal();
    let mut embeddings = Vec::with_capacity(texts.len());
    for batch in texts.chunks(batch_size) {
        let batch_embeddings = model
            .embed(batch, Some(batch_size))
            .context("Failed to embed JSONL passages")?;
        embeddings.extend(batch_embeddings);
        if interactive {
            eprint!(
                "\r{}",
                embedding_progress_line(embeddings.len(), texts.len(), started.elapsed(), 24)
            );
            std::io::stderr().flush()?;
        }
    }
    if interactive {
        eprintln!();
    }
    Ok(embeddings)
}

fn embedding_progress_line(
    completed: usize,
    total: usize,
    elapsed: std::time::Duration,
    width: usize,
) -> String {
    let total = total.max(1);
    let completed = completed.min(total);
    let filled = completed.saturating_mul(width) / total;
    let percent = completed.saturating_mul(100) / total;
    let elapsed_seconds = elapsed.as_secs();
    format!(
        "Embedding passages [{}{}] {:>3}% {completed}/{total} {:02}:{:02}",
        "=".repeat(filled),
        " ".repeat(width.saturating_sub(filled)),
        percent,
        elapsed_seconds / 60,
        elapsed_seconds % 60,
    )
}

pub fn search(
    library: &Path,
    query: &str,
    limit: usize,
    config: &EmbeddingConfig,
) -> Result<Vec<SearchHit>> {
    search_inner(library, query, limit, config, true)
}

pub fn search_silent(
    library: &Path,
    query: &str,
    limit: usize,
    config: &EmbeddingConfig,
) -> Result<Vec<SearchHit>> {
    search_inner(library, query, limit, config, false)
}

fn search_inner(
    library: &Path,
    query: &str,
    limit: usize,
    config: &EmbeddingConfig,
    show_download_progress: bool,
) -> Result<Vec<SearchHit>> {
    validate_embedding_config(config)?;
    let query = query.trim();
    anyhow::ensure!(!query.is_empty(), "Semantic query cannot be empty");
    anyhow::ensure!(limit > 0, "Semantic search limit must be greater than zero");

    let conn = Connection::open(library.join(".grimoire.db"))?;
    let (stored_model_id, dimension, count) = index_metadata(&conn)?;
    let expected_model_id = model_id(config)?;
    anyhow::ensure!(
        stored_model_id == expected_model_id,
        "Semantic index uses model {stored_model_id}; expected {expected_model_id}. Run `grimoire semantic-index` to rebuild it."
    );
    anyhow::ensure!(
        count > 0,
        "Semantic index is empty. Run `grimoire semantic-index` first."
    );

    let mut model = embedding_model(config, show_download_progress)?;
    let embedding_query = query_input(config, query);
    let mut query_embeddings = model
        .embed([embedding_query.as_str()], None)
        .context("Failed to embed semantic query")?;
    let query_embedding = query_embeddings
        .pop()
        .context("Embedding model returned no query vector")?;
    anyhow::ensure!(
        query_embedding.len() == dimension,
        "Query vector has {} dimensions but the index has {dimension}. Run `grimoire semantic-index` to rebuild it.",
        query_embedding.len()
    );

    similarity_search(&conn, &query_embedding, limit)
}

pub fn search_and_print(
    library: &Path,
    query: &str,
    limit: usize,
    config: &EmbeddingConfig,
) -> Result<()> {
    let hits = search(library, query, limit, config)?;
    if hits.is_empty() {
        println!("No semantic matches.");
        return Ok(());
    }

    for (position, hit) in hits.iter().enumerate() {
        let location = match hit.pages.as_slice() {
            [] => String::new(),
            [page] => format!(" · p. {page}"),
            pages => format!(
                " · pp. {}",
                pages
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        println!(
            "{}. {:.3}  {}{}",
            position + 1,
            hit.similarity,
            hit.paper_title,
            location
        );
        if !hit.headings.is_empty() {
            println!("   {}", hit.headings.join(" › "));
        }
        println!("   {}", passage_preview(&hit.text, 280));
        println!("   [{} · chunk {}]", hit.source_path, hit.chunk_index);
    }
    Ok(())
}

fn validate_embedding_config(config: &EmbeddingConfig) -> Result<()> {
    anyhow::ensure!(
        !config.repo.trim().is_empty(),
        "Embedding repo cannot be empty"
    );
    anyhow::ensure!(
        !config.revision.trim().is_empty(),
        "Embedding revision cannot be empty"
    );
    anyhow::ensure!(
        config.max_length > 0,
        "Embedding max_length must be positive"
    );
    anyhow::ensure!(
        config.batch_size > 0,
        "Embedding batch_size must be positive"
    );
    anyhow::ensure!(
        config.query_template.contains("{query}"),
        "Embedding query_template must contain {{query}}"
    );
    anyhow::ensure!(
        config.document_template.contains("{text}"),
        "Embedding document_template must contain {{text}}"
    );
    for path in model_files(config) {
        let path = Path::new(&path);
        anyhow::ensure!(
            !path.is_absolute()
                && !path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir)),
            "Embedding file paths must stay within the model repository: {}",
            path.display()
        );
    }
    Ok(())
}

fn model_id(config: &EmbeddingConfig) -> Result<String> {
    if config == &EmbeddingConfig::default() {
        return Ok(DEFAULT_MODEL_ID.to_string());
    }
    Ok(format!(
        "config:{}",
        serde_json::to_string(config).context("Failed to identify embedding configuration")?
    ))
}

fn query_input(config: &EmbeddingConfig, query: &str) -> String {
    config.query_template.replace("{query}", query)
}

fn document_input(config: &EmbeddingConfig, title: &str, text: &str) -> String {
    config
        .document_template
        .replace("{title}", title.trim())
        .replace("{text}", text)
}

fn model_files(config: &EmbeddingConfig) -> Vec<String> {
    let mut files = vec![
        config.model_file.clone(),
        config.tokenizer_file.clone(),
        config.config_file.clone(),
        config.special_tokens_map_file.clone(),
        config.tokenizer_config_file.clone(),
    ];
    files.extend(config.external_files.iter().cloned());
    files.sort();
    files.dedup();
    files
}

fn embedding_model(
    config: &EmbeddingConfig,
    show_download_progress: bool,
) -> Result<TextEmbedding> {
    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("grimoire")
        .join("models")
        .join(&config.revision);
    std::fs::create_dir_all(&cache_dir)?;
    ensure_model_files(&cache_dir, config, show_download_progress)?;

    let tokenizer_files = TokenizerFiles {
        tokenizer_file: read_model_file(&cache_dir, &config.tokenizer_file)?,
        config_file: read_model_file(&cache_dir, &config.config_file)?,
        special_tokens_map_file: read_model_file(&cache_dir, &config.special_tokens_map_file)?,
        tokenizer_config_file: read_model_file(&cache_dir, &config.tokenizer_config_file)?,
    };
    let mut model = UserDefinedEmbeddingModel::new(
        read_model_file(&cache_dir, &config.model_file)?,
        tokenizer_files,
    );
    for external_file in &config.external_files {
        let file_name = Path::new(external_file)
            .file_name()
            .context("Embedding external file has no file name")?
            .to_string_lossy()
            .to_string();
        model =
            model.with_external_initializer(file_name, read_model_file(&cache_dir, external_file)?);
    }
    model = match config.pooling.as_str() {
        "mean" => model.with_pooling(Pooling::Mean),
        "cls" => model.with_pooling(Pooling::Cls),
        "none" => model,
        pooling => anyhow::bail!("Unsupported embedding pooling method: {pooling}"),
    };
    model.output_key = match &config.output {
        Some(EmbeddingOutput::Name(name)) if name == "sentence_embedding" => {
            Some(OutputKey::ByName("sentence_embedding"))
        }
        Some(EmbeddingOutput::Name(name)) if name == "last_hidden_state" => {
            Some(OutputKey::ByName("last_hidden_state"))
        }
        Some(EmbeddingOutput::Name(name)) if name == "token_embeddings" => {
            Some(OutputKey::ByName("token_embeddings"))
        }
        Some(EmbeddingOutput::Name(name)) => anyhow::bail!(
            "Unsupported embedding output {name:?}; use a numeric output index for another ONNX output"
        ),
        Some(EmbeddingOutput::Index(index)) => Some(OutputKey::ByOrder(*index)),
        None => None,
    };
    let threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(4);
    TextEmbedding::try_new_from_user_defined(
        model,
        InitOptionsUserDefined::new()
            .with_intra_threads(threads)
            .with_max_length(config.max_length),
    )
    .context("Failed to initialize local embedding model")
}

fn ensure_model_files(
    cache_dir: &Path,
    config: &EmbeddingConfig,
    show_download_progress: bool,
) -> Result<()> {
    let client = model_http_client()?;
    for file in model_files(config) {
        let destination = cache_dir.join(&file);
        if destination
            .metadata()
            .is_ok_and(|metadata| metadata.len() > 0)
        {
            continue;
        }
        let parent = destination.parent().context("Model file has no parent")?;
        std::fs::create_dir_all(parent)?;
        let endpoint =
            std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());
        let url = format!(
            "{}/{}/resolve/{}/{file}",
            endpoint.trim_end_matches('/'),
            config.repo,
            config.revision,
        );
        download_model_file(&client, &url, &destination, &file, show_download_progress)?;
    }
    Ok(())
}

fn model_http_client() -> Result<reqwest::blocking::Client> {
    let mut builder = reqwest::blocking::Client::builder();
    let certificate_path = ["SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"]
        .into_iter()
        .find_map(|variable| std::env::var_os(variable).map(PathBuf::from))
        .filter(|path| path.is_file());
    if let Some(path) = certificate_path {
        let bundle = std::fs::read(&path)
            .with_context(|| format!("Failed to read CA bundle {}", path.display()))?;
        for certificate in reqwest::Certificate::from_pem_bundle(&bundle)
            .with_context(|| format!("Failed to parse CA bundle {}", path.display()))?
        {
            builder = builder.add_root_certificate(certificate);
        }
    }
    builder.build().context("Failed to build model HTTP client")
}

fn download_model_file(
    client: &reqwest::blocking::Client,
    url: &str,
    destination: &Path,
    label: &str,
    show_progress: bool,
) -> Result<()> {
    if show_progress {
        eprintln!("Downloading embedding model: {label}");
    }
    let mut response = client
        .get(url)
        .send()
        .with_context(|| format!("Failed to download {label}"))?
        .error_for_status()
        .with_context(|| format!("Failed to download {label}"))?;
    let total = response.content_length();
    let parent = destination.parent().context("Model file has no parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut downloaded = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    let interactive = show_progress && std::io::stderr().is_terminal();
    loop {
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        temporary.write_all(&buffer[..count])?;
        downloaded += count as u64;
        if interactive && let Some(total) = total {
            eprint!(
                "\rDownloading embedding model: {label} ({:.1}/{:.1} MiB)",
                downloaded as f64 / 1_048_576.0,
                total as f64 / 1_048_576.0
            );
        }
    }
    if interactive {
        eprintln!();
    }
    anyhow::ensure!(downloaded > 0, "Downloaded model file {label} is empty");
    if let Some(total) = total {
        anyhow::ensure!(
            downloaded == total,
            "Downloaded model file {label} is incomplete ({downloaded} of {total} bytes)"
        );
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist(destination)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to cache {}", destination.display()))?;
    Ok(())
}

fn read_model_file(cache_dir: &Path, relative_path: &str) -> Result<Vec<u8>> {
    let path = cache_dir.join(relative_path);
    std::fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))
}

fn collect_chunks(library: &Path) -> Result<(Vec<ChunkRecord>, IndexReport)> {
    let mut chunks = Vec::new();
    let mut report = IndexReport::default();

    for ref_dir in storage::list_ref_dirs(library)? {
        let dir_name = ref_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let paper_title = metadata::read_info(&ref_dir)
            .map(|reference| reference.title)
            .unwrap_or_else(|_| dir_name.clone());
        let derived = ref_dir.join("derived");
        if !derived.is_dir() {
            continue;
        }

        let mut jsonl_files = Vec::new();
        find_jsonl_files(&derived, &mut jsonl_files)?;
        jsonl_files.sort();
        for path in jsonl_files {
            report.files += 1;
            let source_path = path
                .strip_prefix(library)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let reader = BufReader::new(
                File::open(&path).with_context(|| format!("Failed to read {}", path.display()))?,
            );
            for (line_index, line) in reader.lines().enumerate() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let value: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(_) => {
                        report.malformed += 1;
                        continue;
                    }
                };
                match normalize_row(&value, &dir_name, &paper_title, &source_path, line_index) {
                    Some(chunk) => {
                        chunks.push(chunk);
                        report.chunks += 1;
                    }
                    None => report.skipped += 1,
                }
            }
        }
    }

    Ok((chunks, report))
}

fn find_jsonl_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            find_jsonl_files(&path, files)?;
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn normalize_row(
    value: &Value,
    dir_name: &str,
    paper_title: &str,
    source_path: &str,
    line_index: usize,
) -> Option<ChunkRecord> {
    let object = value.as_object()?;
    let text = first_string(object, &["text", "content", "page_content", "raw_text"])?;
    let chunk_index = first_value(object, &["chunk_index", "chunk_id", "id"])
        .and_then(integer_value)
        .unwrap_or(line_index as i64);
    let headings = first_value(object, &["headings", "heading", "section", "title"])
        .map(string_values)
        .unwrap_or_default();
    let pages = first_value(object, &["page_numbers", "page_number", "page_no", "page"])
        .map(page_values)
        .unwrap_or_default();

    Some(ChunkRecord {
        dir_name: dir_name.to_string(),
        paper_title: paper_title.to_string(),
        source_path: source_path.to_string(),
        chunk_index,
        text,
        headings,
        pages,
        metadata_json: serde_json::to_string(value).ok()?,
    })
}

fn values_for_keys<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Vec<&'a Value> {
    let mut values = Vec::new();
    for key in keys {
        if let Some(value) = object.get(*key) {
            values.push(value);
        }
    }
    if let Some(metadata) = object.get("metadata").and_then(Value::as_object) {
        for key in keys {
            if let Some(value) = metadata.get(*key) {
                values.push(value);
            }
        }
    }
    values
}

fn first_string(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    values_for_keys(object, keys)
        .into_iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|text| !text.is_empty())
        .map(str::to_string)
}

fn first_value<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    values_for_keys(object, keys)
        .into_iter()
        .find(|value| !value.is_null())
}

fn integer_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn string_values(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => vec![text.trim().to_string()],
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn page_values(value: &Value) -> Vec<u32> {
    let values = match value {
        Value::Array(values) => values.iter().collect(),
        value => vec![value],
    };
    let mut pages: Vec<u32> = values
        .into_iter()
        .filter_map(|value| {
            value
                .as_u64()
                .and_then(|page| u32::try_from(page).ok())
                .or_else(|| value.as_str().and_then(|page| page.parse().ok()))
        })
        .collect();
    pages.sort_unstable();
    pages.dedup();
    pages
}

fn replace_index(
    library: &Path,
    chunks: &[ChunkRecord],
    embeddings: &[Vec<f32>],
    dimension: usize,
    model_id: &str,
) -> Result<()> {
    let conn = Connection::open(library.join(".grimoire.db"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "DROP TABLE IF EXISTS semantic_chunk_fts;
         DROP TABLE IF EXISTS semantic_chunk;
         CREATE TABLE semantic_chunk (
             id INTEGER PRIMARY KEY,
             dir_name TEXT NOT NULL,
             paper_title TEXT NOT NULL,
             source_path TEXT NOT NULL,
             chunk_index INTEGER NOT NULL,
             text TEXT NOT NULL,
             headings TEXT NOT NULL,
             pages TEXT NOT NULL,
             metadata_json TEXT NOT NULL,
             embedding BLOB NOT NULL
         );
         CREATE VIRTUAL TABLE semantic_chunk_fts USING fts5(
             text, headings,
             content='semantic_chunk',
             content_rowid='id'
         );
         CREATE TABLE IF NOT EXISTS semantic_meta (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         DELETE FROM semantic_meta;",
    )?;

    {
        let mut statement = tx.prepare(
            "INSERT INTO semantic_chunk
             (dir_name, paper_title, source_path, chunk_index, text, headings, pages, metadata_json, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for (chunk, embedding) in chunks.iter().zip(embeddings) {
            statement.execute(params![
                chunk.dir_name,
                chunk.paper_title,
                chunk.source_path,
                chunk.chunk_index,
                chunk.text,
                chunk.headings.join("\n"),
                serde_json::to_string(&chunk.pages)?,
                chunk.metadata_json,
                encode_embedding(embedding),
            ])?;
        }
    }

    tx.execute(
        "INSERT INTO semantic_chunk_fts(semantic_chunk_fts) VALUES('rebuild')",
        [],
    )?;
    tx.execute(
        "INSERT INTO semantic_meta(key, value) VALUES('model_id', ?1)",
        [model_id],
    )?;
    tx.execute(
        "INSERT INTO semantic_meta(key, value) VALUES('dimension', ?1)",
        [dimension.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}

fn index_metadata(conn: &Connection) -> Result<(String, usize, usize)> {
    let model_id = semantic_meta(conn, "model_id")
        .context("Semantic index is not initialized. Run `grimoire semantic-index` first.")?;
    let dimension: usize = semantic_meta(conn, "dimension")
        .context("Semantic index has no vector dimension")?
        .parse()
        .context("Semantic index has an invalid vector dimension")?;
    let count: usize = conn
        .query_row("SELECT count(*) FROM semantic_chunk", [], |row| row.get(0))
        .context("Semantic index is incomplete. Run `grimoire semantic-index` to rebuild it.")?;
    Ok((model_id, dimension, count))
}

fn semantic_meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM semantic_meta WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
    .ok()
}

fn similarity_search(
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<SearchHit>> {
    let mut statement = conn.prepare(
        "SELECT dir_name, paper_title, source_path, chunk_index, text, headings, pages, embedding
         FROM semantic_chunk",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Vec<u8>>(7)?,
        ))
    })?;

    let mut chunks = Vec::new();
    for row in rows {
        let (dir_name, paper_title, source_path, chunk_index, text, headings, pages, bytes) = row?;
        let embedding = decode_embedding(&bytes)?;
        let similarity = cosine_similarity(query_embedding, &embedding)?;
        chunks.push(SearchHit {
            dir_name,
            paper_title,
            source_path,
            chunk_index,
            text,
            headings: headings
                .lines()
                .filter(|heading| !heading.is_empty())
                .map(str::to_string)
                .collect(),
            pages: serde_json::from_str(&pages).unwrap_or_default(),
            similarity,
        });
    }

    chunks.sort_by(|left, right| right.similarity.total_cmp(&left.similarity));
    chunks.truncate(limit.min(chunks.len()));
    Ok(chunks)
}

fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
    embedding
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn decode_embedding(bytes: &[u8]) -> Result<Vec<f32>> {
    anyhow::ensure!(
        bytes.len().is_multiple_of(std::mem::size_of::<f32>()),
        "Stored embedding has an invalid byte length"
    );
    Ok(bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect())
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Result<f32> {
    anyhow::ensure!(
        left.len() == right.len(),
        "Embedding dimensions do not match ({} and {})",
        left.len(),
        right.len()
    );
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        return Ok(0.0);
    }
    Ok(dot / (left_norm * right_norm))
}

fn passage_preview(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut preview: String = compact.chars().take(max_chars.saturating_sub(1)).collect();
    preview.push('…');
    preview
}

#[cfg(test)]
mod tests {
    use super::{
        ChunkRecord, DEFAULT_MODEL_ID, IndexReport, collect_chunks, cosine_similarity,
        decode_embedding, document_input, embedding_progress_line, encode_embedding, model_id,
        normalize_row, page_values, passage_preview, query_input, replace_index, similarity_search,
        validate_embedding_config,
    };
    use crate::config::EmbeddingConfig;
    use rusqlite::Connection;
    use serde_json::json;

    #[test]
    fn normalizes_docling_rows() {
        let value = json!({
            "chunk_index": 7,
            "text": "Heading\nA useful passage",
            "headings": ["Heading"],
            "page_numbers": [3, 2, 3]
        });

        let chunk = normalize_row(&value, "paper", "Paper", "paper/chunks.jsonl", 0).unwrap();
        assert_eq!(chunk.chunk_index, 7);
        assert_eq!(chunk.text, "Heading\nA useful passage");
        assert_eq!(chunk.headings, ["Heading"]);
        assert_eq!(chunk.pages, [2, 3]);
    }

    #[test]
    fn normalizes_common_non_docling_rows_and_nested_metadata() {
        let value = json!({
            "page_content": "A passage from another exporter",
            "metadata": {
                "section": "Methods",
                "page": "9",
                "chunk_id": "12"
            }
        });

        let chunk = normalize_row(&value, "paper", "Paper", "paper/export.jsonl", 2).unwrap();
        assert_eq!(chunk.chunk_index, 12);
        assert_eq!(chunk.headings, ["Methods"]);
        assert_eq!(chunk.pages, [9]);
    }

    #[test]
    fn rejects_rows_without_text_content() {
        let value = json!({"chunk_index": 1, "metadata": {"page": 2}});
        assert!(normalize_row(&value, "paper", "Paper", "paper/data.jsonl", 0).is_none());
    }

    #[test]
    fn embeddings_round_trip_and_compare() {
        let embedding = vec![0.25, -0.5, 1.0];
        assert_eq!(
            decode_embedding(&encode_embedding(&embedding)).unwrap(),
            embedding
        );
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]).unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).unwrap(), 0.0);
    }

    #[test]
    fn utility_normalization_is_stable() {
        assert_eq!(page_values(&json!([4, "2", 4, "bad"])), [2, 4]);
        assert_eq!(passage_preview("one  two\nthree", 20), "one two three");
        assert_eq!(passage_preview("abcdef", 4), "abc…");
        assert_eq!(
            embedding_progress_line(5, 10, std::time::Duration::from_secs(65), 10),
            "Embedding passages [=====     ]  50% 5/10 01:05"
        );
    }

    #[test]
    fn embedding_profile_controls_prompts_and_index_identity() {
        let default = EmbeddingConfig::default();
        assert_eq!(
            query_input(&default, "synthetic query"),
            "task: search result | query: synthetic query"
        );
        assert_eq!(
            document_input(&default, "Synthetic Paper", "synthetic passage"),
            "title: Synthetic Paper | text: synthetic passage"
        );
        assert!(validate_embedding_config(&default).is_ok());
        assert_eq!(model_id(&default).unwrap(), DEFAULT_MODEL_ID);

        let mut changed = default.clone();
        changed.document_template = "{title}: {text}".to_string();
        assert_ne!(model_id(&default).unwrap(), model_id(&changed).unwrap());
    }

    #[test]
    fn embedding_profile_rejects_unsafe_model_paths() {
        let config = EmbeddingConfig {
            model_file: "../model.onnx".to_string(),
            ..EmbeddingConfig::default()
        };
        assert!(validate_embedding_config(&config).is_err());
    }

    #[test]
    fn collects_mixed_jsonl_schemas_and_reports_rejected_rows() {
        let library = tempfile::tempdir().unwrap();
        let paper = library.path().join("synthetic-paper");
        let first = paper.join("derived/first");
        let second = paper.join("derived/second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(paper.join("info.toml"), "title = \"Synthetic Paper\"\n").unwrap();
        std::fs::write(
            first.join("chunks.jsonl"),
            concat!(
                "{\"text\":\"First synthetic passage\",\"chunk_index\":2}\n",
                "not json\n",
                "{\"metadata\":{\"page\":3}}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            second.join("export.jsonl"),
            "{\"content\":\"Second synthetic passage\",\"metadata\":{\"page\":4}}\n",
        )
        .unwrap();

        let (chunks, report) = collect_chunks(library.path()).unwrap();
        assert_eq!(
            report,
            IndexReport {
                files: 2,
                chunks: 2,
                skipped: 1,
                malformed: 1,
            }
        );
        assert_eq!(chunks[0].paper_title, "Synthetic Paper");
        assert_eq!(chunks[1].pages, [4]);
    }

    #[test]
    fn search_orders_results_by_descending_similarity() {
        let library = tempfile::tempdir().unwrap();
        let chunks = vec![
            synthetic_chunk("Closest vector", "A related synthetic passage", 0),
            synthetic_chunk("Exact term", "A synthetic passage with needle", 1),
        ];
        replace_index(
            library.path(),
            &chunks,
            &[vec![1.0, 0.0], vec![0.9, 0.1]],
            2,
            "synthetic-model",
        )
        .unwrap();
        let conn = Connection::open(library.path().join(".grimoire.db")).unwrap();

        let hits = similarity_search(&conn, &[1.0, 0.0], 2).unwrap();
        assert_eq!(hits[0].paper_title, "Closest vector");
        assert!(hits[0].similarity >= hits[1].similarity);
    }

    fn synthetic_chunk(paper_title: &str, text: &str, chunk_index: i64) -> ChunkRecord {
        ChunkRecord {
            dir_name: "synthetic-paper".to_string(),
            paper_title: paper_title.to_string(),
            source_path: "synthetic-paper/derived/chunks.jsonl".to_string(),
            chunk_index,
            text: text.to_string(),
            headings: Vec::new(),
            pages: Vec::new(),
            metadata_json: "{}".to_string(),
        }
    }
}
