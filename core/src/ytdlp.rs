//! yt-dlp 參數和輸出解析。

use serde::Serialize;

/// 直播狀態（yt-dlp 的 `live_status`）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveStatus {
    /// 預定直播，還沒開始
    IsUpcoming,
    IsLive,
    /// 剛結束，YouTube 還在處理
    PostLive,
    WasLive,
    NotLive,
    Unknown,
}

impl LiveStatus {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "is_upcoming" => Self::IsUpcoming,
            "is_live" => Self::IsLive,
            "post_live" => Self::PostLive,
            "was_live" => Self::WasLive,
            "not_live" => Self::NotLive,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VideoStatus {
    pub status: LiveStatus,
    pub title: String,
    /// 預定開播時間（Unix 秒），只有預定直播有
    pub release_timestamp: Option<i64>,
}

/// `--encoding utf-8`：Windows 上輸出到管線時 yt-dlp 預設用系統編碼（cp950），中文標題會變亂碼。
const COMMON: [&str; 5] = ["--no-warnings", "--no-playlist", "--no-progress", "--encoding", "utf-8"];
const STATUS_TEMPLATE: &str = "%(live_status)s|%(release_timestamp)s|%(title)s";

/// 查直播狀態。`--ignore-no-formats-error` 讓還沒開播的直播也能回傳狀態，不會直接報錯。
pub fn status_args(url: &str) -> Vec<String> {
    let mut a: Vec<String> = COMMON.map(String::from).to_vec();
    a.extend(["--skip-download", "--ignore-no-formats-error", "--print", STATUS_TEMPLATE, url].map(String::from));
    a
}

pub fn parse_status(stdout: &str) -> Option<VideoStatus> {
    let line = stdout.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    let mut it = line.splitn(3, '|');
    let status = LiveStatus::from_str(it.next()?);
    let release_timestamp = it.next().and_then(|s| s.trim().parse::<f64>().ok()).map(|f| f as i64);
    let title = it.next().unwrap_or("").trim().to_string();
    Some(VideoStatus { status, title, release_timestamp })
}

/// 「從現在開始錄」：取直播的 HLS 播放清單網址。
/// YouTube 直播（2026 年）只給影像、聲音分開的 m3u8，所以優先挑「最好的 H.264 影像 + 最好的聲音」兩個網址；
/// 萬一還有影音合一的格式也接受（只回一個網址）。
pub const HLS_FORMAT: &str = "bv*[protocol^=m3u8][vcodec^=avc1]+ba[protocol^=m3u8]/bv*[protocol^=m3u8]+ba[protocol^=m3u8]/b[protocol^=m3u8]";

pub fn hls_url_args(url: &str) -> Vec<String> {
    let mut a: Vec<String> = COMMON.map(String::from).to_vec();
    a.extend(["-f", HLS_FORMAT, "-g", url].map(String::from));
    a
}

/// `-g` 的輸出：影像（和聲音）的網址，一行一個。
pub fn parse_urls(stdout: &str) -> Vec<String> {
    stdout.lines().map(str::trim).filter(|l| l.starts_with("http")).map(String::from).collect()
}

/// 「從直播開頭錄」用的格式：YouTube 從頭下載只有影音分開的 DASH，各開一個 yt-dlp 輸出到 stdout。
/// 只挑 H.264 / AAC，才能直接 -c copy 進 .ts。
pub const FROM_START_VIDEO: &str = "bv[vcodec^=avc1]";
pub const FROM_START_AUDIO: &str = "ba[acodec^=mp4a]";

pub fn from_start_args(url: &str, format: &str) -> Vec<String> {
    let mut a: Vec<String> = COMMON.map(String::from).to_vec();
    a.extend(["--live-from-start", "--no-part", "-f", format, "-o", "-", url].map(String::from));
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_line() {
        let s = parse_status("is_live|NA|台北 | 記者會\n").unwrap();
        assert_eq!(s.status, LiveStatus::IsLive);
        assert_eq!(s.title, "台北 | 記者會");
        assert_eq!(s.release_timestamp, None);
        let s = parse_status("is_upcoming|1763100000|預告").unwrap();
        assert_eq!(s.status, LiveStatus::IsUpcoming);
        assert_eq!(s.release_timestamp, Some(1763100000));
        assert_eq!(parse_status("NA|NA|x").unwrap().status, LiveStatus::Unknown);
        assert!(parse_status("").is_none());
    }

    #[test]
    fn parses_urls() {
        assert_eq!(parse_urls("\nhttps://a/v.m3u8\nhttps://a/a.m3u8\n"), vec!["https://a/v.m3u8", "https://a/a.m3u8"]);
        assert!(parse_urls("ERROR").is_empty());
    }

    #[test]
    fn from_start_writes_to_stdout() {
        let a = from_start_args("u", FROM_START_VIDEO);
        assert!(a.contains(&"--live-from-start".to_string()));
        let o = a.iter().position(|x| x == "-o").unwrap();
        assert_eq!(a[o + 1], "-");
    }
}
