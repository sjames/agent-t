use anyhow::{anyhow, Result};
use rig::client::EmbeddingsClient;
use rig::embeddings::EmbeddingModel as _;
use rig::providers::ollama;
use ruvector_core::{VectorDB as RuVectorDB, VectorEntry, SearchQuery, DistanceMetric};
use ruvector_core::types::{DbOptions, HnswConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// A code chunk with its metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeChunk {
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub language: String,
}

type OllamaEmbedder = ollama::EmbeddingModel<reqwest::Client>;

/// Vector database for code context
pub struct VectorDB {
    /// Mapping from vector index to code chunk
    chunks: Vec<CodeChunk>,
    /// Embedding model
    embedding_model: OllamaEmbedder,
    /// Database directory
    db_dir: PathBuf,
    /// Embedding dimension
    dimension: usize,
    /// ruvector-core database instance
    ruvector_db: Option<RuVectorDB>,
}

impl VectorDB {
    /// Create a new vector database
    pub fn new(ollama_url: Option<&str>, embedding_model_name: &str) -> Result<Self> {
        // Get database directory (~/.agent-t/)
        let db_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("Cannot determine home directory"))?
            .join(".agent-t");

        // Create directory if it doesn't exist
        std::fs::create_dir_all(&db_dir)?;

        // Create Ollama client
        let ollama_client = if let Some(url) = ollama_url {
            ollama::Client::builder().base_url(url).build()
        } else {
            ollama::Client::new()
        };

        let embedding_model = ollama_client.embedding_model(embedding_model_name);

        Ok(Self {
            chunks: Vec::new(),
            embedding_model,
            db_dir,
            dimension: 768, // Default for nomic-embed-text
            ruvector_db: None,
        })
    }

    /// Check if index exists
    pub fn index_exists(&self) -> bool {
        self.db_dir.join("ruvector.db").exists()
            && self.db_dir.join("chunks.json").exists()
    }

    /// Load existing index
    pub async fn load_index(&mut self) -> Result<()> {
        let chunks_path = self.db_dir.join("chunks.json");
        let db_path = self.db_dir.join("ruvector.db");

        if !chunks_path.exists() {
            return Err(anyhow!("Chunks metadata does not exist"));
        }

        if !db_path.exists() {
            return Err(anyhow!("Vector database does not exist"));
        }

        // Load chunks metadata
        let chunks_json = std::fs::read_to_string(&chunks_path)?;
        self.chunks = serde_json::from_str(&chunks_json)?;

        // Create ruvector database with existing storage path
        let options = DbOptions {
            dimensions: self.dimension,
            distance_metric: DistanceMetric::Cosine,
            storage_path: db_path.to_string_lossy().to_string(),
            hnsw_config: Some(HnswConfig::default()),
            quantization: None,
        };
        self.ruvector_db = Some(RuVectorDB::new(options)?);

        Ok(())
    }

    /// Save chunks metadata to disk
    fn save_chunks(&self) -> Result<()> {
        let chunks_path = self.db_dir.join("chunks.json");
        let chunks_json = serde_json::to_string(&self.chunks)?;
        std::fs::write(&chunks_path, chunks_json)?;
        Ok(())
    }

    /// Index a directory containing code files
    pub async fn index_directory(&mut self, dir_path: &str) -> Result<usize> {
        use crate::terminal;

        // Supported file extensions
        let supported_extensions = vec![
            "rs", "cpp", "cc", "cxx", "c", "h", "hpp",
            "js", "jsx", "ts", "tsx", "py", "java",
            "go", "rb", "php", "swift", "kt",
            "html", "css", "md", "txt", "toml", "yaml", "yml", "json",
        ];

        // First pass: collect all files to process
        let mut files_to_process = Vec::new();
        for entry in WalkDir::new(dir_path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                // Skip hidden directories and common build/dependency directories
                let name = e.file_name().to_string_lossy();
                !name.starts_with('.')
                    && name != "target"
                    && name != "node_modules"
                    && name != "dist"
                    && name != "build"
            })
        {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path();
            if let Some(ext) = path.extension()
                && supported_extensions.contains(&ext.to_string_lossy().as_ref()) {
                    files_to_process.push(path.to_path_buf());
                }
        }

        if files_to_process.is_empty() {
            return Err(anyhow!("No code files found to index"));
        }

        // Create progress bar for file processing
        let pb = terminal::create_indexing_progress(files_to_process.len() as u64);
        pb.set_message("Scanning files...");

        let mut all_chunks = Vec::new();

        // Second pass: process files with progress
        for (idx, path) in files_to_process.iter().enumerate() {
            let file_name = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            pb.set_message(format!("Processing: {}", file_name));

            // Read and chunk the file
            if let Ok(chunks) = self.chunk_file(path).await {
                all_chunks.extend(chunks);
            }

            pb.set_position((idx + 1) as u64);
        }

        pb.finish_with_message(format!("Processed {} files, created {} chunks", files_to_process.len(), all_chunks.len()));

        if all_chunks.is_empty() {
            return Err(anyhow!("No code files found to index"));
        }

        let num_chunks = all_chunks.len();

        // Generate embeddings for all chunks
        let texts: Vec<String> = all_chunks.iter().map(|c| c.content.clone()).collect();

        // Create progress bar for embedding generation
        let embed_pb = terminal::create_embedding_progress(num_chunks as u64);
        embed_pb.set_message("Generating embeddings...");

        let embeddings = self.embed_texts_with_progress(&texts, &embed_pb).await?;

        embed_pb.finish_with_message(format!("Generated {} embeddings", num_chunks));

        // Create ruvector database
        let db_path = self.db_dir.join("ruvector.db");
        let options = DbOptions {
            dimensions: self.dimension,
            distance_metric: DistanceMetric::Cosine,
            storage_path: db_path.to_string_lossy().to_string(),
            hnsw_config: Some(HnswConfig {
                m: 16,
                ef_construction: 200,
                ef_search: 100,
                max_elements: num_chunks,
            }),
            quantization: None,
        };
        let ruvector_db = RuVectorDB::new(options)?;

        // Insert embeddings into index with progress
        let insert_pb = terminal::create_indexing_progress(num_chunks as u64);
        insert_pb.set_message("Building vector index...");

        for (idx, embedding) in embeddings.iter().enumerate() {
            let entry = VectorEntry {
                id: Some(idx.to_string()),
                vector: embedding.clone(),
                metadata: None,
            };
            ruvector_db.insert(entry)?;

            if idx % 100 == 0 || idx == embeddings.len() - 1 {
                insert_pb.set_position((idx + 1) as u64);
            }
        }

        insert_pb.finish_with_message(format!("Built vector index with {} vectors", num_chunks));

        // Database is automatically persisted to disk via storage_path
        terminal::print_success(&format!("Index saved to {}", self.db_dir.display()));

        self.chunks = all_chunks;
        self.ruvector_db = Some(ruvector_db);

        // Save chunks metadata
        self.save_chunks()?;

        Ok(num_chunks)
    }

    /// Chunk a file into smaller pieces using tree-sitter
    async fn chunk_file(&self, file_path: &Path) -> Result<Vec<CodeChunk>> {
        let content = std::fs::read_to_string(file_path)?;
        let language = self.detect_language(file_path);

        if content.is_empty() {
            return Ok(Vec::new());
        }

        // Use tree-sitter to intelligently chunk the code
        crate::tree_sitter_chunker::chunk_code_with_tree_sitter(file_path, &content, &language)
    }

    /// Detect programming language from file extension
    fn detect_language(&self, file_path: &Path) -> String {
        if let Some(ext) = file_path.extension() {
            match ext.to_string_lossy().as_ref() {
                "rs" => "Rust",
                "cpp" | "cc" | "cxx" => "C++",
                "c" => "C",
                "h" | "hpp" => "C/C++ Header",
                "js" | "jsx" => "JavaScript",
                "ts" | "tsx" => "TypeScript",
                "py" => "Python",
                "java" => "Java",
                "go" => "Go",
                "rb" => "Ruby",
                "php" => "PHP",
                "swift" => "Swift",
                "kt" => "Kotlin",
                "html" => "HTML",
                "css" => "CSS",
                "md" => "Markdown",
                "txt" => "Text",
                "toml" => "TOML",
                "yaml" | "yml" => "YAML",
                "json" => "JSON",
                _ => "Unknown",
            }
            .to_string()
        } else {
            "Unknown".to_string()
        }
    }

    /// Generate embeddings for texts with progress bar
    async fn embed_texts_with_progress(&self, texts: &[String], pb: &indicatif::ProgressBar) -> Result<Vec<Vec<f32>>> {
        // Process in batches to avoid overwhelming the embedding model
        const BATCH_SIZE: usize = 32;
        let mut all_embeddings = Vec::new();
        let mut processed = 0;

        for batch in texts.chunks(BATCH_SIZE) {
            pb.set_message(format!("Embedding batch {}/{}", processed / BATCH_SIZE + 1, texts.len().div_ceil(BATCH_SIZE)));

            let batch_embeddings = self.embedding_model
                .embed_texts(batch.to_vec())
                .await
                .map_err(|e| anyhow!("Failed to generate embeddings: {}", e))?;

            for embedding in batch_embeddings {
                // Convert f64 to f32
                let vec_f32: Vec<f32> = embedding.vec.iter().map(|&x| x as f32).collect();
                all_embeddings.push(vec_f32);
                processed += 1;
                pb.set_position(processed as u64);
            }
        }

        Ok(all_embeddings)
    }

    /// Search for relevant code chunks
    pub async fn search(&self, query: &str, top_k: usize) -> Result<Vec<(CodeChunk, f32)>> {
        let ruvector_db = self.ruvector_db.as_ref()
            .ok_or_else(|| anyhow!("Vector database not initialized"))?;

        // Generate embedding for query
        let query_embedding = self.embedding_model
            .embed_texts(vec![query.to_string()])
            .await
            .map_err(|e| anyhow!("Failed to generate query embedding: {}", e))?;

        if query_embedding.is_empty() {
            return Err(anyhow!("No embedding generated for query"));
        }

        // Convert f64 to f32
        let query_vec: Vec<f32> = query_embedding[0].vec.iter().map(|&x| x as f32).collect();

        // Create search query
        let search_query = SearchQuery {
            vector: query_vec,
            k: top_k,
            filter: None,
            ef_search: None,
        };

        // Search in ruvector database
        let search_results = ruvector_db.search(search_query)?;

        // Convert results to code chunks with scores
        let mut results = Vec::new();
        for result in search_results {
            // Parse the ID back to an index
            if let Ok(idx) = result.id.parse::<usize>()
                && idx < self.chunks.len() {
                    let chunk = self.chunks[idx].clone();
                    // ruvector-core returns similarity scores (higher is better)
                    results.push((chunk, result.score));
                }
        }

        Ok(results)
    }

    /// Get database statistics
    pub fn stats(&self) -> HashMap<String, String> {
        let mut stats = HashMap::new();
        stats.insert("chunks".to_string(), self.chunks.len().to_string());
        stats.insert("db_dir".to_string(), self.db_dir.to_string_lossy().to_string());
        stats.insert("indexed".to_string(), "yes".to_string());
        stats
    }
}
