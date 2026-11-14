//! YT 直播錄製工具的純邏輯（不依賴 Tauri，方便測試）。

pub mod naming;
pub mod probe;
pub mod record;
pub mod settings;
pub mod spec;
pub mod ytdlp;

pub use settings::Settings;
