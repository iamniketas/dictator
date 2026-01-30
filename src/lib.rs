//! Dictator library - Voice dictation service
//!
//! This crate provides voice-to-text functionality using local LLM models.

pub mod audio;
pub mod config;
pub mod transcribe;
pub mod llm;
pub mod ui;
pub mod input;
pub mod overlay_win32;
pub mod streaming;