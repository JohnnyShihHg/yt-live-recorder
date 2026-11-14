//! 用真的 ffmpeg 驗證 MXF 參數：做一段像 YouTube 的測試片 → 照 spec 轉 MXF → ffprobe 比對 EBC 規格。
//! 需要 ffmpeg，平常不跑：
//!
//!   set FFMPEG_DIR=C:\path\to\folder\with\ffmpeg-and-ffprobe
//!   cargo test -p yt-live-recorder-core --test mxf_real_ffmpeg -- --ignored --nocapture

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use yt_live_recorder_core::{probe, spec};

fn tool(name: &str) -> PathBuf {
    let dir = std::env::var("FFMPEG_DIR").expect("請設定 FFMPEG_DIR");
    PathBuf::from(dir).join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

fn run(exe: &PathBuf, args: &[String]) -> String {
    let out = Command::new(exe).args(args).output().unwrap();
    assert!(out.status.success(), "{} 失敗：{}", exe.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn check(fps: &str, size: &str, label: &str) {
    let dir = std::env::temp_dir().join("ytlr-mxf-test");
    std::fs::create_dir_all(&dir).unwrap();
    let ts = dir.join(format!("{label}.ts"));
    let mxf = dir.join(format!("{label}.mxf"));
    let ts_s = ts.to_string_lossy().to_string();
    // 像 YouTube 直播的來源：H.264 + AAC 立體聲，逐行
    run(
        &tool("ffmpeg"),
        &[
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", &format!("testsrc2=size={size}:rate={fps}:duration=4"),
            "-f", "lavfi", "-i", "sine=frequency=1000:sample_rate=44100:duration=4",
            "-ac", "2", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-f", "mpegts", &ts_s,
        ]
        .map(String::from),
    );
    let info = probe::parse(&run(&tool("ffprobe"), &probe::args(&ts_s))).unwrap();
    run(&tool("ffmpeg"), &spec::mxf_args(&ts_s, &mxf.to_string_lossy(), &info));

    let json = run(&tool("ffprobe"), &probe::args(&mxf.to_string_lossy()));
    let v: Value = serde_json::from_str(&json).unwrap();
    let streams = v["streams"].as_array().unwrap();
    let video = &streams[0];
    println!("[{label}] video: {}", video);
    assert_eq!(v["format"]["format_name"], "mxf");
    assert_eq!(video["codec_name"], "mpeg2video");
    assert_eq!(video["profile"], "4:2:2");
    assert_eq!(video["level"], 2);
    assert_eq!(video["pix_fmt"], "yuv422p");
    assert_eq!(video["width"], 1920);
    assert_eq!(video["height"], 1080);
    assert_eq!(video["r_frame_rate"], "30000/1001");
    assert_eq!(video["field_order"], "tt");
    assert_eq!(video["color_space"], "bt709");
    assert_eq!(video["display_aspect_ratio"], "16:9");
    let audio: Vec<&Value> = streams.iter().filter(|s| s["codec_type"] == "audio").collect();
    assert_eq!(audio.len(), 8);
    for a in audio {
        assert_eq!(a["codec_name"], "pcm_s24le");
        assert_eq!(a["sample_rate"], "48000");
        assert_eq!(a["channels"], 1);
    }
    let dur: f64 = v["format"]["duration"].as_str().unwrap().parse().unwrap();
    assert!((dur - 4.0).abs() < 0.2, "長度 {dur}");
}

#[test]
#[ignore]
fn youtube_1080p30_to_ebc_mxf() {
    check("30", "1920x1080", "p30");
}

#[test]
#[ignore]
fn youtube_720p60_to_ebc_mxf() {
    check("60", "1280x720", "p60");
}

#[test]
#[ignore]
fn vertical_stream_is_pillarboxed() {
    check("30", "1080x1920", "vertical");
}
