//! 錄製用的 ffmpeg 參數：串流直接 -c copy 切成分段 .ts（不重新編碼）。

use std::path::Path;

pub enum Input<'a> {
    /// 直播的 HLS 播放清單（從現在開始錄）。YouTube 通常是影像、聲音分開的兩個網址。
    Hls { video: &'a str, audio: Option<&'a str> },
    /// 影像、聲音分開的兩路本機 TCP（從直播開頭錄，由程式把 yt-dlp 的 stdout 轉過來）
    Split { video: &'a str, audio: &'a str },
}

/// 15 秒收不到資料就結束，交給外層重新抓網址重連
const RW_TIMEOUT: &str = "15000000";

pub fn segment_args(input: &Input, pattern: &Path, segment_seconds: u32, start_number: u32) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "warning", "-y"].map(String::from).to_vec();
    match input {
        Input::Hls { video, audio } => {
            // extension_picky 0：YouTube 的聲音分段其實是 AAC 但網址結尾是 seg.ts，ffmpeg 7 預設會拒絕
            a.extend(["-rw_timeout", RW_TIMEOUT, "-extension_picky", "0", "-i", video].map(String::from));
            match audio {
                Some(audio) => {
                    a.extend(["-rw_timeout", RW_TIMEOUT, "-extension_picky", "0", "-i", audio].map(String::from));
                    a.extend(["-map", "0:v:0", "-map", "1:a:0"].map(String::from));
                }
                None => a.extend(["-map", "0:v:0", "-map", "0:a:0?"].map(String::from)),
            }
        }
        Input::Split { video, audio } => {
            a.extend(["-i", video, "-i", audio].map(String::from));
            a.extend(["-map", "0:v:0", "-map", "1:a:0"].map(String::from));
        }
    }
    a.extend(
        [
            "-c", "copy",
            "-f", "segment",
            "-segment_format", "mpegts",
            "-segment_time", &segment_seconds.to_string(),
            "-reset_timestamps", "1",
            "-segment_start_number", &start_number.to_string(),
        ]
        .map(String::from),
    );
    a.push(pattern.to_string_lossy().into_owned());
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(a: &[String], k: &str) -> String {
        a[a.iter().position(|x| x == k).unwrap() + 1].clone()
    }

    #[test]
    fn hls_split_video_audio() {
        let a = segment_args(&Input::Hls { video: "https://x/v.m3u8", audio: Some("https://x/a.m3u8") }, Path::new("D:/o/_ts/n_%03d.ts"), 600, 4);
        assert_eq!(a.iter().filter(|x| *x == "-i").count(), 2);
        assert_eq!(a.iter().filter(|x| *x == "-rw_timeout").count(), 2);
        assert!(a.contains(&"1:a:0".to_string()));
        assert_eq!(at(&a, "-c"), "copy");
        assert_eq!(at(&a, "-segment_time"), "600");
        assert_eq!(at(&a, "-segment_start_number"), "4");
        assert!(a.last().unwrap().ends_with("n_%03d.ts"));
    }

    #[test]
    fn hls_muxed_single_url() {
        let a = segment_args(&Input::Hls { video: "https://x/m.m3u8", audio: None }, Path::new("p"), 60, 1);
        assert_eq!(at(&a, "-i"), "https://x/m.m3u8");
        assert!(a.contains(&"0:a:0?".to_string()));
    }

    #[test]
    fn split_inputs_map_both() {
        let a = segment_args(&Input::Split { video: "tcp://127.0.0.1:1", audio: "tcp://127.0.0.1:2" }, Path::new("p"), 60, 1);
        assert_eq!(a.iter().filter(|x| *x == "-i").count(), 2);
        assert!(a.contains(&"1:a:0".to_string()));
    }
}
