# DSH Desk —— DSH 桌面壳（Tauri 2 + Sidecar + Updater）

极简壳：不改 DSH 网页 UI、不加按钮；自包含（内置 node.exe + pnpm.exe + dsh 整树）；
通过 GitHub Releases 自动更新。替代「手动 dsh web + Chrome 安装为应用」的工作流。

## 架构一览

```
安装后（resource_dir）
├── node-<triple>.exe          # sidecar，官方 Node 22 LTS
└── dsh-runtime/
    ├── node_modules/@deepseek-ai/dsh/   # dsh 固定版本整树
    └── pnpm.exe               # dsh plugin 子命令需要
```

- 启动：`node bin.js web --no-open --port 0` → 解析 stdout URL 行 → 端口就绪 → 显示窗口并跳转
- 数据：显式设 `DSH_HOME=%APPDATA%\dsh-desktop\harness`（沿用现役插件/凭证/会话，零迁移）
- 托盘：左键切换显隐；右键「显示主窗口 / 检查更新 / 完全退出」
- 退出/更新前 `taskkill /T /F` 杀整棵进程树（dsh 有孙进程）
- 单实例：二次启动只聚焦已有窗口

## 你需要做的（一次性）

### 1. 建 public 仓库并推送

在 GitHub 建一个 **public** 空仓库（不要勾选 README/gitignore），然后：

```powershell
cd "<本目录>/dsh-desk"
git init
git add .
git commit -m "DSH Desk v0.1.0"
git remote add origin https://github.com/<你的用户名>/<仓库名>.git
git branch -M main
git push -u origin main
```

### 2. 回填 updater 配置里的两个占位符

### 2. updater 配置（已完成）

`src-tauri/tauri.conf.json` 的 endpoints 已指向
`https://github.com/alexguohongning-jpg/dsh-desk/releases/latest/download/latest.json`，
pubkey 已回填。换仓库时改这一处即可。

### 3. 配置 GitHub Secrets

仓库 → Settings → Secrets and variables → Actions → New repository secret：

| Secret 名 | 值 |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | `.keys/desk.key` 文件的**完整内容** |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 留空字符串（当前密钥未设密码） |

> `.keys/desk.key` 是更新通道的控制权（等于谁有它谁就能给你的壳推更新）。
> 填完 Secrets 后请把它移出 OneDrive/工作区，离线妥善保管，**绝不能提交进 git**。

### 4. 打 tag 触发构建

```powershell
git tag v0.1.0
git push origin v0.1.0
```

Actions 会自动：组装 runtime → 构建 NSIS 安装包 → 签名 → 生成 `latest.json` → 发布 Release。
从 Release 下载 `*-setup.exe` 安装即可。

## 验收清单

- [ ] 双击安装（无需管理员，currentUser 模式），无 Node 环境的电脑可用
- [ ] 启动后显示「正在启动 DSH 服务…」→ 自动进入 DSH 界面（不弹系统浏览器）
- [ ] 托盘左键切换显隐；右键含「显示主窗口 / 检查更新 / 完全退出」
- [ ] 「完全退出」后任务管理器中无残留 node.exe（进程树被杀干净）
- [ ] 关闭窗口 = 隐藏到托盘，服务继续跑
- [ ] 二次双击图标 = 聚焦已有窗口（不出现第二个服务）
- [ ] 插件市场安装/卸载插件正常（内置 pnpm 生效）
- [ ] 发 v0.1.1 后：托盘「检查更新」能发现并自动安装重启

## 常用维护操作

| 操作 | 做法 |
|---|---|
| 升级 dsh 版本 | 改 `scripts/prepare-runtime.mjs` 顶部 `DSH_VERSION` → 版本号+1 → commit → tag |
| 升级 Node | 同上，改 `NODE_VERSION` |
| 换官方黑鲸鱼图标 | 拿到 logo.png 覆盖根目录 `logo.png` → `npx @tauri-apps/cli icon logo.png` → commit |
| 本地调试（可选，需 Rust） | `npm install` → `npm run tauri dev` |
| 重新生成签名密钥 | `npx @tauri-apps/cli signer generate -w .keys/desk.key`（换 pubkey 后旧版本收不到新更新，属预期） |

## 已知边界与取舍

1. **更新 = 全量下载**（约 100-130 MB/次）：node+dsh 整包捆绑的代价，个人使用可接受。
2. **与手动 `dsh web` 并存**：壳用 `--port 0` 随机端口，不抢 3080；但同一 harness
   不建议两个服务同时写 profile，调试手动实例时请先退出壳。
3. **WebView2**：Win10/11 一般自带；极老机器首次安装需联网下载（NSIS bootstrapper 默认行为）。
4. **dsh 尚在 0.1.x-rc 阶段**：CLI 行为变化（如 `--no-open` flag 改名）需跟进
   `prepare-runtime.mjs` 的版本号与 `main.rs` 的启动参数。
5. 首次 CI 构建若因 Tauri 插件 crate 小版本 API 差异编译失败，把 Actions 日志贴回来即可对症修。
