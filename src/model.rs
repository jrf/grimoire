use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub year: Option<u16>,
    pub doi: Option<String>,
    pub arxiv: Option<String>,
    pub journal: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    pub r#abstract: Option<String>,
}
