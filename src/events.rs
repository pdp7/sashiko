use crate::patch::{Patch, PatchsetMetadata};

#[derive(Debug)]
#[allow(dead_code)]
pub enum Event {
    ArticleFetched {
        group: String,
        article_id: String,
        content: Vec<String>,
        raw: Option<Vec<u8>>,
    },
}

#[derive(Debug)]
pub struct ParsedArticle {
    pub group: String,
    pub article_id: String,
    pub metadata: PatchsetMetadata,
    pub patch: Option<Patch>,
}
