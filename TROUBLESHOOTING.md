# DSH Desk 排障手册

记录 v0.1.0 → v0.1.2 迭代中实测定位的四个坑。症状 → 根因 → 诊断方法 → 修复方案。
升级 dsh / Node / 打包链路前建议先过一遍本文。

## 快速定位入口

**一切问题的第一现场：`%APPDATA%\dsh-desktop\harness\dsh-desk-sidecar.log`**

自 v0.1.1 起，壳会把 spawn 的完整 argv/cwd、子进程 stdout/stderr 全量追加到这个文件。
v0.1.2 起每次启动有 `===== shell spawn: cwd=… args=… =====` 分隔头，按启动次数分段查看即可。

| 症状 | 直接跳 |
|---|---|
| 窗口一直「正在启动 DSH 服务…」，日志里没有 plugin 加载行 | → 坑 4（EISDIR） |
| 窗口一直转圈，日志里服务正常、有 `dsh web:` 行 | → 坑 1（stdout 缓冲，旧版）或 token 未捕获 |
| 窗口弹了但显示「127.0.0.1 拒绝连接」 | → 坑 2（Node 版本）或坑 3（双开锁） |
| 能打开但停在 401 / 登录墙 | → 坑 5（token）或坑 6（Strict cookie） |
| 日志里 `already owned by process <PID>` | → 坑 3（双开锁），这是预期互斥不是 bug |

---

## 坑 1：窗口永远不显示 —— node stdout 管道块缓冲（v0.1.0 初版）

**症状**：服务其实在后台活着（端口在监听），但窗口永远不弹。

**根因**：初版壳靠解析子进程 stdout 里的 `dsh web: http://127.0.0.1:PORT` 行来拿端口号。
Node 的 stdout 在管道下是块缓冲，这行日志可能长期不 flush，壳永远等不到。

**诊断**：手动跑 sidecar 看监听端口 vs 捕获到的 stdout 是否缺 URL 行：

```powershell
cd "$env:LOCALAPPDATA\DSH Desk"
.\node.exe dsh-runtime\node_modules\@deepseek-ai\dsh\lib\bin.js web --no-open --port 0
# 另开窗口 netstat -ano | findstr node 的监听端口，对比 stdout 有没有 URL 行
```

**修复**（v0.1.0 已含）：壳自己 `TcpListener::bind("127.0.0.1:0")` 选空闲端口，
用 `--port <p>` 显式传给 dsh，不再从 stdout 拿端口。
**注意：端口不靠 stdout 了，但 token 仍必须靠 stdout（见坑 5），stdout 捕获不能省。**

## 坑 2：服务启动约 35 秒后崩溃 —— Node 22.14.0 缺 zstd API（v0.1.0）

**症状**：窗口弹出后很快变「127.0.0.1 拒绝连接」；日志里：

```
SyntaxError: The requested module 'node:zlib' does not provide an export named 'createZstdDecompress'
```

**根因**：`dsh-session-persistence-jsonl` 插件需要 `node:zlib` 的 zstd API
（`createZstdDecompress` 等），Node **22.15.0+ / 23.8.0+** 才加入，22.14.0 没有。
插件加载到一半进程直接崩 → 服务消失 → WebView 拒绝连接。

**修复**（v0.1.0 已含）：`scripts/prepare-runtime.mjs` 的 `NODE_VERSION` 升到 **24.19.0**，
与用户全局 dsh 日常验证过的版本完全对齐。
**升级纪律：捆绑 Node 版本永远跟随「上游 dsh 官方支持且已被日常使用验证」的版本，不要自创组合。**

## 坑 3：双开必崩 —— harness 独占锁（预期行为，不是 bug）

**症状**：日志里：

```
Error: task-board ledger is already owned by process <PID>
```

**根因**：第三方插件 `@linxin666/dsh-client-ui-task-board`（及类似有账本的插件）对
harness 数据目录加**进程独占锁**。同一 harness 上第二个实例（无论壳还是手动 `dsh web`）必崩。

**处理**：
- 全局 `dsh web` 和 DSH Desk **二选一**，不要同时跑；
- 检查开机自启（启动文件夹里的 `DSH后台启动.vbs` 之类），避免重启后旧实例悄悄回来抢锁；
- v0.1.1 起壳会识别该错误并弹通知带占用方 PID，按提示 `taskkill /PID <PID> /T /F` 即可。

## 坑 4：一启动就崩 `EISDIR: lstat 'C:'`（v0.1.1 实测）

**症状**：窗口卡「正在启动」，日志里 node 没跑到业务代码就死：

```
Error: EISDIR: illegal operation on a directory, lstat 'C:'
    at resolveMainPath (node:internal/modules/run_main:35:21)
```

**根因**：sidecar 传**绝对路径**入口（含盘符、空格 `DSH Desk`、潜在中文）时，
在某台实测机器上参数被截断成字面量 `C:`，node 把它当脚本名解析 → EISDIR。

**修复**（v0.1.2 已含）：`current_dir(resource_dir)` + **相对路径**传入口
（`dsh-runtime/node_modules/@deepseek-ai/dsh/lib/bin.js`），参数里不再出现盘符和空格。
**纪律：传给 sidecar 的路径参数一律相对化，别赌绝对路径的转义/截断行为。**

## 坑 5：401 登录墙 / 「无法登录」—— URL 必须带每进程 token（v0.1.2 前一直存在）

**症状**：服务正常、端口在听，但页面是 401 / 登录墙，怎么都进不去。

**根因**：dsh web 的访问 URL 必须带 `?token=<每进程随机值>`。源码确认
（`dsh-client-connection` 的 `processLaunchToken`）：token 由 `randomBytes` 每进程生成，
**无环境变量、无配置项可固定**，唯一出口是子进程 stdout 打印的
`dsh web: http://127.0.0.1:PORT/?token=…` 行。导航到裸地址一律 401。

**验证方法**：

```powershell
# 从日志拿最新 token URL，对比有无 token 的差异：
curl -D - "http://127.0.0.1:<port>/?token=<token>"   # 期望 303 + set-cookie
curl -c jar -b jar -L -o NUL -w "%{http_code}" "<同上>"  # 跟随后 200
curl -o NUL -w "%{http_code}" "http://127.0.0.1:<port>/"  # 裸地址 401
```

**修复**（v0.1.2 已含）：壳在排空 stdout 时用 `extract_dsh_url` 捕获完整带 token 的 URL
（兼容行尾 ` (LAN: …)` 后缀），窗口跳转到该 URL。token 与端口齐备才导航。

## 坑 6：token 带上了还是 401 —— eval 脚本导航扣下 SameSite=Strict cookie（v0.1.2）

**症状**：页面显示 `dsh web authentication required; reopen the URL printed by dsh web.`，
但日志里 token URL 明明已捕获、curl 验证 303→200 流程完全正常。

**根因**：v0.1.2 用 `win.eval("location.replace('<token URL>')")` 发起导航。
dsh 认证是「token URL → 303 + `Set-Cookie: SameSite=Strict` → 重定向到干净 `/`」流程；
而脚本发起的导航带着 **跨站 initiator**（加载页在 `tauri.localhost` 域），
Chromium 会在重定向的后续请求上**扣下 Strict cookie** → 服务端看到无凭证请求 → 401。
手动粘贴网址或用 Chrome 打开没这个问题（无 initiator，等同同站）。

**修复**（v0.1.3 已含）：改用 **`WebviewWindow::navigate()`**——浏览器进程发起导航，
等同用户手动输入网址，Strict cookie 正常携带。
**纪律：壳内跳转到 DSH 一律用 `navigate()`，永远不要 eval location 跳转。**

**验证**：装完启动后窗口应直接进 DSH 对话界面；若仍是 401 文字页即此坑复发。

---

## 维护流程（升级 dsh / Node 时）

1. 只改 `scripts/prepare-runtime.mjs` 头部的版本常量（NODE_VERSION / dsh 版本）；
2. `tauri.conf.json` 版本号 +1（updater 靠它识别新版本，已装机用户自动收更新）；
3. commit → 打新 tag（`v0.x.y`）→ push，CI（`.github/workflows/release.yml`）自动出
   NSIS 安装包 + `latest.json`；
4. 装机验证清单：
   - [ ] `dsh-desk-sidecar.log` 出现本次 `shell spawn` 头
   - [ ] 日志出现 `dsh web: http://…?token=…` 行
   - [ ] 窗口自动跳转到 DSH 界面（不是 401、不是转圈）
   - [ ] 连续运行 5 分钟 node 不崩（坑 2 的 35 秒、坑 3 的 30 秒都在这个窗口内）
5. 若翻车：先看日志对照本文表格；新症状按「坑 X」格式补充进本文。

## CI / 发布链路的坑（一次性，已修复，备查）

- tauri-action 创建 Release 必须显式传 `GITHUB_TOKEN`，且 workflow 需
  `permissions: contents: write`，否则 Release 创建失败；
- Windows 上 spawn npm/pnpm 要走 `cmd.exe` 包装（Node 20+ 对 .cmd 直接 spawn 报 EINVAL）；
- 退出/更新前必须 `taskkill /T /F` 杀整棵进程树——dsh 会再 spawn 孙进程，只杀直接子进程会留残留。
