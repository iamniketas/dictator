//! Dictator library - Voice dictation service
//!
//! This crate provides voice-to-text functionality using local LLM models.

pub mod audio;
pub mod config;
pub mod history;
pub mod model_downloader;
pub mod runtime_adapter;
pub mod settings_window;
pub mod updater;
pub mod transcribe;
pub mod whisper_engine;
pub mod llm;
pub mod ui;
pub mod input;
pub mod overlay_win32;
pub mod streaming;
pub mod whisper_server;
