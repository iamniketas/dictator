//! Dictator library - Voice dictation service
//!
//! This crate provides voice-to-text functionality using local LLM models.

pub mod audio;
pub mod config;
pub mod history;
pub mod input;
pub mod llm;
pub mod model_downloader;
pub mod overlay_win32;
pub mod runtime_adapter;
pub mod settings_window;
pub mod streaming;
pub mod transcribe;
pub mod ui;
pub mod updater;
pub mod whisper_engine;
pub mod whisper_server;
