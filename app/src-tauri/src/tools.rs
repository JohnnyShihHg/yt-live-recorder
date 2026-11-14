//! ffmpeg / ffprobe / yt-dlp：找程式、第一次使用時下載、一鍵更新 yt-dlp。
//! 做法跟關鍵標題整理工具一樣：下載到程式資料夾的 tools\，找不到再找系統 PATH。

use serde::Serialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const FFMPEG: &str = "ffmpeg";
pub const FFPROBE: &str = "ffprobe";
pub const YTDLP: &str = "yt-dlp";

/// 換 ffmpeg 版本時改這裡（下載網址也要一起改），已安裝的會提示重新下載。
const FFMPEG_BUILD: &str = "7.1.1-essentials";
const FFMPEG_URL: &str = "https://github.com/GyanD/codexffmpeg/releases/download/7.1.1/ffmpeg-7.1.1-essentials_build.zip";
const FFMPEG_BUILD_MARKER: &str = "ffmpeg-build.txt";
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

fn tools_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_local_data_dir().map(|d| d.join("tools")).map_err(|e| e.to_string())
}

fn exe_name(tool: &str) -> String {
    format!("{tool}{}", std::env::consts::EXE_SUFFIX)
}

pub fn find_tool(app: &AppHandle, tool: &str) -> Option<PathBuf> {
    let name = exe_name(tool);
    if let Ok(dir) = tools_dir(app) {
        let p = dir.join(&name);
        if p.is_file() {
            return Some(p);
        }
    }
    std::env::var_os("PATH")
        .and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(&name)).find(|p| p.is_file()))
}

pub fn require(app: &AppHandle, tool: &str) -> Result<PathBuf, String> {
    find_tool(app, tool).ok_or_else(|| format!("找不到 {tool}，請到「設定」按「下載工具」"))
}

/// 開子程序。Windows 上要加 CREATE_NO_WINDOW，否則每次呼叫都會閃一個黑色命令列視窗。
pub fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[derive(Serialize)]
pub struct ToolsStatus {
    ffmpeg: Option<String>,
    ffprobe: Option<String>,
    ytdlp: Option<String>,
    ytdlp_version: Option<String>,
    ffmpeg_outdated: bool,
}

fn ytdlp_version(app: &AppHandle) -> Option<String> {
    let exe = find_tool(app, YTDLP)?;
    let out = command(&exe).arg("--version").output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn status(app: &AppHandle) -> ToolsStatus {
    let show = |t: &str| find_tool(app, t).map(|p| p.display().to_string());
    let ffmpeg_outdated = tools_dir(app).is_ok_and(|dir| {
        dir.join(exe_name(FFMPEG)).is_file()
            && std::fs::read_to_string(dir.join(FFMPEG_BUILD_MARKER)).map_or(true, |v| v.trim() != FFMPEG_BUILD)
    });
    ToolsStatus {
        ffmpeg: show(FFMPEG),
        ffprobe: show(FFPROBE),
        ytdlp: show(YTDLP),
        ytdlp_version: ytdlp_version(app),
        ffmpeg_outdated,
    }
}

#[tauri::command(async)]
pub fn tools_status(app: AppHandle) -> ToolsStatus {
    status(&app)
}

enum Payload {
    /// 壓縮檔：(壓縮檔內路徑結尾, 存成的檔名)
    Zip(&'static [(&'static str, &'static str)]),
    File(&'static str),
}

struct Download {
    label: &'static str,
    url: &'static str,
    payload: Payload,
}

#[derive(Clone, Serialize)]
struct Progress {
    label: String,
    downloaded: u64,
    total: u64,
}

/// `which`：`all`、`ffmpeg`、`ytdlp`。
#[tauri::command]
pub async fn install_tools(app: AppHandle, which: String) -> Result<ToolsStatus, String> {
    if !cfg!(windows) {
        return Err("自動下載只支援 Windows，請自行安裝 ffmpeg 和 yt-dlp".into());
    }
    let mut list = Vec::new();
    if which != "ytdlp" {
        list.push(Download {
            label: "ffmpeg",
            url: FFMPEG_URL,
            payload: Payload::Zip(&[("bin/ffmpeg.exe", "ffmpeg.exe"), ("bin/ffprobe.exe", "ffprobe.exe")]),
        });
    }
    if which != "ffmpeg" {
        list.push(Download { label: "yt-dlp", url: YTDLP_URL, payload: Payload::File("yt-dlp.exe") });
    }
    let dir = tools_dir(&app)?;
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        list.iter().try_for_each(|d| {
            install_one(&dir, d, &|p| {
                let _ = app2.emit("tools-progress", p);
            })
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(status(&app))
}

fn install_one(dir: &Path, d: &Download, on_progress: &dyn Fn(Progress)) -> Result<(), String> {
    let tmp = dir.join(format!("{}.download", d.label));
    download_to(d, &tmp, on_progress)?;
    let result = match d.payload {
        Payload::File(name) => replace_file(&tmp, &dir.join(name)),
        Payload::Zip(entries) => extract(&tmp, dir, entries),
    };
    let _ = std::fs::remove_file(&tmp);
    if result.is_ok() && d.label == "ffmpeg" {
        std::fs::write(dir.join(FFMPEG_BUILD_MARKER), FFMPEG_BUILD).map_err(|e| e.to_string())?;
    }
    result
}

fn download_to(d: &Download, dest: &Path, on_progress: &dyn Fn(Progress)) -> Result<(), String> {
    let resp = ureq::get(d.url).call().map_err(|e| format!("下載 {} 失敗：{e}", d.label))?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut downloaded = 0u64;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("下載 {} 中斷：{e}", d.label))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        downloaded += n as u64;
        if last_emit.elapsed() > Duration::from_millis(200) {
            last_emit = Instant::now();
            on_progress(Progress { label: d.label.into(), downloaded, total });
        }
    }
    on_progress(Progress { label: d.label.into(), downloaded, total: downloaded });
    if total > 0 && downloaded != total {
        return Err(format!("下載 {} 不完整（{downloaded}/{total}）", d.label));
    }
    Ok(())
}

fn extract(zip_path: &Path, dir: &Path, entries: &[(&str, &str)]) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("壓縮檔損壞：{e}"))?;
    for (suffix, dest_name) in entries {
        let index = (0..archive.len())
            .find(|&i| archive.by_index(i).map(|f| f.name().replace('\\', "/").ends_with(suffix)).unwrap_or(false))
            .ok_or_else(|| format!("壓縮檔裡找不到 {suffix}"))?;
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let tmp = dir.join(format!("{dest_name}.extract"));
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        drop(out);
        replace_file(&tmp, &dir.join(dest_name))?;
    }
    Ok(())
}

fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        std::fs::remove_file(to).map_err(|e| format!("無法覆蓋 {}（是否正在錄製或轉檔中？）：{e}", to.display()))?;
    }
    std::fs::rename(from, to).map_err(|e| e.to_string())
}
