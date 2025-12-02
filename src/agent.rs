use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Agent configuration and personality
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,

    // Personality & behavior
    pub description: Option<String>,
    pub personality: Option<String>,
    pub system_prompt_override: Option<String>,
    pub system_prompt_additions: Option<String>,

    // Memory settings
    pub memory_enabled: bool,
    pub max_routine_memories: usize,
    pub max_key_memories: usize,
    pub auto_summarize: bool,

    // Statistics
    pub total_conversations: usize,
    pub total_messages: usize,
}

impl AgentConfig {
    pub fn new(name: &str) -> Self {
        let now = Utc::now();
        Self {
            name: name.to_string(),
            created_at: now,
            last_active: now,
            description: None,
            personality: None,
            system_prompt_override: None,
            system_prompt_additions: None,
            memory_enabled: true,
            max_routine_memories: 10000,
            max_key_memories: 1000,
            auto_summarize: false,
            total_conversations: 0,
            total_messages: 0,
        }
    }
}

/// Manager for agent lifecycle
pub struct AgentManager {
    agents_dir: PathBuf,
}

impl AgentManager {
    /// Create a new agent manager
    pub fn new() -> Result<Self> {
        let agents_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("Cannot determine home directory"))?
            .join(".agent-t")
            .join("agents");

        std::fs::create_dir_all(&agents_dir)?;

        Ok(Self { agents_dir })
    }

    /// Validate agent name (single word, safe for filesystem)
    pub fn validate_name(name: &str) -> Result<()> {
        if name.is_empty() {
            return Err(anyhow!("Agent name cannot be empty"));
        }

        // Must be single word: letters, numbers, underscore, hyphen
        let re = Regex::new(r"^[a-zA-Z0-9_-]+$")?;
        if !re.is_match(name) {
            return Err(anyhow!(
                "Agent name must be a single word (letters, numbers, _, -)"
            ));
        }

        if name.len() > 50 {
            return Err(anyhow!("Agent name too long (max 50 characters)"));
        }

        // Prevent reserved names
        let reserved = [".", "..", "default", "system", "admin", "root"];
        if reserved.contains(&name.to_lowercase().as_str()) {
            return Err(anyhow!("Agent name '{}' is reserved", name));
        }

        Ok(())
    }

    /// Check if agent exists
    pub fn exists(&self, name: &str) -> bool {
        self.agents_dir.join(name).join("agent.json").exists()
    }

    /// List all agents
    pub fn list_agents(&self) -> Result<Vec<AgentInfo>> {
        let mut agents = Vec::new();

        for entry in std::fs::read_dir(&self.agents_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let config_path = path.join("agent.json");
                if config_path.exists() {
                    let content = std::fs::read_to_string(&config_path)?;
                    if let Ok(config) = serde_json::from_str::<AgentConfig>(&content) {
                        agents.push(AgentInfo {
                            name: config.name,
                            description: config.description,
                            last_active: config.last_active,
                            total_conversations: config.total_conversations,
                            memory_enabled: config.memory_enabled,
                        });
                    }
                }
            }
        }

        // Sort by last active
        agents.sort_by(|a, b| b.last_active.cmp(&a.last_active));

        Ok(agents)
    }

    /// Create a new agent interactively
    pub fn create_agent_interactive(&self, name: &str) -> Result<AgentConfig> {
        Self::validate_name(name)?;

        if self.exists(name) {
            return Err(anyhow!("Agent '{}' already exists", name));
        }

        use crate::terminal;

        terminal::print_info(&format!("Creating new agent '{}'", name));

        // Ask for description
        println!("\nOptional: Describe this agent's role (e.g., 'Rust expert', 'Python helper')");
        print!("Description (press Enter to skip): ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut description = String::new();
        std::io::stdin().read_line(&mut description)?;
        let description = description.trim();
        let description = if description.is_empty() {
            None
        } else {
            Some(description.to_string())
        };

        // Ask for personality
        println!("\nOptional: Set personality traits (e.g., 'friendly and verbose', 'concise')");
        print!("Personality (press Enter to skip): ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut personality = String::new();
        std::io::stdin().read_line(&mut personality)?;
        let personality = personality.trim();
        let personality = if personality.is_empty() {
            None
        } else {
            Some(personality.to_string())
        };

        // Ask about system_prompt.md
        println!("\nOptional: Create a system_prompt.md template for this agent?");
        println!("(This allows you to customize the agent's expertise and behavior)");
        print!("Create system_prompt.md? (y/N): ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut create_prompt = String::new();
        std::io::stdin().read_line(&mut create_prompt)?;
        let create_prompt = create_prompt.trim().to_lowercase();
        let should_create_prompt = create_prompt == "y" || create_prompt == "yes";

        // Create agent config
        let mut config = AgentConfig::new(name);
        config.description = description;
        config.personality = personality;

        // Create directory structure
        let agent_dir = self.agents_dir.join(name);
        std::fs::create_dir_all(agent_dir.join("memory"))?;

        // Create system_prompt.md template if requested
        if should_create_prompt {
            let prompt_template = r#"# {{agent_name}} - Custom Agent

You are {{agent_name}}, a specialized AI assistant.

## Expertise

<!-- Define your areas of expertise here -->
- Technology/language expertise
- Domain knowledge
- Specific skills

## Approach

<!-- Define how this agent should work -->
1. Working style and methodology
2. Code quality standards
3. Communication preferences

## Technology Stack

<!-- Specify preferred technologies, frameworks, and tools -->
- Preferred programming languages
- Frameworks and libraries
- Development tools

## Guidelines

<!-- Add specific rules or preferences -->
- Testing requirements
- Documentation standards
- Code style preferences
- Security considerations

## Response Structure

<!-- Define how responses should be formatted -->
1. Analysis
2. Implementation
3. Testing
4. Documentation

---
**Context Variables Available:**
- {{working_dir}} - Current working directory
- {{project_name}} - Project name
- {{git_branch}} - Git branch
- {{git_status}} - Git status (clean/dirty)
- {{model}} - LLM model name
- {{date}}, {{time}}, {{datetime}} - Timestamps
- {{username}}, {{hostname}}, {{os}} - System info
"#.to_string();

            std::fs::write(agent_dir.join("system_prompt.md"), prompt_template)?;
            terminal::print_success("Created system_prompt.md template");
            println!("Edit ~/.agent-t/agents/{}/system_prompt.md to customize this agent", name);
        }

        // Save config
        let config_json = serde_json::to_string_pretty(&config)?;
        std::fs::write(agent_dir.join("agent.json"), config_json)?;

        terminal::print_success(&format!("Created agent '{}'", name));

        Ok(config)
    }

    /// Load agent config
    pub fn load_agent(&self, name: &str) -> Result<AgentConfig> {
        let config_path = self.agents_dir.join(name).join("agent.json");

        if !config_path.exists() {
            return Err(anyhow!("Agent '{}' not found", name));
        }

        let content = std::fs::read_to_string(&config_path)?;
        let config: AgentConfig = serde_json::from_str(&content)?;

        Ok(config)
    }

    /// Save agent config
    pub fn save_agent(&self, config: &AgentConfig) -> Result<()> {
        let config_path = self.agents_dir.join(&config.name).join("agent.json");
        let config_json = serde_json::to_string_pretty(config)?;
        std::fs::write(config_path, config_json)?;
        Ok(())
    }

    /// Update agent's last active time
    pub fn update_last_active(&self, name: &str) -> Result<()> {
        let mut config = self.load_agent(name)?;
        config.last_active = Utc::now();
        self.save_agent(&config)?;
        Ok(())
    }

    /// Get agent directory path
    pub fn agent_dir(&self, name: &str) -> PathBuf {
        self.agents_dir.join(name)
    }

    /// Delete an agent
    pub fn delete_agent(&self, name: &str) -> Result<()> {
        let agent_dir = self.agent_dir(name);

        if !agent_dir.exists() {
            return Err(anyhow!("Agent '{}' not found", name));
        }

        std::fs::remove_dir_all(&agent_dir)?;
        Ok(())
    }
}

/// Summary information about an agent for listing
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    pub description: Option<String>,
    pub last_active: DateTime<Utc>,
    pub total_conversations: usize,
    pub memory_enabled: bool,
}

impl std::fmt::Display for AgentInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let desc = self.description.as_deref().unwrap_or("No description");
        let active = self.last_active.format("%Y-%m-%d %H:%M");
        let memory = if self.memory_enabled { "📝" } else { "  " };

        write!(
            f,
            "{} {} - {} ({} convos, last active: {})",
            memory, self.name, desc, self.total_conversations, active
        )
    }
}

/// Build system prompt with agent personality
/// If agent_file_prompt is provided, it takes precedence over agent.json fields
pub fn build_system_prompt(
    agent_config: &AgentConfig,
    base_prompt: &str,
    agent_file_prompt: Option<&str>
) -> String {
    // If there's a file-based prompt, use it (takes precedence over agent.json)
    if let Some(file_prompt) = agent_file_prompt {
        let mut prompt = base_prompt.to_string();
        prompt.push_str("\n\n");
        prompt.push_str(file_prompt);
        return prompt;
    }

    // Use full override if specified
    if let Some(override_prompt) = &agent_config.system_prompt_override {
        return override_prompt.clone();
    }

    let mut prompt = base_prompt.to_string();

    // Add agent identity
    prompt.push_str(&format!("\n\n# Agent Identity\n\nYou are {}", agent_config.name));

    if let Some(desc) = &agent_config.description {
        prompt.push_str(&format!(", {}", desc));
    }
    prompt.push('.');

    // Add personality
    if let Some(personality) = &agent_config.personality {
        prompt.push_str(&format!("\n\nPersonality: {}", personality));
    }

    // Add custom additions
    if let Some(additions) = &agent_config.system_prompt_additions {
        prompt.push_str(&format!("\n\n{}", additions));
    }

    prompt
}

/// Load agent-specific system prompt from file if it exists
/// Returns Ok(None) if file doesn't exist or is empty
/// Returns Ok(Some(content)) if file exists and has content
/// Returns Err if file exists but cannot be read
pub fn load_agent_system_prompt(agent_manager: &AgentManager, agent_name: &str) -> Result<Option<String>> {
    let prompt_path = agent_manager.agent_dir(agent_name).join("system_prompt.md");

    if !prompt_path.exists() {
        return Ok(None);
    }

    match std::fs::read_to_string(&prompt_path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                Ok(None)  // Empty file treated as non-existent
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Err(e) => Err(anyhow!(
            "Failed to read system_prompt.md for agent '{}': {}",
            agent_name,
            e
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt_with_file() {
        let config = AgentConfig::new("test");
        let base = "Base prompt";
        let file_content = "File-based additions\nWith multiple lines";

        let result = build_system_prompt(&config, base, Some(file_content));

        assert!(result.contains("Base prompt"));
        assert!(result.contains("File-based additions"));
        assert!(result.contains("With multiple lines"));
        assert!(!result.contains("Agent Identity")); // Should skip agent.json fields
    }

    #[test]
    fn test_build_system_prompt_without_file() {
        let mut config = AgentConfig::new("test");
        config.personality = Some("Friendly".to_string());
        config.description = Some("Test agent".to_string());
        let base = "Base prompt";

        let result = build_system_prompt(&config, base, None);

        assert!(result.contains("Base prompt"));
        assert!(result.contains("Agent Identity"));
        assert!(result.contains("test"));
        assert!(result.contains("Test agent"));
        assert!(result.contains("Friendly"));
    }

    #[test]
    fn test_build_system_prompt_file_precedence() {
        let mut config = AgentConfig::new("test");
        config.system_prompt_additions = Some("JSON additions".to_string());
        config.personality = Some("Friendly".to_string());
        let base = "Base prompt";
        let file_content = "File content";

        let result = build_system_prompt(&config, base, Some(file_content));

        assert!(result.contains("Base prompt"));
        assert!(result.contains("File content"));
        assert!(!result.contains("JSON additions")); // File takes precedence
        assert!(!result.contains("Friendly"));
    }

    #[test]
    fn test_build_system_prompt_system_override_without_file() {
        let mut config = AgentConfig::new("test");
        config.system_prompt_override = Some("Complete override".to_string());
        let base = "Base prompt";

        let result = build_system_prompt(&config, base, None);

        assert_eq!(result, "Complete override");
        assert!(!result.contains("Base prompt"));
    }

    #[test]
    fn test_build_system_prompt_file_over_override() {
        let mut config = AgentConfig::new("test");
        config.system_prompt_override = Some("Complete override".to_string());
        let base = "Base prompt";
        let file_content = "File wins";

        let result = build_system_prompt(&config, base, Some(file_content));

        assert!(result.contains("Base prompt"));
        assert!(result.contains("File wins"));
        assert!(!result.contains("Complete override")); // File takes precedence
    }
}
