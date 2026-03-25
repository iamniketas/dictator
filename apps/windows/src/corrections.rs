use crate::config::CorrectionsConfig;
use anyhow::Result;
use hardware_profiler::default_audio_models_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionEntry {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub hits: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CandidateEntry {
    pub from: String,
    pub to: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CorrectionsDoc {
    pub schema_version: String,
    pub updated_at: String,
    #[serde(default)]
    pub entries: Vec<CorrectionEntry>,
    #[serde(default)]
    pub auto_candidates: Vec<CandidateEntry>,
}

impl Default for CorrectionsDoc {
    fn default() -> Self {
        Self {
            schema_version: "dictator_corrections.v1".to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            entries: vec![],
            auto_candidates: vec![],
        }
    }
}

struct CorrectionState {
    doc: CorrectionsDoc,
    index: HashMap<String, usize>,
}

pub struct CorrectionsManager {
    config: CorrectionsConfig,
    path: PathBuf,
    state: RwLock<CorrectionState>,
}

impl CorrectionsManager {
    pub fn new(config: &CorrectionsConfig) -> Result<Self> {
        let path = resolve_dictionary_path(config);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let doc = load_or_create(&path)?;
        let state = CorrectionState {
            index: build_index(&doc.entries),
            doc,
        };
        Ok(Self {
            config: config.clone(),
            path,
            state: RwLock::new(state),
        })
    }

    pub fn dictionary_path(&self) -> &Path {
        &self.path
    }

    pub fn apply(&self, text: &str) -> String {
        if !self.config.enabled || text.trim().is_empty() {
            return text.to_string();
        }
        let Ok(mut state) = self.state.write() else {
            return text.to_string();
        };

        let mut replaced = 0_u64;
        let mut out_tokens = Vec::with_capacity(text.split_whitespace().count());
        for token in text.split_whitespace() {
            let (prefix, core, suffix) = split_token(token);
            if core.is_empty() {
                out_tokens.push(token.to_string());
                continue;
            }
            let key = normalize(core);
            if let Some(idx) = state.index.get(&key).copied() {
                let entry = &mut state.doc.entries[idx];
                entry.hits = entry.hits.saturating_add(1);
                replaced = replaced.saturating_add(1);
                out_tokens.push(format!("{}{}{}", prefix, entry.to, suffix));
            } else {
                out_tokens.push(token.to_string());
            }
        }

        if replaced > 0 {
            state.doc.updated_at = chrono::Utc::now().to_rfc3339();
            if let Err(e) = save_doc(&self.path, &state.doc) {
                warn!("[CORRECTIONS] failed to persist hits: {}", e);
            }
        }
        out_tokens.join(" ")
    }

    pub fn learn_from_pair(&self, original: &str, corrected: &str) {
        if !self.config.auto_learn || original.trim().is_empty() || corrected.trim().is_empty() {
            return;
        }

        let original_tokens = normalized_words(original);
        let corrected_tokens = normalized_words(corrected);
        if original_tokens.len() != corrected_tokens.len() {
            return;
        }

        let Ok(mut state) = self.state.write() else {
            return;
        };

        let mut changed = false;
        let mut candidate_map: HashMap<(String, String), usize> = state
            .doc
            .auto_candidates
            .iter()
            .enumerate()
            .map(|(idx, c)| ((c.from.clone(), c.to.clone()), idx))
            .collect();

        for (from, to) in original_tokens.iter().zip(corrected_tokens.iter()) {
            if from == to || from.len() < 3 || to.len() < 3 {
                continue;
            }

            let key = (from.clone(), to.clone());
            if let Some(idx) = candidate_map.get(&key).copied() {
                let candidate = &mut state.doc.auto_candidates[idx];
                candidate.count = candidate.count.saturating_add(1);
            } else {
                state.doc.auto_candidates.push(CandidateEntry {
                    from: from.clone(),
                    to: to.clone(),
                    count: 1,
                });
                candidate_map.insert(key, state.doc.auto_candidates.len() - 1);
            }
            changed = true;
        }

        let min_repeats = self.config.min_auto_learn_repeats.max(2);
        let to_promote: Vec<(String, String)> = state
            .doc
            .auto_candidates
            .iter()
            .filter(|c| c.count >= min_repeats && !state.index.contains_key(&c.from))
            .map(|c| (c.from.clone(), c.to.clone()))
            .collect();

        let mut promoted = false;
        for (from, to) in to_promote {
            state
                .doc
                .entries
                .push(CorrectionEntry { from, to, hits: 0 });
            promoted = true;
        }

        if promoted {
            state.index = build_index(&state.doc.entries);
            changed = true;
        }

        if changed {
            state.doc.updated_at = chrono::Utc::now().to_rfc3339();
            if let Err(e) = save_doc(&self.path, &state.doc) {
                warn!("[CORRECTIONS] failed to persist dictionary: {}", e);
            }
        }
    }
}

fn resolve_dictionary_path(config: &CorrectionsConfig) -> PathBuf {
    if let Some(path) = &config.dictionary_path {
        return path.clone();
    }
    default_audio_models_dir().join("shared_corrections.v1.json")
}

fn load_or_create(path: &Path) -> Result<CorrectionsDoc> {
    if path.exists() {
        let raw = fs::read_to_string(path)?;
        let doc = serde_json::from_str::<CorrectionsDoc>(&raw).unwrap_or_default();
        return Ok(doc);
    }
    let doc = CorrectionsDoc::default();
    save_doc(path, &doc)?;
    Ok(doc)
}

fn save_doc(path: &Path, doc: &CorrectionsDoc) -> Result<()> {
    let mut tmp = path.to_path_buf();
    tmp.set_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(doc)?)?;
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(tmp, path)?;
    Ok(())
}

fn normalize(word: &str) -> String {
    word.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
}

fn normalized_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(normalize)
        .filter(|w| !w.is_empty())
        .collect()
}

fn build_index(entries: &[CorrectionEntry]) -> HashMap<String, usize> {
    let mut index = HashMap::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        index.insert(normalize(&e.from), i);
    }
    index
}

fn split_token(token: &str) -> (&str, &str, &str) {
    let start = token
        .char_indices()
        .find(|(_, ch)| ch.is_alphanumeric())
        .map(|(idx, _)| idx)
        .unwrap_or(token.len());

    let end = token
        .char_indices()
        .rfind(|(_, ch)| ch.is_alphanumeric())
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);

    if start >= end {
        return ("", "", token);
    }

    (&token[..start], &token[start..end], &token[end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_word_with_punctuation() {
        let (p, c, s) = split_token("\"GPU,");
        assert_eq!(p, "\"");
        assert_eq!(c, "GPU");
        assert_eq!(s, ",");
    }

    #[test]
    fn normalize_unicode() {
        assert_eq!(normalize("ДжИПИУ!"), "джипиу");
        assert_eq!(normalize("GPU"), "gpu");
    }
}
