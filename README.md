# YT 直播錄製工具

貼上 YouTube 直播網址就錄：錄的是串流本身（不是錄螢幕），邊錄邊切段，每段錄完自動轉成公司 Avid 用的 MXF。

## 用法

1. 第一次開：到「設定」按「下載工具」（ffmpeg、yt-dlp），順便設「預設存檔路徑」和「每段長度」。
2. 「錄製」頁貼網址（可以先按「查詢」看標題和狀態）→ 填名稱 → 選存檔路徑、錄製起點、要不要排程 → 「開始錄製」。
3. 檔案：名稱 `香蕉蛋糕` →
   ```
   <存檔路徑>\香蕉蛋糕\香蕉蛋糕_001.mxf
   <存檔路徑>\香蕉蛋糕\香蕉蛋糕_002.mxf
   <存檔路徑>\香蕉蛋糕\錄製紀錄.txt      ← 開始、斷線空檔、結束時間
   <存檔路徑>\香蕉蛋糕\_ts\香蕉蛋糕_001.ts ← 原始錄影（設定可選轉完刪掉）
   ```
   轉檔中的檔案叫 `*.mxf.part`，轉完才改名，剪輯看到的 `.mxf` 一定是完整的。同名再錄一次會接著編號，不會蓋掉。

- 直播還沒開始：自動等，開播就錄。直播結束：自動停止。
- 斷線、串流網址過期（約 6 小時）、90 秒沒資料：自動重新抓網址接著錄，空檔記進紀錄。
- 「從直播開頭錄」：第一次連線從頭抓（DVR 範圍內），之後斷線重連一律從現在接著錄。
- 錄製中電腦不會進入睡眠；關閉程式會先問，確定後讓目前那段存好再關。
- YouTube 改版錄不到時，先按「設定 → 更新 yt-dlp」。

## MXF 規格

全部集中在 [`core/src/spec.rs`](core/src/spec.rs)，換規格只改那裡。目前照 EBC 轉檔器產出檔的 ffprobe 結果：

MXF OP1a ／ MPEG-2 4:2:2 Profile @ High Level，50 Mb/s CBR，VBV 17825792 ／ 1920×1080 29.97i 上場優先 ／
BT.709 ／ PCM 24-bit 48 kHz × 8 個單聲道軌（1 左、2 右、3～8 靜音）／ Timecode 00:00:00:00。

- 來源 50/60p：先轉 59.94p 再兩幀合成一個交錯幀（動作順）。30p 以下：直接 29.97 標成交錯。
- 比例不是 16:9（例如直式直播）：補黑邊。
- ffprobe 看不出來、照 XDCAM HD422 慣例設的：GOP 15、2 張 B 幀。**第一次實際進 Avid 時確認。**

## 開發

```
app/src        畫面（原生 HTML/JS，不用 Node）
app/src-tauri  Tauri 後端：recorder.rs（錄製、重連、轉檔佇列）、tools.rs（下載工具）
core           純邏輯：spec（MXF 規格）、naming、record／ytdlp（參數組裝和解析）、probe、settings
```

本機 Rust 若是 GNU 工具鏈（缺 dlltool），用 MSVC 版：

```
cargo +stable-x86_64-pc-windows-msvc test -p yt-live-recorder-core
cargo +stable-x86_64-pc-windows-msvc run -p yt-live-recorder
```

需要 ffmpeg／網路的測試平常不跑：

```
set FFMPEG_DIR=<有 ffmpeg.exe、ffprobe.exe、yt-dlp.exe 的資料夾>
cargo test -p yt-live-recorder-core --test mxf_real_ffmpeg -- --ignored    :: 轉出來的 MXF 符合規格
set YT_LIVE_URL=https://www.youtube.com/@某頻道/live
cargo test -p yt-live-recorder-core --test live_record -- --ignored        :: 實際錄 25 秒直播再轉 MXF
```

### 發布

`scripts/release.sh 0.1.1 "說明"` → 推標籤 → GitHub Actions 打包安裝檔並發布，已安裝的程式會提示更新。
repo 要先設好 secrets `TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（跟關鍵標題整理工具同一組金鑰）。

## 之後的階段

- 多路任務管理畫面（後端已經支援同時多個任務）
- 內建 HTTP 伺服器：狀態 API、監看網頁、縮圖、低畫質即時預覽、保留的控制 API
- 硬碟空間警告
- 會員限定直播（瀏覽器 cookies）、yt-dlp 的 JS runtime（deno）
