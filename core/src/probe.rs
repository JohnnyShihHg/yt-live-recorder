//! 讀 ffprobe 的 JSON 輸出，取出轉檔需要的來源資訊。

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SourceInfo {
    pub fps: Option<f64>,
    pub has_audio: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<f64>,
}

/// ffprobe 參數：`ffprobe <這些> <檔案>`
pub fn args(path: &str) -> Vec<String> {
    ["-v", "error", "-show_streams", "-show_format", "-of", "json", path]
        .map(String::from)
        .to_vec()
}

fn rate(v: &Value) -> Option<f64> {
    let s = v.as_str()?;
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (n > 0.0 && d > 0.0).then(|| n / d)
}

pub fn parse(json: &str) -> Result<SourceInfo, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("ffprobe 輸出看不懂：{e}"))?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video");
    let Some(video) = video else {
        return Err("檔案裡沒有影像".into());
    };
    let fps = rate(&video["avg_frame_rate"]).or_else(|| rate(&video["r_frame_rate"]));
    let num = |x: &Value| x.as_u64().map(|n| n as u32);
    let dur = |x: &Value| x.as_str().and_then(|s| s.parse::<f64>().ok());
    Ok(SourceInfo {
        fps,
        has_audio: streams.iter().any(|s| s["codec_type"] == "audio"),
        width: num(&video["width"]),
        height: num(&video["height"]),
        duration: dur(&v["format"]["duration"]).or_else(|| dur(&video["duration"])),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_youtube_ts() {
        let j = r#"{"streams":[
            {"codec_type":"video","width":1920,"height":1080,"avg_frame_rate":"30/1","r_frame_rate":"30/1"},
            {"codec_type":"audio","sample_rate":"48000"}],
            "format":{"duration":"600.033"}}"#;
        let s = parse(j).unwrap();
        assert_eq!(s.fps, Some(30.0));
        assert!(s.has_audio);
        assert_eq!(s.width, Some(1920));
        assert_eq!(s.duration, Some(600.033));
    }

    #[test]
    fn falls_back_to_r_frame_rate_and_detects_no_audio() {
        let j = r#"{"streams":[{"codec_type":"video","avg_frame_rate":"0/0","r_frame_rate":"60000/1001"}],"format":{}}"#;
        let s = parse(j).unwrap();
        assert!((s.fps.unwrap() - 59.94).abs() < 0.01);
        assert!(!s.has_audio);
    }

    #[test]
    fn no_video_is_error() {
        assert!(parse(r#"{"streams":[{"codec_type":"audio"}]}"#).is_err());
    }
}
