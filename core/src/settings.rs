//! 設定頁的內容（存在使用者設定資料夾的 settings.json）。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// 預設存檔路徑（通常是 NAS）
    pub output_dir: String,
    /// 每段幾分鐘
    pub segment_minutes: u32,
    /// 預設錄製起點：`now`（從現在）或 `start`（從直播開頭）
    pub default_mode: String,
    /// 同時轉檔幾段
    pub convert_workers: u32,
    /// 轉成 MXF 之後刪掉 .ts
    pub delete_ts: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: String::new(),
            segment_minutes: 10,
            default_mode: "now".into(),
            convert_workers: 2,
            delete_ts: false,
        }
    }
}

impl Settings {
    /// 防呆：值超出範圍就拉回合理範圍
    pub fn normalized(mut self) -> Self {
        self.segment_minutes = self.segment_minutes.clamp(1, 240);
        self.convert_workers = self.convert_workers.clamp(1, 8);
        if self.default_mode != "start" {
            self.default_mode = "now".into();
        }
        self
    }

    pub fn segment_seconds(&self) -> u32 {
        self.segment_minutes.clamp(1, 240) * 60
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let s: Settings = serde_json::from_str(r#"{"output_dir":"N:/x"}"#).unwrap();
        assert_eq!(s.segment_minutes, 10);
        assert_eq!(s.output_dir, "N:/x");
    }

    #[test]
    fn normalizes() {
        let s = Settings { segment_minutes: 0, convert_workers: 99, default_mode: "x".into(), ..Default::default() }.normalized();
        assert_eq!((s.segment_minutes, s.convert_workers, s.default_mode.as_str()), (1, 8, "now"));
    }
}
