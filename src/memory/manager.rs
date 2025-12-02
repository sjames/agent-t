use anyhow::{anyhow, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use ruvector_core::{DistanceMetric, SearchQuery, VectorDB as RuVectorDB, VectorEntry};
use ruvector_core::types::{DbOptions, HnswConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::types::{ImportanceLevel, KeyMemoryChunk, MemoryCategory, RoutineMemoryChunk};

/// Manager for long-term memory (routine and key memories)
pub struct MemoryManager {
    agent_name: String,
    memory_dir: PathBuf,

    // Routine memory (automatic conversation history)
    routine_db: Option<RuVectorDB>,
    routine_chunks: Vec<RoutineMemoryChunk>,

    // Key memory (LLM-curated important facts)
    key_db: Option<RuVectorDB>,
    key_chunks: Vec<KeyMemoryChunk>,

    // Local embedding model
    embedding_model: TextEmbedding,
    dimension: usize,
}

impl MemoryManager {
    /// Create a new memory manager for an agent
    pub fn new(agent_name: &str, embedding_model_name: &str) -> Result<Self> {
        let memory_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("Cannot determine home directory"))?
            .join(".agent-t")
            .join("agents")
            .join(agent_name)
            .join("memory");

        std::fs::create_dir_all(&memory_dir)?;

        // Initialize local embedding model
        let model = match embedding_model_name {
            "BAAI/bge-small-en-v1.5" => EmbeddingModel::BGESmallENV15,
            "BAAI/bge-base-en-v1.5" => EmbeddingModel::BGEBaseENV15,
            "sentence-transformers/all-MiniLM-L6-v2" => EmbeddingModel::AllMiniLML6V2,
            _ => {
                eprintln!(
                    "Warning: Unknown model '{}', defaulting to BAAI/bge-small-en-v1.5",
                    embedding_model_name
                );
                EmbeddingModel::BGESmallENV15
            }
        };

        // Get embedding dimension before moving model
        let dimension = match model {
            EmbeddingModel::BGESmallENV15 | EmbeddingModel::AllMiniLML6V2 => 384,
            EmbeddingModel::BGEBaseENV15 => 768,
            _ => 384,
        };

        // Set custom cache directory within ~/.agent-t
        let cache_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("Cannot determine home directory"))?
            .join(".agent-t")
            .join("fastembed_cache");

        std::fs::create_dir_all(&cache_dir)?;

        let init_options = InitOptions::new(model)
            .with_cache_dir(cache_dir)
            .with_show_download_progress(true);

        let embedding_model = TextEmbedding::try_new(init_options)?;

        Ok(Self {
            agent_name: agent_name.to_string(),
            memory_dir,
            routine_db: None,
            routine_chunks: Vec::new(),
            key_db: None,
            key_chunks: Vec::new(),
            embedding_model,
            dimension,
        })
    }

    /// Load or initialize memory databases
    pub async fn load_or_initialize(&mut self) -> Result<()> {
        // Load or create routine memory
        let routine_db_path = self.memory_dir.join("routine.db");
        let routine_chunks_path = self.memory_dir.join("routine_chunks.json");

        if routine_db_path.exists() && routine_chunks_path.exists() {
            // Load existing
            let chunks_json = std::fs::read_to_string(&routine_chunks_path)?;
            self.routine_chunks = serde_json::from_str(&chunks_json)?;

            let options = DbOptions {
                dimensions: self.dimension,
                distance_metric: DistanceMetric::Cosine,
                storage_path: routine_db_path.to_string_lossy().to_string(),
                hnsw_config: Some(HnswConfig::default()),
                quantization: None,
            };
            self.routine_db = Some(RuVectorDB::new(options)?);
        } else {
            // Create new
            let options = DbOptions {
                dimensions: self.dimension,
                distance_metric: DistanceMetric::Cosine,
                storage_path: routine_db_path.to_string_lossy().to_string(),
                hnsw_config: Some(HnswConfig {
                    m: 16,
                    ef_construction: 200,
                    ef_search: 100,
                    max_elements: 10000,
                }),
                quantization: None,
            };
            self.routine_db = Some(RuVectorDB::new(options)?);
            self.routine_chunks = Vec::new();
        }

        // Load or create key memory
        let key_db_path = self.memory_dir.join("key.db");
        let key_chunks_path = self.memory_dir.join("key_chunks.json");

        if key_db_path.exists() && key_chunks_path.exists() {
            // Load existing
            let chunks_json = std::fs::read_to_string(&key_chunks_path)?;
            self.key_chunks = serde_json::from_str(&chunks_json)?;

            let options = DbOptions {
                dimensions: self.dimension,
                distance_metric: DistanceMetric::Cosine,
                storage_path: key_db_path.to_string_lossy().to_string(),
                hnsw_config: Some(HnswConfig::default()),
                quantization: None,
            };
            self.key_db = Some(RuVectorDB::new(options)?);
        } else {
            // Create new
            let options = DbOptions {
                dimensions: self.dimension,
                distance_metric: DistanceMetric::Cosine,
                storage_path: key_db_path.to_string_lossy().to_string(),
                hnsw_config: Some(HnswConfig {
                    m: 16,
                    ef_construction: 200,
                    ef_search: 100,
                    max_elements: 1000,
                }),
                quantization: None,
            };
            self.key_db = Some(RuVectorDB::new(options)?);
            self.key_chunks = Vec::new();
        }

        Ok(())
    }

    /// Generate embeddings locally
    fn embed_texts(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let embeddings = self.embedding_model.embed(texts.to_vec(), None)?;
        Ok(embeddings)
    }

    /// Store a routine memory (automatic, from chat)
    pub fn store_routine_memory(&mut self, chunk: RoutineMemoryChunk) -> Result<()> {
        // Generate embedding first (requires mutable borrow)
        let embedding = self.embed_texts(&[chunk.content.clone()])?;

        // Store in vector DB
        let idx = self.routine_chunks.len();
        let entry = VectorEntry {
            id: Some(idx.to_string()),
            vector: embedding[0].clone(),
            metadata: None,
        };

        let db = self
            .routine_db
            .as_ref()
            .ok_or_else(|| anyhow!("Routine memory DB not initialized"))?;

        db.insert(entry)?;

        // Store chunk metadata
        self.routine_chunks.push(chunk);
        self.save_routine_chunks()?;

        Ok(())
    }

    /// Store a key memory (LLM-curated)
    pub fn store_key_memory(&mut self, chunk: KeyMemoryChunk) -> Result<()> {
        // Generate embedding first (requires mutable borrow)
        let embedding = self.embed_texts(&[chunk.content.clone()])?;

        // Store in vector DB
        let idx = self.key_chunks.len();
        let entry = VectorEntry {
            id: Some(idx.to_string()),
            vector: embedding[0].clone(),
            metadata: None,
        };

        let db = self
            .key_db
            .as_ref()
            .ok_or_else(|| anyhow!("Key memory DB not initialized"))?;

        db.insert(entry)?;

        // Store chunk metadata
        self.key_chunks.push(chunk);
        self.save_key_chunks()?;

        Ok(())
    }

    /// Search routine memories
    pub fn search_routine(
        &mut self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<(RoutineMemoryChunk, f32)>> {
        // Generate query embedding first (requires mutable borrow)
        let query_embedding = self.embed_texts(&[query.to_string()])?;

        // Now get immutable db reference
        let db = self
            .routine_db
            .as_ref()
            .ok_or_else(|| anyhow!("Routine memory DB not initialized"))?;

        let search_query = SearchQuery {
            vector: query_embedding[0].clone(),
            k: top_k,
            filter: None,
            ef_search: None,
        };

        let results = db.search(search_query)?;

        let mut memories = Vec::new();
        for result in results {
            if let Ok(idx) = result.id.parse::<usize>()
                && idx < self.routine_chunks.len() {
                    memories.push((self.routine_chunks[idx].clone(), result.score));
                }
        }

        Ok(memories)
    }

    /// Search key memories with optional filtering
    pub fn search_key(
        &mut self,
        query: &str,
        top_k: usize,
        categories: Option<Vec<MemoryCategory>>,
        min_importance: Option<ImportanceLevel>,
    ) -> Result<Vec<(KeyMemoryChunk, f32)>> {
        // Generate query embedding first (requires mutable borrow)
        let query_embedding = self.embed_texts(&[query.to_string()])?;

        // Now get immutable db reference
        let db = self
            .key_db
            .as_ref()
            .ok_or_else(|| anyhow!("Key memory DB not initialized"))?;

        // Get more results for filtering
        let search_query = SearchQuery {
            vector: query_embedding[0].clone(),
            k: top_k * 3,
            filter: None,
            ef_search: None,
        };

        let results = db.search(search_query)?;

        let mut memories = Vec::new();
        for result in results {
            if let Ok(idx) = result.id.parse::<usize>()
                && idx < self.key_chunks.len() {
                    let chunk = &self.key_chunks[idx];

                    // Apply category filter
                    if let Some(ref cats) = categories
                        && !cats.contains(&chunk.category) {
                            continue;
                        }

                    // Apply importance filter
                    if let Some(ref min_imp) = min_importance
                        && chunk.importance < *min_imp {
                            continue;
                        }

                    memories.push((chunk.clone(), result.score));

                    if memories.len() >= top_k {
                        break;
                    }
                }
        }

        Ok(memories)
    }

    /// Save routine chunks metadata
    fn save_routine_chunks(&self) -> Result<()> {
        let path = self.memory_dir.join("routine_chunks.json");
        let json = serde_json::to_string(&self.routine_chunks)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Save key chunks metadata
    fn save_key_chunks(&self) -> Result<()> {
        let path = self.memory_dir.join("key_chunks.json");
        let json = serde_json::to_string(&self.key_chunks)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Get memory statistics
    pub fn stats(&self) -> MemoryStats {
        MemoryStats {
            routine_count: self.routine_chunks.len(),
            key_count: self.key_chunks.len(),
            agent_name: self.agent_name.clone(),
        }
    }

    /// Get the most recent session summary
    pub fn get_last_session_summary(&self) -> Option<KeyMemoryChunk> {
        // Find the most recent SessionSummary
        self.key_chunks
            .iter()
            .filter(|chunk| chunk.category == MemoryCategory::SessionSummary)
            .max_by_key(|chunk| chunk.timestamp)
            .cloned()
    }

    /// Explicitly flush memory to disk (useful before shutdown)
    pub fn flush(&self) -> Result<()> {
        // Save routine chunks
        if let Err(e) = self.save_routine_chunks() {
            eprintln!("[WARN] Failed to flush routine memory: {}", e);
        }

        // Save key chunks
        if let Err(e) = self.save_key_chunks() {
            eprintln!("[WARN] Failed to flush key memory: {}", e);
        }

        // Note: VectorDB uses memory-mapped files that auto-persist,
        // so no explicit flush needed for the vector indices
        Ok(())
    }
}

// Manual Debug implementation since TextEmbedding and VectorDB don't implement Debug
impl std::fmt::Debug for MemoryManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryManager")
            .field("agent_name", &self.agent_name)
            .field("memory_dir", &self.memory_dir)
            .field("routine_chunks", &self.routine_chunks.len())
            .field("key_chunks", &self.key_chunks.len())
            .field("dimension", &self.dimension)
            .finish()
    }
}

// Ensure clean shutdown when MemoryManager is dropped
impl Drop for MemoryManager {
    fn drop(&mut self) {
        // Flush memory to disk on drop
        if let Err(e) = self.flush() {
            eprintln!("[ERROR] Failed to flush memory on shutdown: {}", e);
        } else {
            eprintln!("[INFO] Memory flushed successfully ({} routine, {} key memories)",
                self.routine_chunks.len(), self.key_chunks.len());
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    pub routine_count: usize,
    pub key_count: usize,
    pub agent_name: String,
}
