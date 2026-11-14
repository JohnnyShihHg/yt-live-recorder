//! 公司 Avid 用的 MXF 規格。**要換規格只改這個檔案。**
//!
//! 依據：EBC 轉檔器產出檔（kk1114-1_YT.mxf）的 ffprobe 結果（桌面 `EBC轉檔器_MXF規格.txt`）：
//!
//! | 項目 | 值 |
//! |---|---|
//! | 封裝 | MXF OP1a（ffmpeg 的 mxf muxer） |
//! | 影像 | MPEG-2 4:2:2 Profile @ High Level（XDCAM HD422 型），50 Mb/s，yuv422p |
//! | 解析度 | 1920×1080，SAR 1:1，DAR 16:9 |
//! | 幀率 | 30000/1001（29.97），交錯，上場優先（field_order=tt） |
//! | 色彩 | BT.709，tv range，chroma topleft |
//! | VBV | 17825792 bits |
//! | 聲音 | 8 軌，每軌單聲道 PCM 24-bit little-endian，48 kHz |
//! | Timecode | 00:00:00:00 |
//!
//! 原始碼看不到（轉檔器是打包好的 exe），所以 GOP 長度等 ffprobe 看不出來的參數是照 XDCAM HD422
//! 1080i59.94 的慣例（GOP 15、2 張 B 幀）設定，第一次實際進 Avid 時要確認。

use crate::probe::SourceInfo;

pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
/// 輸出幀率（交錯，每幀兩場）
pub const FRAME_RATE: &str = "30000/1001";
/// 來源超過這個幀率（50/60p）時，先轉成 59.94p 再兩幀合一幀交錯，動作才順；低於的話直接 29.97 標成交錯。
pub const HIGH_FPS_THRESHOLD: f64 = 40.0;

pub const VIDEO_BITRATE: u64 = 50_000_000;
pub const VBV_BUFFER: u64 = 17_825_792;
/// 每個 GOP 幾幀、兩個 P 幀之間幾張 B 幀（ffprobe 看不出來，照 XDCAM HD422 慣例）
pub const GOP: u32 = 15;
pub const B_FRAMES: u32 = 2;

pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const AUDIO_CODEC: &str = "pcm_s24le";
pub const TIMECODE: &str = "00:00:00:00";

/// 每一軌聲音放什麼。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Left,
    Right,
    Silent,
}

/// 8 軌：1 左、2 右，其餘靜音。
pub const AUDIO_TRACKS: [Track; 8] = [
    Track::Left,
    Track::Right,
    Track::Silent,
    Track::Silent,
    Track::Silent,
    Track::Silent,
    Track::Silent,
    Track::Silent,
];

/// 輸出副檔名
pub const EXTENSION: &str = "mxf";

/// 影像濾鏡：縮放到 1920×1080（比例不同就補黑邊）→ 轉幀率 → 交錯。
fn video_filter(src: &SourceInfo) -> String {
    let fit = format!(
        "scale={WIDTH}:{HEIGHT}:force_original_aspect_ratio=decrease:flags=lanczos,\
         pad={WIDTH}:{HEIGHT}:(ow-iw)/2:(oh-ih)/2,setsar=1"
    );
    let high_fps = src.fps.is_some_and(|f| f > HIGH_FPS_THRESHOLD);
    let interlace = if high_fps {
        // 59.94p → 每兩幀合成一幀的兩個場，輸出 29.97i
        "fps=60000/1001,interlace=scan=tff:lowpass=complex".to_string()
    } else {
        format!("fps={FRAME_RATE}")
    };
    format!("[0:v:0]{fit},{interlace},setfield=tff,format=yuv422p[v]")
}

/// 聲音濾鏡：來源轉成 48k 立體聲後拆成左右，再依 `AUDIO_TRACKS` 排成 8 個單聲道輸出 `[a1]`…`[a8]`。
/// 靜音軌從左聲道乘 0 做出來，長度自然跟來源一樣（不用無限長的 anullsrc + -shortest）。
fn audio_filter(input: &str) -> String {
    let n = |t: Track| AUDIO_TRACKS.iter().filter(|&&x| x == t).count();
    let (left, right, silent) = (n(Track::Left), n(Track::Right), n(Track::Silent));
    let mut parts = vec![format!(
        "[{input}]aresample={AUDIO_SAMPLE_RATE},aformat=channel_layouts=stereo,\
         channelsplit=channel_layout=stereo[L][R]"
    )];
    // 左聲道多拆一份給靜音軌用
    let l_outs = split(&mut parts, "L", "", left + usize::from(silent > 0));
    let r_outs = split(&mut parts, "R", "", right);
    let s_outs = match l_outs.get(left) {
        Some(base) => split(&mut parts, base, "volume=0,", silent),
        None => Vec::new(),
    };
    let (mut li, mut ri, mut si) = (0, 0, 0);
    for (i, t) in AUDIO_TRACKS.iter().enumerate() {
        let src = match t {
            Track::Left => {
                li += 1;
                &l_outs[li - 1]
            }
            Track::Right => {
                ri += 1;
                &r_outs[ri - 1]
            }
            Track::Silent => {
                si += 1;
                &s_outs[si - 1]
            }
        };
        parts.push(format!("[{src}]anull[a{}]", i + 1));
    }
    parts.join(";")
}

/// 把 `[src]` 經過 `pre` 濾鏡後分成 `count` 份，回傳輸出標籤；0 份就丟掉（anullsink）。
fn split(parts: &mut Vec<String>, src: &str, pre: &str, count: usize) -> Vec<String> {
    let outs: Vec<String> = (0..count).map(|i| format!("{src}_{i}")).collect();
    let tags: String = outs.iter().map(|o| format!("[{o}]")).collect();
    parts.push(match count {
        0 => format!("[{src}]{pre}anullsink"),
        1 => format!("[{src}]{pre}anull{tags}"),
        n => format!("[{src}]{pre}asplit={n}{tags}"),
    });
    outs
}

/// ffmpeg 參數：把一段錄好的 .ts 轉成公司規格 MXF。`output` 會用 `-f mxf` 強制格式，所以副檔名可以是暫存名。
pub fn mxf_args(input: &str, output: &str, src: &SourceInfo) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-stats", "-i", input]
        .iter()
        .map(|s| s.to_string())
        .collect();
    // 來源沒聲音：補一個靜音輸入（-shortest 讓它跟影像一樣長）
    let audio_input = if src.has_audio {
        "0:a:0"
    } else {
        a.extend(["-f", "lavfi", "-i"].map(String::from));
        a.push(format!("anullsrc=r={AUDIO_SAMPLE_RATE}:cl=stereo"));
        "1:a:0"
    };
    a.push("-filter_complex".into());
    a.push(format!("{};{}", video_filter(src), audio_filter(audio_input)));
    a.extend(["-map", "[v]"].map(String::from));
    for i in 1..=AUDIO_TRACKS.len() {
        a.push("-map".into());
        a.push(format!("[a{i}]"));
    }
    let b = VIDEO_BITRATE.to_string();
    let vbv = VBV_BUFFER.to_string();
    let gop = GOP.to_string();
    let bf = B_FRAMES.to_string();
    let ar = AUDIO_SAMPLE_RATE.to_string();
    a.extend(
        [
            "-c:v", "mpeg2video",
            "-profile:v", "0", // 4:2:2
            "-level:v", "2",   // High
            "-pix_fmt", "yuv422p",
            "-b:v", &b, "-minrate", &b, "-maxrate", &b,
            "-bufsize", &vbv, "-rc_init_occupancy", &vbv,
            "-g", &gop, "-bf", &bf,
            "-flags", "+ildct+ilme", "-top", "1",
            "-intra_vlc", "1", "-non_linear_quant", "1", "-dc", "10",
            "-qmin", "1", "-qmax", "12",
            "-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709",
            "-color_range", "tv", "-chroma_sample_location", "topleft",
            "-r", FRAME_RATE,
            "-c:a", AUDIO_CODEC, "-ar", &ar,
            "-timecode", TIMECODE,
        ]
        .map(String::from),
    );
    if !src.has_audio {
        a.push("-shortest".into());
    }
    a.extend(["-f", "mxf", output].map(String::from));
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(fps: f64, audio: bool) -> SourceInfo {
        SourceInfo { fps: Some(fps), has_audio: audio, width: Some(1920), height: Some(1080), duration: Some(600.0) }
    }

    fn value_after<'a>(args: &'a [String], key: &str) -> &'a str {
        let i = args.iter().position(|a| a == key).unwrap_or_else(|| panic!("沒有 {key}"));
        &args[i + 1]
    }

    #[test]
    fn video_matches_ebc_spec() {
        let a = mxf_args("in.ts", "out.mxf", &src(30.0, true));
        assert_eq!(value_after(&a, "-c:v"), "mpeg2video");
        assert_eq!(value_after(&a, "-pix_fmt"), "yuv422p");
        assert_eq!(value_after(&a, "-b:v"), "50000000");
        assert_eq!(value_after(&a, "-bufsize"), "17825792");
        assert_eq!(value_after(&a, "-r"), "30000/1001");
        assert_eq!(value_after(&a, "-top"), "1");
        assert_eq!(value_after(&a, "-f"), "mxf");
        assert_eq!(a.last().unwrap(), "out.mxf");
    }

    #[test]
    fn eight_mono_pcm_tracks() {
        let a = mxf_args("in.ts", "out.mxf", &src(30.0, true));
        let maps: Vec<&String> = a.iter().zip(a.iter().skip(1)).filter(|(k, _)| *k == "-map").map(|(_, v)| v).collect();
        assert_eq!(maps.len(), 9);
        assert_eq!(maps[0], "[v]");
        assert_eq!(value_after(&a, "-c:a"), "pcm_s24le");
        assert_eq!(value_after(&a, "-ar"), "48000");
        let fc = value_after(&a, "-filter_complex");
        for i in 1..=8 {
            assert!(fc.contains(&format!("[a{i}]")), "缺 [a{i}]：{fc}");
        }
        assert!(fc.contains("volume=0"));
    }

    #[test]
    fn high_fps_source_is_interlaced_from_59_94p() {
        let fc = value_after(&mxf_args("i", "o", &src(60.0, true)), "-filter_complex").to_string();
        assert!(fc.contains("fps=60000/1001,interlace=scan=tff"));
        let fc = value_after(&mxf_args("i", "o", &src(29.97, true)), "-filter_complex").to_string();
        assert!(fc.contains("fps=30000/1001,setfield=tff"));
    }

    #[test]
    fn silent_source_gets_generated_audio() {
        let a = mxf_args("in.ts", "out.mxf", &src(30.0, false));
        assert!(a.iter().any(|x| x.starts_with("anullsrc")));
        assert!(value_after(&a, "-filter_complex").contains("[1:a:0]"));
        assert!(a.contains(&"-shortest".to_string()));
    }
}
