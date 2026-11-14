//! 實際錄一小段 YouTube 直播（用跟程式一樣的參數）：yt-dlp 取網址 → ffmpeg 切 10 秒一段 → 第 1 段轉 MXF。
//! 需要網路、ffmpeg、yt-dlp，平常不跑：
//!
//!   set FFMPEG_DIR=C:\folder\with\ffmpeg-ffprobe-yt-dlp
//!   set YT_LIVE_URL=https://www.youtube.com/@某頻道/live
//!   cargo test -p yt-live-recorder-core --test live_record -- --ignored --nocapture

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use yt_live_recorder_core::{naming, probe, record, spec, ytdlp};

fn tool(name: &str) -> PathBuf {
    let dir = std::env::var("FFMPEG_DIR").expect("請設定 FFMPEG_DIR");
    PathBuf::from(dir).join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

fn stdout(exe: &str, args: &[String]) -> String {
    let out = Command::new(tool(exe)).args(args).output().unwrap();
    assert!(out.status.success(), "{exe} 失敗：{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("輸出不是 UTF-8")
}

#[test]
#[ignore]
fn record_live_hls_and_convert() {
    let url = std::env::var("YT_LIVE_URL").expect("請設定 YT_LIVE_URL");
    let st = ytdlp::parse_status(&stdout("yt-dlp", &ytdlp::status_args(&url))).unwrap();
    println!("狀態：{:?}，標題：{}", st.status, st.title);
    assert_eq!(st.status, ytdlp::LiveStatus::IsLive);

    let urls = ytdlp::parse_urls(&stdout("yt-dlp", &ytdlp::hls_url_args(&url)));
    println!("網址數：{}", urls.len());
    assert!(!urls.is_empty());

    let out = std::env::temp_dir().join("ytlr-live-test");
    let _ = std::fs::remove_dir_all(&out);
    let name = "測試直播";
    std::fs::create_dir_all(naming::ts_dir(&out, name)).unwrap();
    let args = record::segment_args(
        &record::Input::Hls { video: &urls[0], audio: urls.get(1).map(String::as_str) },
        &naming::ts_pattern(&out, name),
        10,
        1,
    );
    let mut child = Command::new(tool("ffmpeg")).args(&args).stdin(Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_secs(25));
    // 跟程式按停止一樣：送 q 讓 ffmpeg 收尾
    let _ = child.stdin.take().unwrap().write_all(b"q");
    let st = child.wait().unwrap();
    println!("ffmpeg 結束：{st}");

    assert_eq!(naming::max_index(&out, name).map(|i| i >= 2), Some(true), "應該至少有 2 段");
    let ts = naming::ts_dir(&out, name).join(naming::segment_file(name, 1, "ts"));
    let ts_s = ts.to_string_lossy().to_string();
    let info = probe::parse(&stdout("ffprobe", &probe::args(&ts_s))).unwrap();
    println!("第 1 段：{info:?}");
    assert!(info.has_audio, "分段要有聲音");
    let d = info.duration.unwrap();
    assert!((8.0..=14.0).contains(&d), "第 1 段長度 {d}");

    let mxf = naming::mxf_path(&out, name, 1);
    stdout("ffmpeg", &spec::mxf_args(&ts_s, &mxf.to_string_lossy(), &info));
    let m = probe::parse(&stdout("ffprobe", &probe::args(&mxf.to_string_lossy()))).unwrap();
    println!("MXF：{m:?}");
    assert_eq!((m.width, m.height), (Some(1920), Some(1080)));
}
