use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceKind {
    #[default]
    Paper,
    Book,
}

impl ReferenceKind {
    pub fn is_paper(&self) -> bool {
        *self == Self::Paper
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    #[serde(default, skip_serializing_if = "ReferenceKind::is_paper")]
    pub kind: ReferenceKind,
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub year: Option<u16>,
    pub doi: Option<String>,
    pub arxiv: Option<String>,
    pub journal: Option<String>,
    pub edition: Option<String>,
    pub publisher: Option<String>,
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub isbn: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    pub r#abstract: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{Reference, ReferenceKind};

    #[test]
    fn legacy_metadata_defaults_to_paper() {
        let reference: Reference = toml::from_str("title = \"Synthetic work\"\n").unwrap();
        assert_eq!(reference.kind, ReferenceKind::Paper);
        assert!(reference.edition.is_none());
        assert!(reference.isbn.is_empty());
    }

    #[test]
    fn book_metadata_round_trips() {
        let source = concat!(
            "kind = \"book\"\n",
            "title = \"Synthetic Analysis\"\n",
            "authors = [\"Ada Example\"]\n",
            "year = 2026\n",
            "edition = \"2\"\n",
            "publisher = \"Example Press\"\n",
            "isbn = [\"978-0-00-000000-0\"]\n",
        );
        let reference: Reference = toml::from_str(source).unwrap();
        assert_eq!(reference.kind, ReferenceKind::Book);
        assert_eq!(reference.edition.as_deref(), Some("2"));
        assert_eq!(reference.isbn, ["978-0-00-000000-0"]);
        assert_eq!(
            toml::from_str::<Reference>(&toml::to_string(&reference).unwrap()).unwrap(),
            reference
        );
    }
}
