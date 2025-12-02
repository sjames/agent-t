use anyhow::{anyhow, Result};
use std::path::Path;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

use crate::vecdb::CodeChunk;

/// Maximum chunk size in characters for fallback chunking
const MAX_CHUNK_SIZE: usize = 1500;

/// Get the appropriate tree-sitter language for a file extension
fn get_language(ext: &str) -> Option<Language> {
    match ext {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "js" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "c" | "h" => Some(tree_sitter_c::LANGUAGE.into()),
        "cpp" | "cc" | "cxx" | "hpp" => Some(tree_sitter_cpp::LANGUAGE.into()),
        _ => None,
    }
}

/// Get tree-sitter query patterns for extracting code units by language
fn get_query_patterns(ext: &str) -> Option<&'static str> {
    match ext {
        "rs" => Some(
            r#"
            (function_item) @function
            (impl_item) @impl
            (struct_item) @struct
            (enum_item) @enum
            (trait_item) @trait
            (mod_item) @module
            (const_item) @const
            (static_item) @static
            "#,
        ),
        "py" => Some(
            r#"
            (function_definition) @function
            (class_definition) @class
            "#,
        ),
        "js" | "jsx" => Some(
            r#"
            (function_declaration) @function
            (method_definition) @method
            (class_declaration) @class
            (arrow_function) @arrow_function
            (function_expression) @function_expr
            "#,
        ),
        "ts" | "tsx" => Some(
            r#"
            (function_declaration) @function
            (method_definition) @method
            (class_declaration) @class
            (interface_declaration) @interface
            (type_alias_declaration) @type
            (enum_declaration) @enum
            (arrow_function) @arrow_function
            (function_expression) @function_expr
            "#,
        ),
        "go" => Some(
            r#"
            (function_declaration) @function
            (method_declaration) @method
            (type_declaration) @type
            "#,
        ),
        "java" => Some(
            r#"
            (method_declaration) @method
            (class_declaration) @class
            (interface_declaration) @interface
            (enum_declaration) @enum
            (constructor_declaration) @constructor
            "#,
        ),
        "c" | "h" => Some(
            r#"
            (function_definition) @function
            (struct_specifier) @struct
            (enum_specifier) @enum
            (union_specifier) @union
            "#,
        ),
        "cpp" | "cc" | "cxx" | "hpp" => Some(
            r#"
            (function_definition) @function
            (class_specifier) @class
            (struct_specifier) @struct
            (namespace_definition) @namespace
            (enum_specifier) @enum
            (template_declaration) @template
            "#,
        ),
        _ => None,
    }
}

/// Extract code chunks using tree-sitter parsing
pub fn chunk_code_with_tree_sitter(
    file_path: &Path,
    content: &str,
    language: &str,
) -> Result<Vec<CodeChunk>> {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // Get language and query pattern
    let Some(ts_language) = get_language(ext) else {
        // Fallback to simple chunking for unsupported languages
        return Ok(fallback_chunk(file_path, content, language));
    };

    let Some(query_pattern) = get_query_patterns(ext) else {
        return Ok(fallback_chunk(file_path, content, language));
    };

    // Parse the file
    let mut parser = Parser::new();
    parser
        .set_language(&ts_language)
        .map_err(|e| anyhow!("Failed to set language: {}", e))?;

    let tree = parser
        .parse(content, None)
        .ok_or_else(|| anyhow!("Failed to parse file"))?;

    // Create query
    let query = Query::new(&ts_language, query_pattern)
        .map_err(|e| anyhow!("Failed to create query: {}", e))?;

    // Execute query and collect chunks
    let mut chunks = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), content.as_bytes());

    // Iterate through matches
    while let Some(match_) = matches.next() {
        for capture in match_.captures {
            let node = capture.node;

            // Skip nodes that are too large
            if node.byte_range().len() > MAX_CHUNK_SIZE {
                // For large nodes, try to extract smaller sub-nodes
                chunks.extend(extract_subnodes(node, content, file_path, language)?);
            } else if let Some(chunk) = node_to_chunk(node, content, file_path, language)? {
                chunks.push(chunk);
            }
        }
    }

    // If we didn't find any chunks, fall back to simple chunking
    if chunks.is_empty() {
        return Ok(fallback_chunk(file_path, content, language));
    }

    // Add coverage for uncaptured regions if there are significant gaps
    chunks.extend(fill_gaps(&tree, content, file_path, language, &chunks)?);

    Ok(chunks)
}

/// Extract smaller sub-nodes from a large node
fn extract_subnodes(
    node: Node,
    content: &str,
    file_path: &Path,
    language: &str,
) -> Result<Vec<CodeChunk>> {
    let mut chunks = Vec::new();

    // Try to extract child nodes
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.byte_range().len() <= MAX_CHUNK_SIZE
            && let Some(chunk) = node_to_chunk(child, content, file_path, language)? {
                chunks.push(chunk);
            }
    }

    // If no suitable children, fall back to the original node (truncated)
    if chunks.is_empty()
        && let Some(chunk) = node_to_chunk(node, content, file_path, language)? {
            chunks.push(chunk);
        }

    Ok(chunks)
}

/// Convert a tree-sitter node to a CodeChunk
fn node_to_chunk(
    node: Node,
    content: &str,
    file_path: &Path,
    language: &str,
) -> Result<Option<CodeChunk>> {
    let start_byte = node.start_byte();
    let end_byte = node.end_byte();

    // Skip empty nodes
    if start_byte >= end_byte {
        return Ok(None);
    }

    let chunk_content = &content[start_byte..end_byte];

    // Skip whitespace-only chunks
    if chunk_content.trim().is_empty() {
        return Ok(None);
    }

    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;

    Ok(Some(CodeChunk {
        file_path: file_path.to_string_lossy().to_string(),
        start_line,
        end_line,
        content: chunk_content.to_string(),
        language: language.to_string(),
    }))
}

/// Fill gaps between extracted chunks with additional content
fn fill_gaps(
    _tree: &Tree,
    content: &str,
    file_path: &Path,
    language: &str,
    existing_chunks: &[CodeChunk],
) -> Result<Vec<CodeChunk>> {
    let mut gap_chunks = Vec::new();

    // Get byte ranges of existing chunks
    let mut covered_ranges: Vec<(usize, usize)> = existing_chunks
        .iter()
        .map(|chunk| {
            let start = content[..].lines()
                .take(chunk.start_line - 1)
                .map(|l| l.len() + 1)
                .sum::<usize>();
            let end = content[..].lines()
                .take(chunk.end_line)
                .map(|l| l.len() + 1)
                .sum::<usize>();
            (start, end)
        })
        .collect();

    covered_ranges.sort_by_key(|r| r.0);

    // Find gaps
    let mut current_pos = 0;
    for (start, end) in covered_ranges {
        if start > current_pos {
            // There's a gap
            let gap_content = &content[current_pos..start];
            if gap_content.trim().len() > 50 {  // Only include significant gaps
                let start_line = content[..current_pos].matches('\n').count() + 1;
                let end_line = content[..start].matches('\n').count() + 1;

                gap_chunks.push(CodeChunk {
                    file_path: file_path.to_string_lossy().to_string(),
                    start_line,
                    end_line,
                    content: gap_content.to_string(),
                    language: language.to_string(),
                });
            }
        }
        current_pos = current_pos.max(end);
    }

    Ok(gap_chunks)
}

/// Fallback chunking for unsupported languages or parsing errors
fn fallback_chunk(file_path: &Path, content: &str, language: &str) -> Vec<CodeChunk> {
    const CHUNK_SIZE: usize = 1000;
    const CHUNK_OVERLAP: usize = 200;

    let mut chunks = Vec::new();
    let mut current_pos = 0;

    while current_pos < content.len() {
        let remaining = &content[current_pos..];

        // Find a safe chunk end that respects UTF-8 boundaries
        let chunk_end = if remaining.len() > CHUNK_SIZE {
            let safe_end = remaining
                .char_indices()
                .take_while(|(idx, _)| *idx <= CHUNK_SIZE)
                .last()
                .map(|(idx, ch)| idx + ch.len_utf8())
                .unwrap_or(0);

            // Try to find a newline before the safe boundary
            if let Some(pos) = remaining[..safe_end].rfind('\n') {
                current_pos + pos + 1
            } else {
                current_pos + safe_end
            }
        } else {
            content.len()
        };

        let chunk_content = content[current_pos..chunk_end].trim().to_string();

        if !chunk_content.is_empty() {
            let chunk_lines = content[..current_pos].matches('\n').count();
            let end_lines = content[..chunk_end].matches('\n').count();

            chunks.push(CodeChunk {
                file_path: file_path.to_string_lossy().to_string(),
                start_line: chunk_lines + 1,
                end_line: end_lines + 1,
                content: chunk_content,
                language: language.to_string(),
            });
        }

        // Move forward with overlap
        current_pos = if chunk_end < content.len() {
            let target_pos = chunk_end.saturating_sub(CHUNK_OVERLAP);
            content[..chunk_end]
                .char_indices()
                .find(|(idx, _)| *idx >= target_pos)
                .map(|(idx, _)| idx)
                .unwrap_or(chunk_end)
                .max(current_pos + 1)
        } else {
            chunk_end
        };
    }

    chunks
}
