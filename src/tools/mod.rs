//! Tools module for the terminal coding agent
//!
//! This module contains implementations of various tools that the agent
//! can use to interact with the filesystem, execute commands, and more.

mod read_file;
mod write_file;
mod list_dir;
mod bash;
mod edit_file;
mod grep;
mod glob_files;
mod bash_status;
mod bash_output;
mod bash_kill;
mod bash_list;
mod web_fetch;
mod web_search;
mod math_calc;

// Memory tools
mod store_key_memory;
mod search_routine_memory;
mod search_key_memory;

// Rust Analyzer tools
pub mod ra_common;
mod ra_diagnostics;
mod ra_goto_definition;
mod ra_find_references;
mod ra_hover;
mod ra_symbols;
mod ra_completion;
mod ra_code_actions;
mod ra_rename;
mod ra_format;

pub use read_file::ReadFile;
pub use write_file::WriteFile;
pub use list_dir::ListDir;
pub use bash::BashCommand;
pub use edit_file::EditFile;
pub use grep::GrepSearch;
pub use glob_files::GlobFiles;
pub use bash_status::BashStatus;
pub use bash_output::BashOutput;
pub use bash_kill::BashKill;
pub use bash_list::BashList;
pub use web_fetch::WebFetch;
pub use web_search::WebSearch;
pub use math_calc::MathCalc;

// Memory tools
pub use store_key_memory::StoreKeyMemory;
pub use search_routine_memory::SearchRoutineMemory;
pub use search_key_memory::SearchKeyMemory;

// Rust Analyzer tools
pub use ra_diagnostics::RaDiagnostics;
pub use ra_goto_definition::RaGotoDefinition;
pub use ra_find_references::RaFindReferences;
pub use ra_hover::RaHover;
pub use ra_symbols::RaSymbols;
pub use ra_completion::RaCompletion;
pub use ra_code_actions::RaCodeActions;
pub use ra_rename::RaRename;
pub use ra_format::RaFormat;
