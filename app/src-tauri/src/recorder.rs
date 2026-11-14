//! 錄製任務：排程 → 等開播 → 錄成分段 .ts → 每段錄完丟進轉檔佇列轉 MXF。
//! 斷線或串流網址過期時自動重新抓網址接著錄（分段號接續），空檔寫進紀錄；直播結束自動停止。
//!
//! 後端一開始就支援多個任務同時跑（之後的多路錄製、監看 API 直接用 `Recorder::list`）。

use crate::tools::{self, command};
use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::AppHandle;
use yt_live_recorder_core::ytdlp::{self, LiveStatus};
use yt_live_recorder_core::{naming, probe, record, spec};

/// 串流多久沒有新資料就當成卡住，砍掉重連
const STALL_TIMEOUT: Duration = Duration::from_secs(90);
/// 等開播時多久查一次
const UPCOMING_POLL: Duration = Duration::from_secs(30);
/// 中斷後隔多久重連
const RETRY_DELAY: Duration = Duration::from_secs(5);

// --- 對外的資料結構 ---

#[derive(Clone, Debug, Deserialize)]
pub struct NewTask {
    pub url: String,
    pub name: String,
    pub output_dir: String,
    /// `now`：從現在開始錄；`start`：從直播開頭錄
    pub mode: String,
    /// 排程開始時間（Unix 秒）；None 就馬上開始
    pub start_at: Option<i64>,
    pub segment_minutes: u32,
    pub delete_ts: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// 等排程時間
    Scheduled,
    /// 等開播
    Waiting,
    Recording,
    Reconnecting,
    /// 直播結束（錄完）
    Ended,
    /// 使用者按停止
    Stopped,
    Error,
}

impl Phase {
    fn active(self) -> bool {
        matches!(self, Phase::Scheduled | Phase::Waiting | Phase::Recording | Phase::Reconnecting)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SegStatus {
    Recording,
    Queued,
    Converting,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct Segment {
    pub index: u32,
    pub status: SegStatus,
    /// 轉檔進度 0～1
    pub progress: f64,
    pub error: Option<String>,
    pub ts_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Gap {
    pub from: String,
    pub to: String,
    pub seconds: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskView {
    pub id: u32,
    pub name: String,
    pub url: String,
    pub folder: String,
    pub mode: String,
    pub start_at: Option<i64>,
    pub segment_minutes: u32,
    pub title: String,
    pub phase: Phase,
    pub message: String,
    pub recording_since: Option<i64>,
    pub segments: Vec<Segment>,
    pub gaps: Vec<Gap>,
    pub log: Vec<String>,
}

// --- 內部 ---

struct State {
    phase: Phase,
    message: String,
    title: String,
    recording_since: Option<i64>,
    segments: BTreeMap<u32, Segment>,
    gaps: Vec<Gap>,
    log: VecDeque<String>,
}

pub struct Task {
    id: u32,
    cfg: NewTask,
    folder: PathBuf,
    /// 這個任務從第幾段開始（同名資料夾裡之前的檔案不算這個任務的）
    first_index: u32,
    stop: AtomicBool,
    state: Mutex<State>,
}

impl Task {
    fn out_dir(&self) -> &Path {
        Path::new(&self.cfg.output_dir)
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn set(&self, phase: Phase, message: impl Into<String>) {
        let mut s = self.state.lock().unwrap();
        s.phase = phase;
        s.message = message.into();
    }

    /// 寫進畫面上的紀錄和資料夾裡的「錄製紀錄.txt」
    fn log(&self, line: impl AsRef<str>) {
        let line = format!("{} {}", Local::now().format("%Y-%m-%d %H:%M:%S"), line.as_ref());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.folder.join(naming::LOG_FILE)) {
            let _ = writeln!(f, "{line}");
        }
        let mut s = self.state.lock().unwrap();
        s.log.push_back(line);
        while s.log.len() > 300 {
            s.log.pop_front();
        }
    }

    fn sleep(&self, d: Duration) -> bool {
        let end = Instant::now() + d;
        while Instant::now() < end {
            if self.stopped() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        !self.stopped()
    }

    fn view(&self) -> TaskView {
        let s = self.state.lock().unwrap();
        TaskView {
            id: self.id,
            name: self.cfg.name.clone(),
            url: self.cfg.url.clone(),
            folder: self.folder.display().to_string(),
            mode: self.cfg.mode.clone(),
            start_at: self.cfg.start_at,
            segment_minutes: self.cfg.segment_minutes,
            title: s.title.clone(),
            phase: s.phase,
            message: s.message.clone(),
            recording_since: s.recording_since,
            segments: s.segments.values().cloned().collect(),
            gaps: s.gaps.clone(),
            log: s.log.iter().rev().take(80).rev().cloned().collect(),
        }
    }
}

struct Job {
    task: Arc<Task>,
    index: u32,
}

pub struct Recorder {
    tasks: Mutex<Vec<Arc<Task>>>,
    next_id: AtomicU32,
    jobs: Mutex<Option<Sender<Job>>>,
    app: Mutex<Option<AppHandle>>,
}

impl Recorder {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { tasks: Mutex::default(), next_id: AtomicU32::new(1), jobs: Mutex::default(), app: Mutex::default() })
    }

    /// 開轉檔工人（程式啟動時呼叫一次；工人數改了要重開程式）
    pub fn start_workers(self: &Arc<Self>, app: AppHandle, workers: u32) {
        *self.app.lock().unwrap() = Some(app.clone());
        let (tx, rx) = channel::<Job>();
        *self.jobs.lock().unwrap() = Some(tx);
        let rx = Arc::new(Mutex::new(rx));
        for _ in 0..workers.max(1) {
            let rx = rx.clone();
            let app = app.clone();
            std::thread::spawn(move || convert_worker(app, rx));
        }
    }

    pub fn list(&self) -> Vec<TaskView> {
        self.tasks.lock().unwrap().iter().map(|t| t.view()).collect()
    }

    pub fn any_active(&self) -> bool {
        self.tasks.lock().unwrap().iter().any(|t| t.state.lock().unwrap().phase.active())
    }

    /// 還在等待轉檔或轉檔中的段數（關程式前提醒用）
    pub fn pending_converts(&self) -> usize {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .map(|t| {
                t.state.lock().unwrap().segments.values().filter(|s| matches!(s.status, SegStatus::Queued | SegStatus::Converting)).count()
            })
            .sum()
    }

    pub fn create(self: &Arc<Self>, mut cfg: NewTask) -> Result<TaskView, String> {
        cfg.url = cfg.url.trim().to_string();
        if !cfg.url.starts_with("http") {
            return Err("請貼上 YouTube 直播網址".into());
        }
        cfg.name = naming::sanitize(&cfg.name);
        if cfg.name.is_empty() {
            return Err("請填名稱（會用來建資料夾和檔名）".into());
        }
        if cfg.output_dir.trim().is_empty() {
            return Err("請選存檔路徑".into());
        }
        cfg.segment_minutes = cfg.segment_minutes.clamp(1, 240);
        let out = PathBuf::from(cfg.output_dir.trim());
        if !out.is_dir() {
            return Err(format!("存檔路徑不存在或連不到：{}", out.display()));
        }
        {
            let tasks = self.tasks.lock().unwrap();
            if tasks.iter().any(|t| t.cfg.name == cfg.name && t.cfg.output_dir == cfg.output_dir && t.state.lock().unwrap().phase.active()) {
                return Err(format!("「{}」已經在錄了", cfg.name));
            }
        }
        let folder = naming::task_dir(&out, &cfg.name);
        std::fs::create_dir_all(naming::ts_dir(&out, &cfg.name)).map_err(|e| format!("建不了資料夾 {}：{e}", folder.display()))?;
        let first_index = naming::max_index(&out, &cfg.name).map_or(1, |i| i + 1);
        let task = Arc::new(Task {
            id: self.next_id.fetch_add(1, Ordering::SeqCst),
            cfg,
            folder,
            first_index,
            stop: AtomicBool::new(false),
            state: Mutex::new(State {
                phase: Phase::Scheduled,
                message: String::new(),
                title: String::new(),
                recording_since: None,
                segments: BTreeMap::new(),
                gaps: Vec::new(),
                log: VecDeque::new(),
            }),
        });
        self.tasks.lock().unwrap().push(task.clone());
        let me = self.clone();
        let t = task.clone();
        std::thread::spawn(move || {
            keep_awake();
            me.run(&t);
        });
        Ok(task.view())
    }

    pub fn stop(&self, id: u32) {
        if let Some(t) = self.find(id) {
            t.stop.store(true, Ordering::SeqCst);
        }
    }

    pub fn stop_all(&self) {
        for t in self.tasks.lock().unwrap().iter() {
            t.stop.store(true, Ordering::SeqCst);
        }
    }

    /// 等所有錄製結束（停止後 ffmpeg 要把最後一段寫完）
    pub fn wait_idle(&self, timeout: Duration) {
        let end = Instant::now() + timeout;
        while self.any_active() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn remove(&self, id: u32) -> Result<(), String> {
        let mut tasks = self.tasks.lock().unwrap();
        let Some(i) = tasks.iter().position(|t| t.id == id) else { return Ok(()) };
        let s = tasks[i].state.lock().unwrap();
        if s.phase.active() {
            return Err("請先停止錄製".into());
        }
        if s.segments.values().any(|g| matches!(g.status, SegStatus::Queued | SegStatus::Converting)) {
            return Err("還有分段在轉檔，轉完才能移除".into());
        }
        drop(s);
        tasks.remove(i);
        Ok(())
    }

    pub fn retry(&self, id: u32, index: u32) -> Result<(), String> {
        let t = self.find(id).ok_or("找不到任務")?;
        {
            let mut s = t.state.lock().unwrap();
            let seg = s.segments.get_mut(&index).ok_or("找不到這一段")?;
            if seg.status != SegStatus::Failed {
                return Ok(());
            }
            seg.status = SegStatus::Queued;
            seg.error = None;
            seg.progress = 0.0;
        }
        self.enqueue(&t, index);
        Ok(())
    }

    fn find(&self, id: u32) -> Option<Arc<Task>> {
        self.tasks.lock().unwrap().iter().find(|t| t.id == id).cloned()
    }

    fn app(&self) -> AppHandle {
        self.app.lock().unwrap().clone().expect("轉檔工人還沒啟動")
    }

    fn enqueue(&self, task: &Arc<Task>, index: u32) {
        if let Some(tx) = self.jobs.lock().unwrap().as_ref() {
            let _ = tx.send(Job { task: task.clone(), index });
        }
    }

    // --- 錄製流程 ---

    fn run(&self, t: &Arc<Task>) {
        let mode_label = if t.cfg.mode == "start" { "從直播開頭錄" } else { "從現在開始錄" };
        t.log(format!("建立任務：{}（{mode_label}，每段 {} 分鐘）", t.cfg.url, t.cfg.segment_minutes));

        if let Some(at) = t.cfg.start_at {
            let when = fmt_unix(at);
            t.set(Phase::Scheduled, format!("排程 {when} 開始"));
            t.log(format!("排程 {when} 開始"));
            while Local::now().timestamp() < at {
                if !t.sleep(Duration::from_millis(500)) {
                    return self.finish(t, Phase::Stopped, "已取消排程");
                }
            }
        }

        let app = self.app();
        let mut recorded_once = false;
        let mut from_start = t.cfg.mode == "start";
        let mut lost_at: Option<chrono::DateTime<Local>> = None;
        let mut status_failures = 0;

        loop {
            if t.stopped() {
                return self.finish(t, Phase::Stopped, "已停止");
            }
            let status = match query_status(&app, &t.cfg.url) {
                Ok(s) => {
                    status_failures = 0;
                    s
                }
                Err(e) => {
                    status_failures += 1;
                    let phase = if recorded_once { Phase::Reconnecting } else { Phase::Waiting };
                    t.set(phase, format!("查不到直播狀態，稍後重試：{e}"));
                    if status_failures == 1 || status_failures % 20 == 0 {
                        t.log(format!("查直播狀態失敗：{e}"));
                    }
                    if !t.sleep(Duration::from_secs(15)) {
                        continue;
                    }
                    continue;
                }
            };
            if !status.title.is_empty() {
                t.state.lock().unwrap().title = status.title.clone();
            }
            match status.status {
                LiveStatus::IsLive => {}
                LiveStatus::IsUpcoming => {
                    let when = status.release_timestamp.map(|ts| format!("（預定 {}）", fmt_unix(ts))).unwrap_or_default();
                    if t.state.lock().unwrap().phase != Phase::Waiting {
                        t.log(format!("直播還沒開始，等待開播{when}"));
                    }
                    t.set(Phase::Waiting, format!("等待開播{when}"));
                    t.sleep(UPCOMING_POLL);
                    continue;
                }
                other => {
                    if recorded_once {
                        return self.finish(t, Phase::Ended, "直播已結束");
                    }
                    let msg = match other {
                        LiveStatus::PostLive | LiveStatus::WasLive => "這場直播已經結束了",
                        LiveStatus::NotLive => "這個網址不是直播",
                        _ => "看不出這個網址的直播狀態",
                    };
                    return self.finish(t, Phase::Error, msg);
                }
            }

            if let Some(since) = lost_at.take() {
                let now = Local::now();
                let gap = Gap {
                    from: since.format("%H:%M:%S").to_string(),
                    to: now.format("%H:%M:%S").to_string(),
                    seconds: (now - since).num_seconds(),
                };
                t.log(format!("重新連上，空檔 {} ～ {}（約 {} 秒）", gap.from, gap.to, gap.seconds));
                t.state.lock().unwrap().gaps.push(gap);
            }

            let start_index = naming::max_index(t.out_dir(), &t.cfg.name).map_or(t.first_index, |i| (i + 1).max(t.first_index));
            {
                let mut s = t.state.lock().unwrap();
                s.recording_since.get_or_insert(Local::now().timestamp());
            }
            t.set(Phase::Recording, if from_start { "從直播開頭錄製中" } else { "錄製中" });
            t.log(format!("開始錄製，從第 {start_index} 段{}", if from_start { "（從直播開頭）" } else { "" }));

            let result = if from_start { self.record_from_start(&app, t, start_index) } else { self.record_hls(&app, t, start_index) };
            // 第一次之後一律從現在接著錄：重新從頭會把整場再抓一次
            from_start = false;
            recorded_once = true;
            self.scan(t, false);

            if t.stopped() {
                return self.finish(t, Phase::Stopped, "已停止");
            }
            let reason = match result {
                Ok(()) => "串流結束".to_string(),
                Err(e) => e,
            };
            lost_at = Some(Local::now());
            t.log(format!("串流中斷：{reason}，確認直播狀態後重新連線"));
            t.set(Phase::Reconnecting, format!("重新連線中（{reason}）"));
            if !t.sleep(RETRY_DELAY) {
                continue;
            }
        }
    }

    fn finish(&self, t: &Arc<Task>, phase: Phase, msg: &str) {
        self.scan(t, false);
        t.log(msg);
        t.set(phase, msg);
    }

    /// 從現在開始錄：yt-dlp 取 HLS 網址 → ffmpeg -c copy 分段。
    fn record_hls(&self, app: &AppHandle, t: &Arc<Task>, start_index: u32) -> Result<(), String> {
        let ytdlp = tools::require(app, tools::YTDLP)?;
        let out = run_capture(&ytdlp, &ytdlp::hls_url_args(&t.cfg.url))?;
        let urls = ytdlp::parse_urls(&out);
        let Some(video) = urls.first() else {
            return Err(format!("取不到串流網址：{}", last_line(&out)));
        };
        let args = record::segment_args(
            &record::Input::Hls { video, audio: urls.get(1).map(String::as_str) },
            &naming::ts_pattern(t.out_dir(), &t.cfg.name),
            t.cfg.segment_minutes * 60,
            start_index,
        );
        self.run_ffmpeg(app, t, &args, Vec::new())
    }

    /// 從直播開頭錄：影像、聲音各開一個 yt-dlp（--live-from-start -o -），
    /// stdout 轉到本機 TCP，ffmpeg 從兩個 tcp:// 讀進來合併分段。
    fn record_from_start(&self, app: &AppHandle, t: &Arc<Task>, start_index: u32) -> Result<(), String> {
        let ytdlp = tools::require(app, tools::YTDLP)?;
        let mut helpers = Vec::new();
        let mut inputs = Vec::new();
        for format in [ytdlp::FROM_START_VIDEO, ytdlp::FROM_START_AUDIO] {
            let mut child = command(&ytdlp)
                .args(ytdlp::from_start_args(&t.cfg.url, format))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("yt-dlp 啟動失敗：{e}"))?;
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();
            let tc = t.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if line.contains("ERROR") {
                        tc.log(format!("yt-dlp：{line}"));
                    }
                }
            });
            let port = bridge(stdout)?;
            inputs.push(format!("tcp://127.0.0.1:{port}"));
            helpers.push(child);
        }
        let args = record::segment_args(
            &record::Input::Split { video: &inputs[0], audio: &inputs[1] },
            &naming::ts_pattern(t.out_dir(), &t.cfg.name),
            t.cfg.segment_minutes * 60,
            start_index,
        );
        self.run_ffmpeg(app, t, &args, helpers)
    }

    /// 跑錄製用的 ffmpeg，期間每 2 秒掃一次分段；按停止就送 `q` 讓 ffmpeg 把最後一段寫完整。
    fn run_ffmpeg(&self, app: &AppHandle, t: &Arc<Task>, args: &[String], mut helpers: Vec<Child>) -> Result<(), String> {
        let ffmpeg = tools::require(app, tools::FFMPEG)?;
        let mut child = command(&ffmpeg)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("ffmpeg 啟動失敗：{e}"))?;
        let mut stdin: Option<ChildStdin> = child.stdin.take();
        let errors = Arc::new(Mutex::new(VecDeque::<String>::new()));
        {
            let errors = errors.clone();
            let stderr = child.stderr.take().unwrap();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let mut e = errors.lock().unwrap();
                    e.push_back(line);
                    if e.len() > 20 {
                        e.pop_front();
                    }
                }
            });
        }

        let mut last_size = 0u64;
        let mut last_progress = Instant::now();
        let mut quit_sent: Option<Instant> = None;
        let status = loop {
            if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
                break st;
            }
            if t.stopped() {
                match quit_sent {
                    None => {
                        if let Some(mut s) = stdin.take() {
                            let _ = s.write_all(b"q");
                        }
                        quit_sent = Some(Instant::now());
                    }
                    Some(at) if at.elapsed() > Duration::from_secs(15) => {
                        let _ = child.kill();
                    }
                    _ => {}
                }
            }
            let size = self.scan(t, true);
            if size != last_size {
                last_size = size;
                last_progress = Instant::now();
            } else if quit_sent.is_none() && last_progress.elapsed() > STALL_TIMEOUT {
                t.log(format!("超過 {} 秒沒有收到新資料，重新連線", STALL_TIMEOUT.as_secs()));
                let _ = child.kill();
            }
            std::thread::sleep(Duration::from_secs(2));
        };
        for h in helpers.iter_mut() {
            let _ = h.kill();
            let _ = h.wait();
        }
        if status.success() || t.stopped() {
            Ok(())
        } else {
            let e = errors.lock().unwrap();
            Err(e.back().cloned().unwrap_or_else(|| format!("ffmpeg 結束（{status}）")))
        }
    }

    /// 掃 _ts 資料夾：新出現的段加進清單；錄完的段（不是最新那段，或 ffmpeg 已結束）送去轉檔。
    /// 回傳這個任務目前所有 .ts 的總大小（判斷串流有沒有卡住）。
    fn scan(&self, t: &Arc<Task>, running: bool) -> u64 {
        let dir = naming::ts_dir(t.out_dir(), &t.cfg.name);
        let mut files: Vec<(u32, u64)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let idx = naming::parse_index(&t.cfg.name, &e.file_name().to_string_lossy(), "ts")?;
                (idx >= t.first_index).then(|| (idx, e.metadata().map(|m| m.len()).unwrap_or(0)))
            })
            .collect();
        files.sort();
        let newest = files.last().map(|f| f.0);
        let total = files.iter().map(|f| f.1).sum();
        let mut ready = Vec::new();
        {
            let mut s = t.state.lock().unwrap();
            for (idx, size) in files {
                let seg = s.segments.entry(idx).or_insert(Segment {
                    index: idx,
                    status: SegStatus::Recording,
                    progress: 0.0,
                    error: None,
                    ts_bytes: 0,
                });
                if seg.status != SegStatus::Recording {
                    continue;
                }
                seg.ts_bytes = size;
                let complete = !running || Some(idx) != newest;
                if !complete {
                    continue;
                }
                if size == 0 {
                    // ffmpeg 結束時可能留下空的最後一段
                    s.segments.remove(&idx);
                    let _ = std::fs::remove_file(dir.join(naming::segment_file(&t.cfg.name, idx, "ts")));
                } else {
                    seg.status = SegStatus::Queued;
                    ready.push(idx);
                }
            }
        }
        for idx in ready {
            t.log(format!("第 {idx} 段錄完，排入轉檔"));
            self.enqueue(t, idx);
        }
        total
    }
}

// --- 轉檔 ---

/// 轉檔中的 ffmpeg（PID）。Windows 上主程式結束不會連帶結束子程序，關程式時要自己收掉。
static CONVERT_PIDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// 關程式前呼叫：結束所有轉檔中的 ffmpeg，等工人把 .part 清掉。
pub fn kill_converts() {
    let pids: Vec<u32> = CONVERT_PIDS.lock().unwrap().clone();
    if pids.is_empty() {
        return;
    }
    for pid in &pids {
        let _ = command(Path::new("taskkill")).args(["/PID", &pid.to_string(), "/F"]).output();
    }
    let end = Instant::now() + Duration::from_secs(5);
    while !CONVERT_PIDS.lock().unwrap().is_empty() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn convert_worker(app: AppHandle, rx: Arc<Mutex<Receiver<Job>>>) {
    loop {
        let job = match rx.lock().unwrap().recv() {
            Ok(j) => j,
            Err(_) => return,
        };
        let Job { task, index } = job;
        let set = |f: &dyn Fn(&mut Segment)| {
            if let Some(seg) = task.state.lock().unwrap().segments.get_mut(&index) {
                f(seg);
            }
        };
        set(&|s| {
            s.status = SegStatus::Converting;
            s.progress = 0.0;
        });
        let result = convert(&app, &task, index, &|p| set(&|s| s.progress = p));
        let name = naming::segment_file(&task.cfg.name, index, spec::EXTENSION);
        match result {
            Ok(()) => {
                set(&|s| {
                    s.status = SegStatus::Done;
                    s.progress = 1.0;
                });
                task.log(format!("{name} 轉檔完成"));
                if task.cfg.delete_ts {
                    let _ = std::fs::remove_file(naming::ts_dir(task.out_dir(), &task.cfg.name).join(naming::segment_file(&task.cfg.name, index, "ts")));
                }
            }
            Err(e) => {
                task.log(format!("{name} 轉檔失敗：{e}"));
                set(&|s| {
                    s.status = SegStatus::Failed;
                    s.error = Some(e.clone());
                });
            }
        }
    }
}

fn convert(app: &AppHandle, t: &Task, index: u32, on_progress: &dyn Fn(f64)) -> Result<(), String> {
    let ffmpeg = tools::require(app, tools::FFMPEG)?;
    let ffprobe = tools::require(app, tools::FFPROBE)?;
    let out = t.out_dir();
    let ts = naming::ts_dir(out, &t.cfg.name).join(naming::segment_file(&t.cfg.name, index, "ts"));
    let ts_str = ts.to_string_lossy().into_owned();
    let info = probe::parse(&run_capture(&ffprobe, &probe::args(&ts_str))?)?;
    let part = naming::mxf_part_path(out, &t.cfg.name, index);
    let dest = naming::mxf_path(out, &t.cfg.name, index);
    let args = spec::mxf_args(&ts_str, &part.to_string_lossy(), &info);

    let mut child = command(&ffmpeg)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("ffmpeg 啟動失敗：{e}"))?;
    let pid = child.id();
    CONVERT_PIDS.lock().unwrap().push(pid);
    // -stats 的進度用 \r 分隔，自己切
    let mut stderr = child.stderr.take().unwrap();
    let mut buf = Vec::new();
    let mut last_err = String::new();
    let mut chunk = [0u8; 4096];
    while let Ok(n) = stderr.read(&mut chunk) {
        if n == 0 {
            break;
        }
        for &b in &chunk[..n] {
            if b == b'\r' || b == b'\n' {
                let line = String::from_utf8_lossy(&buf).into_owned();
                buf.clear();
                if let (Some(sec), Some(total)) = (stats_time(&line), info.duration) {
                    if total > 0.0 {
                        on_progress((sec / total).clamp(0.0, 1.0));
                    }
                } else if !line.trim().is_empty() {
                    last_err = line;
                }
            } else {
                buf.push(b);
            }
        }
    }
    let status = child.wait();
    let ok = matches!(status, Ok(ref s) if s.success());
    if !ok {
        let _ = std::fs::remove_file(&part);
    }
    CONVERT_PIDS.lock().unwrap().retain(|&p| p != pid);
    let status = status.map_err(|e| e.to_string())?;
    if !ok {
        return Err(if last_err.is_empty() { format!("ffmpeg 結束（{status}）") } else { last_err });
    }
    if dest.exists() {
        std::fs::remove_file(&dest).map_err(|e| format!("無法覆蓋 {}：{e}", dest.display()))?;
    }
    std::fs::rename(&part, &dest).map_err(|e| format!("改檔名失敗：{e}"))
}

/// `-stats` 一行裡的 `time=00:01:02.50` → 62.5 秒
fn stats_time(line: &str) -> Option<f64> {
    let rest = &line[line.find("time=")? + 5..];
    let t = rest.split_whitespace().next()?;
    let mut secs = 0.0;
    for part in t.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(secs)
}

// --- 小工具 ---

fn query_status(app: &AppHandle, url: &str) -> Result<ytdlp::VideoStatus, String> {
    let ytdlp = tools::require(app, tools::YTDLP)?;
    let out = run_capture(&ytdlp, &ytdlp::status_args(url))?;
    ytdlp::parse_status(&out).ok_or_else(|| "yt-dlp 沒有回傳狀態".into())
}

/// 給畫面「先查一下」用：貼上網址後顯示標題和狀態
pub fn check_url(app: &AppHandle, url: &str) -> Result<ytdlp::VideoStatus, String> {
    query_status(app, url.trim())
}

/// 執行並回傳 stdout；失敗時回傳 stderr 最後一行。
fn run_capture(exe: &Path, args: &[String]) -> Result<String, String> {
    let out = command(exe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{} 啟動失敗：{e}", exe.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(last_line(&err).to_string())
    }
}

fn last_line(s: &str) -> &str {
    s.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("（沒有訊息）")
}

/// 把子程序的 stdout 接到本機一個 TCP port（只接受一次連線），回傳 port。
fn bridge(mut from: impl Read + Send + 'static) -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    std::thread::spawn(move || {
        if let Ok((mut conn, _)) = listener.accept() {
            let _ = std::io::copy(&mut from, &mut conn);
        }
    });
    Ok(port)
}

fn fmt_unix(ts: i64) -> String {
    Local.timestamp_opt(ts, 0).single().map(|d| d.format("%m/%d %H:%M").to_string()).unwrap_or_default()
}

/// 錄製中不讓電腦睡眠（只對呼叫的執行緒有效，執行緒結束就恢復）
fn keep_awake() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn SetThreadExecutionState(flags: u32) -> u32;
        }
        const ES_CONTINUOUS: u32 = 0x8000_0000;
        const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stats_time;

    #[test]
    fn parses_stats_time() {
        assert_eq!(stats_time("frame= 100 fps= 50 q=2.0 size= 1kB time=00:01:02.50 bitrate=1"), Some(62.5));
        assert_eq!(stats_time("no time here"), None);
    }
}
