# Wu Plus 开发与维护

Wu Plus 直接在 Wu 源码上开发，当前固定的上游标签和提交记录在 `scripts/wu-upstream.json`。宿主和 SVN 后台在同一仓库维护。macOS App 随包安装同一提交构建的 SVN 后台，宿主通过标准输入输出上的 JSON-RPC 2.0 进行版本握手、请求和取消。后台可以独立编译和测试，直接运行宿主二进制调试时仍可使用开发扩展。

## 本地构建与调试

下面的命令从仓库根目录运行。

~~~bash
rustup show
brew install lld cmake pkgconf subversion
scripts/wu.sh check
scripts/wu.sh build
~~~

Rust 工具链在 `rust-toolchain.toml` 中固定。`check` 检查当前源码并运行 SVN 集成测试，`build` 构建本地宿主二进制。如需包含 SVN 后台的 macOS DMG，运行 `scripts/wu.sh bundle`。本地和 CI 共用 Wu 原生的 `script/bundle-mac`，产物默认写入 `dist/`，可通过 `WU_PLUS_DIST_DIR` 调整。只需重建开发扩展时可直接运行 `scripts/package-plugin.sh`。

用 Wu 打开仓库根目录，Apple Silicon 可选「调试 Wu Plus（Apple Silicon / SVN）」。启动前会构建宿主和扩展、准备 CodeLLDB，并用 `.build/dev-profile` 启动独立实例。调试准备也会将固定版本的 CodeLLDB 放进已存在的官方 Wu 数据目录，供 Wu 1.5.0 联网检查失败时从本地缓存加载。只调试宿主时，跨平台配置「Debug Wu Plus (CodeLLDB)」沿用 Wu 原生的 Cargo 构建与调试器查找方式。 [上游开发文档](https://zed.dev/docs/development)

### 回归验收

~~~bash
cargo test --locked --manifest-path plugin/Cargo.toml
scripts/smoke-workspace.sh
python3 scripts/demo-workspace.py
~~~

SVN 测试使用临时仓库和本机 `svnserve`，覆盖提交范围、更新、冲突、认证和取消。烟雾脚本生成两个 SVN 工作副本和一个 Git 仓库。完整演示工作区包含三个独立 Git 仓库，以及带历史、差异、冲突和 externals 的多个 SVN 工作副本。

验收时用独立配置打开完整演示工作区，核对内置 SVN 自动加载与关闭、多仓库切换、文件 Diff、日志、提交、更新、冲突处理和原生 Git 面板。钥匙串及证书指纹流程已有本地验证，实际内网仓库需要按部署环境验收。

## GitHub Actions

| 工作流 | 用途 | 产物 |
| --- | --- | --- |
| **Wu Plus verify macOS** | 手动运行，快速验收 Apple Silicon 上的宿主与 SVN | 内置 SVN 的临时签名 `Wu Plus Dev.app`，以 ZIP 提供 |
| **Wu Plus candidate** | 手动运行或定时检查后构建正式候选 | macOS Apple Silicon 与 Intel、Linux x86_64 与 ARM64、Windows x86_64 的候选包，macOS 包内置 SVN 后台 |
| **Wu Plus publish** | 手动发布已验收的完整候选 | GitHub Releases 及应用内更新使用的正式资产 |

快速验收只运行 Apple Silicon 的 SVN 集成测试并打包开发版，省去 DMG、dSYM 和远程服务端构建。解压后用下面的命令启动独立配置，即可验收内置 SVN。

~~~bash
open -n "/path/to/Wu Plus Dev.app" --args --user-data-dir "$HOME/.wu-plus-verify"
~~~

Apple Silicon 快速验收和 macOS Apple Silicon、macOS Intel、Windows x86_64 正式候选各有独立的 Rust 编译缓存。手动运行时可用 `rust_cache` 切换缓存，比较同一提交的构建耗时。测热缓存时确认缓存已命中，并单独记录缓存恢复、保存和排队时间。 [快速验收配置](../.github/workflows/wu-plus-verify-macos.yml) · [候选配置](../.github/workflows/wu-plus-candidate.yml)

### 跟进 Wu 上游

1. 定时任务会查找上游正式发布，并在运行摘要提示版本差异。把新标签对应的提交合入集成分支，更新 `scripts/wu-upstream.json` 的标签和提交。
2. 在集成分支运行 `scripts/wu.sh check`，再用快速验收或本地包验证实际工作区。传给脚本或候选工作流的上游版本应与该提交固定的标签一致。
3. 将通过验收的源码合入默认分支，构建完整候选。产品版本取自 `crates/wu/Cargo.toml`。即使上游版本相同，发布 Wu Plus 修复版时也要递增产品版本并同步 `Cargo.lock`。

### macOS 签名

完整候选沿用 Wu 的签名脚本，SVN 后台在 App 签名前装入 `Contents/MacOS`。没有配置签名证书时使用临时签名，配置证书后使用对应身份签名，公证凭据齐全时提交 Apple 公证。Windows 和 Linux 使用各自的 Wu 打包脚本。

| GitHub Actions 机密 | 用途 |
| --- | --- |
| `MACOS_CERTIFICATE`、`MACOS_CERTIFICATE_PASSWORD`、`MACOS_SIGNING_IDENTITY` | 配置签名证书、导出密码和签名标识 |
| `APPLE_NOTARIZATION_KEY`、`APPLE_NOTARIZATION_KEY_ID`、`APPLE_NOTARIZATION_ISSUER_ID`、`APPLE_NOTARIZATION_TEAM` | 提交 Apple 公证 |
| `MACOS_PROVISIONING_PROFILE` | 可选的描述文件 |

## 发布与回退

1. 从候选运行的 **Artifacts** 下载各平台包，用独立配置验收。
2. 在 **Wu Plus publish** 提供候选运行 ID 和候选摘要中的 `SHA256SUMS` 文件摘要。发布工作流从默认分支接受本仓库成功的完整候选，并逐项复核包和发行说明的哈希。
3. 发布后的标签格式为 `wu-plus-v<版本>`。Wu 原生更新器从本项目的 GitHub Releases 下载匹配平台的资产，校验 GitHub 提供的 SHA-256，重启后应用更新。

升级日常配置前保留一份数据目录备份。回退时，退出 Wu Plus，安装带相应 SVN 后台的上一版 App。如果配置或数据库格式已经升级，再恢复升级前的数据备份。 [安装与迁移](INSTALL.md#设置和应用更新) · [发布工作流](../.github/workflows/wu-plus-publish.yml)
