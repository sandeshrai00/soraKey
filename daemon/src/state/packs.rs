use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// V2 key definition
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyDefinition {
    pub timing: Vec<[f32; 2]>, // Array of [start_ms, end_ms] pairs
    #[serde(default)]
    pub audio_file: Option<String>, // For "multi" definition method
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundPack {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub audio_file: Option<String>, // Used only in "single" definition_method
    pub definition_method: String, // "single" or "multi"
    pub definitions: HashMap<String, KeyDefinition>,
}
