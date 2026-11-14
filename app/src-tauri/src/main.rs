#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod recorder;
mod tools;

use recorder::{NewTask, Recorder, TaskView};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager, State};
use yt_live_recorder_core::ytdlp::VideoStatus;
use yt_live_recorder_core::Settings;

type Rec<'a> = State<'a, Arc<Recorder>>;

// --- 設定：存在使用者設定資料夾的 settings.json ---

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map(|d| d.join("settings.json")).map_err(|e| e.to_string())
}

fn load_settings(app: &tauri::AppHandle) -> Settings {
    settings_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
        .unwrap_or_default()
        .normalized()
}

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> Settings {
    load_settings(&app)
}

#[tauri::command]
fn save_settings(app: tauri::AppHandle, settings: Settings) -> Result<Settings, String> {
    let settings = settings.normalized();
    let path = settings_path(&app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(settings)
}

// --- 錄製任務 ---

#[tauri::command(async)]
fn check_url(app: tauri::AppHandle, url: String) -> Result<VideoStatus, String> {
    recorder::check_url(&app, &url)
}

#[tauri::command(async)]
fn task_create(rec: Rec, task: NewTask) -> Result<TaskView, String> {
    rec.create(task)
}

#[tauri::command]
fn task_list(rec: Rec) -> Vec<TaskView> {
    rec.list()
}

#[tauri::command]
fn task_stop(rec: Rec, id: u32) {
    rec.stop(id)
}

#[tauri::command]
fn task_remove(rec: Rec, id: u32) -> Result<(), String> {
    rec.remove(id)
}

#[tauri::command]
fn task_retry(rec: Rec, id: u32, index: u32) -> Result<(), String> {
    rec.retry(id, index)
}

/// 用檔案總管打開資料夾
#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    if !std::path::Path::new(&path).exists() {
        return Err(format!("找不到 {path}"));
    }
    std::process::Command::new("explorer").arg(&path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

// --- 關閉程式 ---

static CLOSING: AtomicBool = AtomicBool::new(false);

/// 畫面確認後呼叫：停止所有錄製（讓 ffmpeg 把最後一段寫完）再結束。
#[tauri::command(async)]
fn quit_app(app: tauri::AppHandle, rec: Rec) {
    CLOSING.store(true, Ordering::SeqCst);
    rec.stop_all();
    rec.wait_idle(Duration::from_secs(20));
    recorder::kill_converts();
    app.exit(0);
}

fn main() {
    let rec = Recorder::new();
    tauri::Builder::default()
        // 必須第一個註冊：再開一次程式時把已開的視窗叫到前面，不開第二個（避免同一路錄兩次）
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(rec.clone())
        .setup(move |app| {
            let settings = load_settings(app.handle());
            rec.start_workers(app.handle().clone(), settings.convert_workers);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if CLOSING.load(Ordering::SeqCst) {
                    return;
                }
                let rec = window.state::<Arc<Recorder>>();
                let recording = rec.any_active();
                let converting = rec.pending_converts();
                if recording || converting > 0 {
                    // 還在錄或還在轉檔：先問使用者（見 app.js 的 close-requested）
                    api.prevent_close();
                    let _ = window.emit("close-requested", serde_json::json!({ "recording": recording, "converting": converting }));
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            check_url,
            task_create,
            task_list,
            task_stop,
            task_remove,
            task_retry,
            reveal,
            quit_app,
            tools::tools_status,
            tools::install_tools
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
