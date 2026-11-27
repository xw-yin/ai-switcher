# AI Switcher

基于 **Rust + GPUI** 构建的轻量级 macOS 账号切换与额度监控工具，专注支持 **Codex** 与 **Antigravity**。

## 功能
- **Codex**：OAuth PKCE 浏览器登录授权、捕获当前 `~/.codex/auth.json`、5h / 周额度与重置倒计时监控、一键切号并安全重启新版 `ChatGPT.app` / `Codex.app`
- **Antigravity**：实时探测本地 `language_server` 获取 Gemini / Claude / GPT 分组额度、macOS Keychain 与 `state.vscdb` 凭证管理、一键登录新账号与切换重启
- **导入导出**：支持导出私有权限 JSON 备份，支持导入原生备份、Codex 扁平数组格式及单账号 `auth.json`
- **菜单栏常驻**：实时显示 `CX xx% · AG xx%`，左键直达主窗口、右键快捷菜单，关闭窗口后后台 60s 静默轮询
- **在线更新**：在主窗口底部点击「检查更新」，应用会查询此仓库的最新正式 GitHub Release。Windows 安装版可在应用内下载、校验 SHA-256 并启动安装程序。Windows 便携版、macOS 和 Linux 会在浏览器中打开对应安装包的下载地址。

## 本地构建
```bash
cargo test
./scripts/bundle-macos.sh --package
```

## 发布多渠道安装包
发布正式版前，先同步更新 `Cargo.toml` 和 `Cargo.lock` 中的应用版本，并使用对应的 `v<版本号>` 标签，确保应用能匹配 Release 中的安装包。

推送 `v*` 标签或在 GitHub Actions 手动触发 `Build & Release`（支持 `stable` / `beta` / `nightly` 渠道），自动产出：
- `macOS Apple Silicon (arm64)` — `.dmg` & `.zip`
- `macOS Intel (x86_64)` — `.dmg` & `.zip`
- `macOS Universal` — `.dmg` & `.zip`
- `Windows x86_64` — NSIS 安装程序与便携 `.zip`
- `Linux x86_64` — `.tar.gz`
- `SHA256SUMS.txt` 与 Homebrew Cask (`ai-switcher.rb`)
