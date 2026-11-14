const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;
const $ = (id) => document.getElementById(id);

let settings = null;

const PHASE_LABEL = {
  scheduled: "排程中",
  waiting: "等待開播",
  recording: "錄製中",
  reconnecting: "重新連線",
  ended: "已錄完",
  stopped: "已停止",
  error: "錯誤",
};
const SEG_LABEL = {
  recording: "錄製中",
  queued: "等待轉檔",
  converting: "轉檔中",
  done: "完成",
  failed: "轉檔失敗",
};
const ACTIVE = new Set(["scheduled", "waiting", "recording", "reconnecting"]);

function esc(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

function fmtDuration(sec) {
  sec = Math.max(0, Math.floor(sec));
  const h = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60), s = sec % 60;
  return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

function fmtBytes(n) {
  if (!n) return "";
  const u = ["B", "KB", "MB", "GB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i ? 1 : 0)} ${u[i]}`;
}

// --- 分頁 ---

document.querySelectorAll(".tab").forEach((btn) =>
  btn.addEventListener("click", () => {
    document.querySelectorAll(".tab").forEach((b) => b.classList.toggle("active", b === btn));
    $("tab-record").classList.toggle("hidden", btn.dataset.tab !== "record");
    $("tab-settings").classList.toggle("hidden", btn.dataset.tab !== "settings");
    if (btn.dataset.tab === "settings") refreshTools();
  })
);

// --- 設定 ---

function fillSettingsForm() {
  $("setOutputDir").value = settings.output_dir;
  $("setSegment").value = settings.segment_minutes;
  $("setWorkers").value = settings.convert_workers;
  $("setDeleteTs").checked = settings.delete_ts;
  document.querySelectorAll('input[name="setMode"]').forEach((r) => (r.checked = r.value === settings.default_mode));
  $("segmentHint").textContent = `每段 ${settings.segment_minutes} 分鐘（在「設定」調整）`;
}

function applyDefaultsToForm() {
  if (!$("outputDir").value) $("outputDir").value = settings.output_dir;
  document.querySelectorAll('input[name="mode"]').forEach((r) => (r.checked = r.value === settings.default_mode));
}

async function loadSettings() {
  settings = await invoke("get_settings");
  fillSettingsForm();
  applyDefaultsToForm();
}

$("saveSettings").addEventListener("click", async () => {
  try {
    settings = await invoke("save_settings", {
      settings: {
        output_dir: $("setOutputDir").value.trim(),
        segment_minutes: Number($("setSegment").value) || 10,
        default_mode: document.querySelector('input[name="setMode"]:checked')?.value || "now",
        convert_workers: Number($("setWorkers").value) || 2,
        delete_ts: $("setDeleteTs").checked,
      },
    });
    fillSettingsForm();
    $("settingsStatus").textContent = "已儲存";
  } catch (e) {
    $("settingsStatus").textContent = `⚠ ${e}`;
  }
});

async function pickDir(inputId) {
  const dir = await dialog.open({ directory: true, defaultPath: $(inputId).value || undefined });
  if (dir) $(inputId).value = dir;
}
$("pickDir").addEventListener("click", () => pickDir("outputDir"));
$("setPickDir").addEventListener("click", () => pickDir("setOutputDir"));

// --- 工具 ---

async function refreshTools() {
  const st = await invoke("tools_status");
  $("toolFfmpeg").textContent = (st.ffmpeg || "未安裝") + (st.ffmpeg_outdated ? "（版本不是指定版，建議重新下載）" : "");
  $("toolFfprobe").textContent = st.ffprobe || "未安裝";
  $("toolYtdlp").textContent = st.ytdlp ? `${st.ytdlp}（${st.ytdlp_version || "?"}）` : "未安裝";
  $("toolsWarning").classList.toggle("hidden", !!(st.ffmpeg && st.ffprobe && st.ytdlp));
  return st;
}

async function installTools(which) {
  const btns = [$("installAll"), $("updateYtdlp")];
  btns.forEach((b) => (b.disabled = true));
  $("toolsProgress").classList.remove("hidden");
  $("toolsStatus").textContent = "下載中…";
  try {
    await invoke("install_tools", { which });
    $("toolsStatus").textContent = "完成";
  } catch (e) {
    $("toolsStatus").textContent = `⚠ ${e}`;
  } finally {
    btns.forEach((b) => (b.disabled = false));
    $("toolsProgress").classList.add("hidden");
    refreshTools();
  }
}
$("installAll").addEventListener("click", () => installTools("all"));
$("updateYtdlp").addEventListener("click", () => installTools("ytdlp"));
listen("tools-progress", ({ payload }) => {
  const p = payload.total ? payload.downloaded / payload.total : 0;
  $("toolsProgress").value = p;
  $("toolsStatus").textContent = `下載 ${payload.label}：${fmtBytes(payload.downloaded)}${payload.total ? ` / ${fmtBytes(payload.total)}` : ""}`;
});

// --- 新增錄製 ---

const STATUS_LABEL = {
  is_live: "直播中",
  is_upcoming: "還沒開始",
  post_live: "剛結束",
  was_live: "已結束",
  not_live: "不是直播",
  unknown: "不明",
};

$("checkBtn").addEventListener("click", async () => {
  const url = $("url").value.trim();
  if (!url) return;
  $("checkResult").textContent = "查詢中…";
  try {
    const st = await invoke("check_url", { url });
    let text = `${STATUS_LABEL[st.status] || st.status}：${st.title}`;
    if (st.release_timestamp) text += `（預定 ${new Date(st.release_timestamp * 1000).toLocaleString()}）`;
    $("checkResult").textContent = text;
  } catch (e) {
    $("checkResult").textContent = `⚠ ${e}`;
  }
});

$("useSchedule").addEventListener("change", () => {
  $("startAt").disabled = !$("useSchedule").checked;
  if ($("useSchedule").checked && !$("startAt").value) {
    const d = new Date(Date.now() + 10 * 60000);
    d.setSeconds(0, 0);
    $("startAt").value = new Date(d - d.getTimezoneOffset() * 60000).toISOString().slice(0, 16);
  }
});

$("newTask").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  $("formError").textContent = "";
  let startAt = null;
  if ($("useSchedule").checked) {
    const t = new Date($("startAt").value).getTime();
    if (!t) return ($("formError").textContent = "請填開始時間");
    if (t > Date.now()) startAt = Math.floor(t / 1000);
  }
  $("startBtn").disabled = true;
  try {
    await invoke("task_create", {
      task: {
        url: $("url").value,
        name: $("name").value,
        output_dir: $("outputDir").value.trim(),
        mode: document.querySelector('input[name="mode"]:checked').value,
        start_at: startAt,
        segment_minutes: settings.segment_minutes,
        delete_ts: settings.delete_ts,
      },
    });
    $("url").value = "";
    $("name").value = "";
    $("checkResult").textContent = "";
    $("useSchedule").checked = false;
    $("startAt").disabled = true;
    refreshTasks();
  } catch (e) {
    $("formError").textContent = String(e);
  } finally {
    $("startBtn").disabled = false;
  }
});

// --- 任務清單 ---

const openLogs = new Set();

function taskCard(t) {
  const active = ACTIVE.has(t.phase);
  const done = t.segments.filter((s) => s.status === "done").length;
  const pending = t.segments.filter((s) => s.status === "queued" || s.status === "converting").length;
  let elapsed = "";
  if (t.recording_since && active) elapsed = `已錄 ${fmtDuration(Date.now() / 1000 - t.recording_since)}`;
  const rows = t.segments
    .slice()
    .reverse()
    .map((s) => {
      const file = `${t.name}_${String(s.index).padStart(3, "0")}.mxf`;
      let st = SEG_LABEL[s.status];
      if (s.status === "converting") st += ` <progress max="1" value="${s.progress}"></progress> ${Math.round(s.progress * 100)}%`;
      if (s.status === "failed") st += ` <button data-retry="${t.id}:${s.index}">重試</button><div class="hint">${esc(s.error)}</div>`;
      return `<tr><td>${s.index}</td><td>${esc(file)}</td><td>${fmtBytes(s.ts_bytes)}</td><td class="st-${s.status}">${st}</td></tr>`;
    })
    .join("");
  const gaps = t.gaps.length
    ? `<div class="gaps">斷線空檔：${t.gaps.map((g) => `${g.from}～${g.to}（${g.seconds} 秒）`).join("、")}</div>`
    : "";
  return `
    <div class="card" data-task="${t.id}">
      <div class="task-head">
        <h2>${esc(t.name)}</h2>
        <span class="phase ${t.phase}">${PHASE_LABEL[t.phase]}</span>
        <span class="task-title" title="${esc(t.title)}">${esc(t.title)}</span>
        ${active ? `<button data-stop="${t.id}">停止</button>` : `<button data-remove="${t.id}">移除</button>`}
        <button data-reveal="${esc(t.folder)}">開資料夾</button>
      </div>
      <div class="task-msg">${esc(t.message)} ${elapsed ? `<span class="hint">${elapsed}</span>` : ""}</div>
      <div class="hint">${esc(t.url)} ／ ${t.mode === "start" ? "從直播開頭" : "從現在"} ／ 每段 ${t.segment_minutes} 分鐘 ／ 轉好 ${done} 段${pending ? `，待轉 ${pending} 段` : ""}</div>
      ${gaps}
      ${rows ? `<table class="segs"><tr><th>段</th><th>檔名</th><th>錄影大小</th><th>狀態</th></tr>${rows}</table>` : ""}
      <details data-log="${t.id}" ${openLogs.has(t.id) ? "open" : ""}><summary>紀錄</summary><pre>${esc(t.log.join("\n"))}</pre></details>
    </div>`;
}

async function refreshTasks() {
  const tasks = await invoke("task_list");
  const html = tasks.slice().reverse().map(taskCard).join("");
  if ($("tasks").dataset.html !== html) {
    $("tasks").innerHTML = html;
    $("tasks").dataset.html = html;
  }
}

$("tasks").addEventListener("click", async (ev) => {
  const b = ev.target.closest("button");
  if (!b) return;
  try {
    if (b.dataset.stop) {
      if (await dialog.ask("確定要停止這一路錄製？", { title: "停止錄製", kind: "warning" })) {
        await invoke("task_stop", { id: Number(b.dataset.stop) });
      }
    } else if (b.dataset.remove) {
      await invoke("task_remove", { id: Number(b.dataset.remove) });
    } else if (b.dataset.reveal) {
      await invoke("reveal", { path: b.dataset.reveal });
    } else if (b.dataset.retry) {
      const [id, index] = b.dataset.retry.split(":").map(Number);
      await invoke("task_retry", { id, index });
    }
  } catch (e) {
    await dialog.message(String(e), { title: "錯誤", kind: "error" });
  }
  refreshTasks();
});

$("tasks").addEventListener(
  "toggle",
  (ev) => {
    const id = Number(ev.target.dataset?.log);
    if (!id) return;
    ev.target.open ? openLogs.add(id) : openLogs.delete(id);
  },
  true
);

// --- 關閉 ---

listen("close-requested", async ({ payload }) => {
  const lines = [];
  if (payload?.recording) lines.push("還有直播在錄製或等待中，關閉會停止錄製（目前這段會存好）。");
  if (payload?.converting) lines.push(`還有 ${payload.converting} 段在轉 MXF，關閉會中斷轉檔，這幾段不會有 .mxf（.ts 還在）。`);
  lines.push("確定要關閉嗎？");
  const ok = await dialog.ask(lines.join("\n\n"), { title: "關閉程式", kind: "warning" });
  if (ok) {
    $("tasks").insertAdjacentHTML("afterbegin", '<div class="warning">正在停止錄製並關閉…</div>');
    await invoke("quit_app");
  }
});

// --- 版本與自動更新 ---

let pendingUpdate = null;

async function checkForUpdate(silent) {
  if (!silent) $("updateStatus").textContent = "檢查中…";
  try {
    pendingUpdate = await window.__TAURI__.updater.check();
  } catch (e) {
    if (!silent) $("updateStatus").textContent = `⚠ 檢查更新失敗：${e}`;
    return;
  }
  const has = !!pendingUpdate;
  $("updateBadge").classList.toggle("hidden", !has);
  $("installUpdateBtn").classList.toggle("hidden", !has);
  if (has) {
    $("updateBadge").textContent = `有新版本 v${pendingUpdate.version}`;
    $("updateStatus").textContent = `有新版本 v${pendingUpdate.version}`;
  } else if (!silent) {
    $("updateStatus").textContent = "已經是最新版本。";
  }
}

$("checkUpdateBtn").addEventListener("click", () => checkForUpdate(false));
$("updateBadge").addEventListener("click", () => document.querySelector('.tab[data-tab="settings"]').click());
$("installUpdateBtn").addEventListener("click", async () => {
  if (!pendingUpdate) return;
  try {
    const res = await invoke("task_list");
    if (res.some((t) => ACTIVE.has(t.phase))) {
      $("updateStatus").textContent = "錄製中不能更新，請錄完再更新。";
      return;
    }
    $("installUpdateBtn").disabled = true;
    $("updateProgress").classList.remove("hidden");
    let got = 0, total = 0;
    await pendingUpdate.downloadAndInstall((ev) => {
      if (ev.event === "Started") total = ev.data.contentLength ?? 0;
      else if (ev.event === "Progress") {
        got += ev.data.chunkLength ?? 0;
        if (total) $("updateProgress").value = got / total;
      }
    });
    await window.__TAURI__.process.relaunch();
  } catch (e) {
    $("updateStatus").textContent = `⚠ 更新失敗：${e}`;
    $("installUpdateBtn").disabled = false;
  }
});

// --- 啟動 ---

(async () => {
  try {
    $("versionLabel").textContent = `目前版本 v${await window.__TAURI__.app.getVersion()}`;
  } catch (_) {}
  await loadSettings();
  await refreshTools();
  await refreshTasks();
  setInterval(refreshTasks, 1000);
  checkForUpdate(true);
})();
