//! Heading-based chunking for semantic wiki indexing.
//!
//! Spec: `docs/design/phase2/05-knowledge-semantic.md §5`.
//!
//! P2A keeps the implementation deliberately small:
//!
//! - `## ` headings define section boundaries
//! - pages with no h2 headings stay as a single chunk
//! - oversize sections split on paragraph boundaries
//! - undersize chunks merge with their neighbor so embeddings do not
//!   get dominated by tiny fragments

use std::mem;

/// Maximum approximate token count for a chunk before paragraph splitting.
pub const CHUNK_MAX_TOKENS: usize = 512;

/// Minimum approximate token count. Smaller chunks are merged with a
/// neighbor to avoid low-signal embeddings.
pub const CHUNK_MIN_TOKENS: usize = 64;

/// A semantic chunk derived from a wiki page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub page_name: String,
    pub chunk_index: usize,
    pub section: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChunkDraft {
    section: Option<String>,
    content: String,
}

/// Approximate token count for embedding chunk sizing.
///
/// P2A uses a whitespace split instead of model-specific tokenization so the
/// behavior stays deterministic and dependency-light.
pub fn count_tokens(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Split a wiki page body into semantic chunks.
///
/// `body` must be the markdown body without TOML frontmatter. Callers that
/// read raw page files should run `parse_page` first.
pub fn chunk_page(page_name: &str, body: &str) -> Vec<Chunk> {
    if body.trim().is_empty() {
        return Vec::new();
    }

    let mut drafts = Vec::new();
    for section in split_sections(body) {
        drafts.extend(split_oversize_section(section));
    }
    if drafts.len() > 1 {
        merge_small_chunks(&mut drafts);
    }

    drafts
        .into_iter()
        .enumerate()
        .map(|(chunk_index, draft)| Chunk {
            page_name: page_name.to_string(),
            chunk_index,
            section: draft.section,
            content: draft.content,
        })
        .collect()
}

fn split_sections(body: &str) -> Vec<ChunkDraft> {
    let has_h2 = body
        .lines()
        .any(|line| line.trim_end_matches('\r').starts_with("## "));
    if !has_h2 {
        return normalize_chunk(body)
            .into_iter()
            .map(|content| ChunkDraft {
                section: None,
                content,
            })
            .collect();
    }

    let mut out = Vec::new();
    let mut pending_intro = String::new();
    let mut current_section: Option<String> = None;
    let mut current_content = String::new();

    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if let Some(section) = trimmed.strip_prefix("## ") {
            if let Some(prev_section) = current_section.take() {
                push_section(
                    &mut out,
                    Some(prev_section),
                    mem::take(&mut current_content),
                );
            } else if !pending_intro.trim().is_empty() {
                current_content = mem::take(&mut pending_intro);
            }
            current_section = Some(section.trim().to_string());
            continue;
        }

        if current_section.is_some() {
            current_content.push_str(line);
        } else {
            pending_intro.push_str(line);
        }
    }

    if let Some(section) = current_section {
        push_section(&mut out, Some(section), current_content);
    } else {
        push_section(&mut out, None, pending_intro);
    }

    out
}

fn push_section(out: &mut Vec<ChunkDraft>, section: Option<String>, content: String) {
    if let Some(content) = normalize_chunk(&content) {
        out.push(ChunkDraft { section, content });
    }
}

fn normalize_chunk(content: &str) -> Option<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn split_oversize_section(section: ChunkDraft) -> Vec<ChunkDraft> {
    if count_tokens(&section.content) <= CHUNK_MAX_TOKENS {
        return vec![section];
    }

    let section_name = section.section.clone();
    let paragraphs = split_paragraphs(&section.content);
    if paragraphs.len() <= 1 {
        return vec![section];
    }

    let mut out = Vec::new();
    let mut current = String::new();

    for paragraph in paragraphs {
        if current.is_empty() {
            if count_tokens(&paragraph) > CHUNK_MAX_TOKENS {
                out.push(ChunkDraft {
                    section: section_name.clone(),
                    content: paragraph,
                });
            } else {
                current = paragraph;
            }
            continue;
        }

        let candidate = join_chunks(&current, &paragraph);
        if count_tokens(&candidate) <= CHUNK_MAX_TOKENS {
            current = candidate;
            continue;
        }

        out.push(ChunkDraft {
            section: section_name.clone(),
            content: mem::take(&mut current),
        });

        if count_tokens(&paragraph) > CHUNK_MAX_TOKENS {
            out.push(ChunkDraft {
                section: section_name.clone(),
                content: paragraph,
            });
        } else {
            current = paragraph;
        }
    }

    if !current.is_empty() {
        out.push(ChunkDraft {
            section: section_name,
            content: current,
        });
    }

    if out.is_empty() {
        vec![section]
    } else {
        out
    }
}

fn split_paragraphs(content: &str) -> Vec<String> {
    content.split("\n\n").filter_map(normalize_chunk).collect()
}

fn merge_small_chunks(chunks: &mut Vec<ChunkDraft>) {
    if chunks.len() < 2 {
        return;
    }

    let mut idx = 0usize;
    while idx < chunks.len() {
        if count_tokens(&chunks[idx].content) >= CHUNK_MIN_TOKENS {
            idx += 1;
            continue;
        }

        if idx + 1 < chunks.len() {
            let next = chunks.remove(idx + 1);
            chunks[idx].content = join_chunks(&chunks[idx].content, &next.content);
            if chunks[idx].section.is_none() {
                chunks[idx].section = next.section;
            }
            continue;
        }

        if idx > 0 {
            let current = chunks.remove(idx);
            let prev = &mut chunks[idx - 1];
            prev.content = join_chunks(&prev.content, &current.content);
            if prev.section.is_none() {
                prev.section = current.section;
            }
            idx -= 1;
            continue;
        }

        break;
    }
}

fn join_chunks(left: &str, right: &str) -> String {
    match (normalize_chunk(left), normalize_chunk(right)) {
        (Some(left), Some(right)) => format!("{left}\n\n{right}"),
        (Some(left), None) => left,
        (None, Some(right)) => right,
        (None, None) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(count: usize) -> String {
        (0..count)
            .map(|idx| format!("word{idx}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn chunk_single_h2_section_produces_one_chunk() {
        let body = format!("## Diagnosis\n\n{}", words(80));
        let chunks = chunk_page("incidents/boot", &body);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].section.as_deref(), Some("Diagnosis"));
    }

    #[test]
    fn chunk_multiple_h2_sections_splits_correctly() {
        let body = format!("## One\n\n{}\n\n## Two\n\n{}", words(80), words(90));
        let chunks = chunk_page("runbooks/test", &body);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].section.as_deref(), Some("One"));
        assert_eq!(chunks[1].section.as_deref(), Some("Two"));
    }

    #[test]
    fn chunk_no_headings_single_chunk() {
        let body = words(120);
        let chunks = chunk_page("notes/plain", &body);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].section, None);
        assert_eq!(chunks[0].content, body);
    }

    #[test]
    fn chunk_oversize_section_splits_by_paragraph() {
        let body = format!("## Long\n\n{}\n\n{}", words(300), words(300));
        let chunks = chunk_page("notes/long", &body);
        assert_eq!(chunks.len(), 2);
        assert!(chunks
            .iter()
            .all(|chunk| count_tokens(&chunk.content) <= CHUNK_MAX_TOKENS));
    }

    #[test]
    fn chunk_undersize_merges_with_next() {
        let body = format!("## Short\n\n{}\n\n## Long\n\n{}", words(10), words(90));
        let chunks = chunk_page("notes/merge-next", &body);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("word9"));
        assert!(chunks[0].content.contains("word89"));
    }

    #[test]
    fn chunk_undersize_at_end_merges_with_prev() {
        let body = format!("## Long\n\n{}\n\n## Tail\n\n{}", words(90), words(10));
        let chunks = chunk_page("notes/merge-prev", &body);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("word89"));
        assert!(chunks[0].content.contains("word9"));
    }

    #[test]
    fn chunk_preserves_section_metadata() {
        let body = format!("Intro\n\n## Runbook\n\n{}", words(80));
        let chunks = chunk_page("runbooks/install", &body);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].section.as_deref(), Some("Runbook"));
        assert!(chunks[0].content.starts_with("Intro"));
    }

    #[test]
    fn chunk_index_is_zero_based_sequential() {
        let body = format!(
            "## One\n\n{}\n\n{}\n\n## Two\n\n{}",
            words(300),
            words(300),
            words(90)
        );
        let chunks = chunk_page("notes/indexes", &body);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[1].chunk_index, 1);
        assert_eq!(chunks[2].chunk_index, 2);
    }

    #[test]
    fn chunk_h1_is_ignored_as_section_boundary() {
        let body = format!("# Title\n\n{}", words(80));
        let chunks = chunk_page("notes/h1", &body);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].section, None);
    }

    #[test]
    fn chunk_empty_body_produces_no_chunks() {
        assert!(chunk_page("notes/empty", "").is_empty());
        assert!(chunk_page("notes/space", " \n\n ").is_empty());
    }
}
