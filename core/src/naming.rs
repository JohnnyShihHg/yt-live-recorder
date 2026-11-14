//! 命名規則：名稱 `香蕉蛋糕` → `<存檔路徑>/香蕉蛋糕/香蕉蛋糕_001.mxf`、`香蕉蛋糕_002.mxf`…
//! 錄下來的 .ts 放在 `<存檔路徑>/香蕉蛋糕/_ts/香蕉蛋糕_001.ts`。

use std::path::{Path, PathBuf};

pub const TS_DIR: &str = "_ts";
pub const LOG_FILE: &str = "錄製紀錄.txt";

/// 把名稱裡 Windows 不能用的字元換掉。`%` 也換掉，因為 ffmpeg 的分段檔名會把它當格式字元。
pub fn sanitize(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            '%' => '％',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    // Windows 不允許結尾是點或空白
    s.trim_end_matches(['.', ' ']).to_string()
}

pub fn task_dir(output_dir: &Path, name: &str) -> PathBuf {
    output_dir.join(name)
}

pub fn ts_dir(output_dir: &Path, name: &str) -> PathBuf {
    task_dir(output_dir, name).join(TS_DIR)
}

/// 給 ffmpeg segment muxer 的檔名樣式（`名稱_%03d.ts`）
pub fn ts_pattern(output_dir: &Path, name: &str) -> PathBuf {
    ts_dir(output_dir, name).join(format!("{name}_%03d.ts"))
}

pub fn segment_file(name: &str, index: u32, ext: &str) -> String {
    format!("{name}_{index:03}.{ext}")
}

pub fn mxf_path(output_dir: &Path, name: &str, index: u32) -> PathBuf {
    task_dir(output_dir, name).join(segment_file(name, index, crate::spec::EXTENSION))
}

/// 轉檔中的暫存檔名：剪輯端看到的一定是轉完的檔案。
pub fn mxf_part_path(output_dir: &Path, name: &str, index: u32) -> PathBuf {
    task_dir(output_dir, name).join(format!("{}.part", segment_file(name, index, crate::spec::EXTENSION)))
}

/// 從檔名取出分段號：`香蕉蛋糕_012.ts` → 12。名稱不符就是 None。
pub fn parse_index(name: &str, file_name: &str, ext: &str) -> Option<u32> {
    let rest = file_name.strip_prefix(name)?.strip_prefix('_')?;
    let num = rest.strip_suffix(ext)?.strip_suffix('.')?;
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    num.parse().ok()
}

/// 資料夾裡（.ts 和 .mxf 都算）已經用到的最大分段號。同名任務再開一次時接著編號，不會蓋掉舊檔。
pub fn max_index(output_dir: &Path, name: &str) -> Option<u32> {
    let scan = |dir: PathBuf, ext: &str| -> Vec<u32> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| parse_index(name, &e.file_name().to_string_lossy(), ext))
            .collect()
    };
    scan(ts_dir(output_dir, name), "ts")
        .into_iter()
        .chain(scan(task_dir(output_dir, name), crate::spec::EXTENSION))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize(" 香蕉/蛋糕?. "), "香蕉_蛋糕_");
        assert_eq!(sanitize("100%直播"), "100％直播");
    }

    #[test]
    fn segment_names_follow_rule() {
        let out = Path::new("N:/live");
        assert_eq!(mxf_path(out, "香蕉蛋糕", 1), Path::new("N:/live/香蕉蛋糕/香蕉蛋糕_001.mxf"));
        assert_eq!(segment_file("香蕉蛋糕", 1234, "ts"), "香蕉蛋糕_1234.ts");
        assert_eq!(ts_pattern(out, "a").file_name().unwrap(), "a_%03d.ts");
    }

    #[test]
    fn parses_index() {
        assert_eq!(parse_index("香蕉蛋糕", "香蕉蛋糕_012.ts", "ts"), Some(12));
        assert_eq!(parse_index("香蕉蛋糕", "香蕉蛋糕_1000.ts", "ts"), Some(1000));
        assert_eq!(parse_index("香蕉蛋糕", "香蕉蛋糕_012.mxf.part", "mxf"), None);
        assert_eq!(parse_index("香蕉", "香蕉蛋糕_012.ts", "ts"), None);
        assert_eq!(parse_index("a", "a_x1.ts", "ts"), None);
    }

    #[test]
    fn max_index_scans_both_folders() {
        let root = std::env::temp_dir().join(format!("ytlr-naming-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(ts_dir(&root, "x")).unwrap();
        assert_eq!(max_index(&root, "x"), None);
        std::fs::write(ts_dir(&root, "x").join("x_003.ts"), b"").unwrap();
        std::fs::write(task_dir(&root, "x").join("x_005.mxf"), b"").unwrap();
        assert_eq!(max_index(&root, "x"), Some(5));
        let _ = std::fs::remove_dir_all(&root);
    }
}
