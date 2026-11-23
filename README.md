# Wu Plus

Wu Plus 是基于 Wu 的独立 IDE 分支，提供 macOS、Linux 和 Windows 的构建与原生更新。Subversion 扩展目前面向 macOS 本地工作区，使用系统安装的 SVN 1.14 或更新版本，支持 `file://`、`svn://`、`svn+ssh://`、`http://` 和 `https://`。宿主与扩展进程在同一仓库维护，扩展进程可独立构建和测试。

## 安装与使用

1. 打开 `Wu Plus.app`。应用默认使用独立的 `~/Library/Application Support/Wu Plus` 存放配置、扩展、状态、日志和缓存，可与官方 Wu 并排运行。
2. 解压与 macOS 架构对应的 `Subversion-macos-aarch64.zip` 或 `Subversion-macos-x86_64.zip` 到固定目录。本地构建的扩展文件名为 `Subversion.zip`。在 Wu 扩展页点「安装开发扩展」，选择包含 `extension.toml` 的 `Subversion` 目录。开发扩展通过符号链接加载，安装后请保留该目录。
3. 打开工作区，在活动栏找提示为「SVN 面板」的图标，或在命令面板运行「切换 SVN 面板」。在更改面板底部、历史面板顶部点击工作副本名称切换仓库。
4. 普通编辑器可在线号旁和滚动条显示相对 SVN 基线的变更。点击行边标记可展开差异，行尾 Blame 显示已提交行的作者、修订号和时间。工具栏「查看差异」打开当前工作副本的文本变更汇总。点击变更文件名打开单文件 Diff，可切换左右对比或统一视图、跳转差异块，并直接编辑和保存工作文件。右键提供打开文件、添加、撤销、删除、移动、历史和属性操作。勾选变更后在侧栏底部填写说明并提交。点击提交区左下角的星形按钮，或在提交说明框按 `Option+Tab`，可用 Wu 已配置的智能体和模型根据所选变更生成说明。生成的内容可继续编辑。各工作副本独立保留当前会话中的提交草稿。勾选移动后的文件时会同时选中原路径。未跟踪文件先点行尾 `+`，缺失文件可登记删除。提交新目录中的一个文件时会添加必要的目录节点。
5. 「历史记录」可展开修订并对比具体文件。点击提交区右下角的日志图标，可在编辑区打开完整日志。工具栏的操作按钮提供检出、更新到指定修订、批量更新、远端状态、Blame、分支标签、切换、合并、属性、锁和 Changelist。需要初始化或更新 externals 时，在「更新到指定修订」中勾选「包含 Externals」。点击冲突文件，在原生 Diff 中逐块采用本地、传入或两者的内容，也可直接编辑合并结果。处理完所有冲突块后，点击「标记为已解决」保存结果并清除 SVN 冲突状态。属性冲突可对照并采用本地、传入或基础属性值。更新时双方新增同名文本文件，可对照内容并保留工作副本或采用传入文件。其他树冲突可在 Wu 内置终端使用 SVN 提供的交互选项。
6. 卸载时在扩展页移除 Subversion。下次需要时可再次安装同一目录。

在 Wu 的「设置 → 版本控制 → Subversion（SVN）」中，可以设置变更标记、行内逐行追溯和逐行追溯栏。变更标记和行内追溯默认开启，追溯栏默认关闭。这些偏好保存在用户设置的 `svn` 字段中，打开的 SVN 编辑器会随设置更新。编辑器右上角的追溯开关可临时调整当前标签页。

SVN 默认沿用用户的标准 SVN 配置、认证缓存和 SSH Agent。HTTPS 或 SVN 认证失败后，可以在当前工作副本的「SVN 凭据」里填写账号密码。启用钥匙串保存时，首次成功请求后写入凭据。填写同一用户名且密码留空可以读取已保存的密码。检出表单也可以填写凭据。服务器证书验证失败时，在 Wu 内置终端按 SVN 提示核对指纹，确认信任后重新发起原操作。

## 构建与验证

~~~bash
rustup toolchain install 1.97.1 --profile minimal --component rustfmt,clippy,rust-src
brew install lld cmake pkgconf subversion
scripts/wu.sh bundle
~~~

开发期间可用 `scripts/wu.sh build` 构建本地二进制。生成带原生更新器的 macOS DMG 时运行 `scripts/wu.sh bundle`，本地和 CI 都通过 Wu 的 `script/bundle-mac` 从当前源码打包。此 Fork 以 Wu 官方 `v1.5.0` 为基线，使用运行时 Metal 着色器。构建产物在 `dist/`，可通过 `WU_PLUS_DIST_DIR` 改写。只更新扩展时运行 `scripts/package-plugin.sh`，然后重启 Wu Plus 加载新的后台进程。默认使用本地临时签名。

在 Apple Silicon Mac 上打开仓库根目录，选择「调试 Wu Plus（Apple Silicon / SVN）」。启动前会构建宿主和 SVN 后台，自动注册开发扩展，并用 `.build/dev-profile` 启动调试实例。该配置首次启动时下载并校验 CodeLLDB，后续使用项目内缓存。跨平台的「Debug Wu Plus (CodeLLDB)」沿用 Wu 原生的 Cargo 调试配置和调试器查找方式。

~~~bash
cargo test --locked --manifest-path plugin/Cargo.toml
scripts/smoke-workspace.sh
python3 scripts/demo-workspace.py
~~~

集成测试用临时 SVN 仓库和本机 `svnserve` 检查变更、提交、认证和取消。烟雾工作区脚本会打印一个包含两个 SVN 工作副本和一个 Git 仓库的临时目录，可直接用 Wu 打开。使用 `scripts/demo-workspace.py` 可生成带历史、差异、冲突、externals 和三个独立 Git 仓库的演示工作区。

## 首轮验收

在 Apple Silicon macOS 的 Wu 中，用相同的 Git 和 SVN 文件对照更改、增删统计、差异、提交和历史界面。临时仓库中验证了扩展安装卸载、单文件提交、指定修订和批量更新、分支标签、切换、合并、三类冲突解决及认证重试。另用本地 `svnserve` 验证了钥匙串跨进程读取，用本地自签名 HTTPS 服务核对了证书拒绝和指纹提示。实际内网仓库留待实测。

## 跟进 Wu 新版本

~~~bash
scripts/wu.sh check v1.5.0
scripts/wu.sh bundle v1.5.0
~~~

上游发布新版后，先在集成分支将对应提交合入 Wu Plus，并同步更新 `scripts/wu-upstream.json` 中的上游标签和提交。验收后再触发候选构建。脚本直接检查和构建当前 Git 检出，并运行 Subversion 集成测试。新包交付前，使用独立配置验收。升级日常配置前保留数据目录副本，以便数据格式变化后回退。

插件使用版本化的 JSON-RPC 2.0 标准输入输出接口。宿主从 Wu 的开发扩展列表加载 Subversion 后台，通过 `initialize` 校验协议版本，通过 `discover`、`status`、`read`、`write` 调用业务操作，通过 `$/cancelRequest` 取消。请用同一提交的宿主和扩展包。Zed 的宿主适配推迟到 Wu 验收后。

Wu 基于 Zed 开发。感谢 Wu 和 Zed 团队维护和开放源码。原生编辑器功能参见 [Wu 文档](https://wu.dstrml.org)，宿主开发参见 [Zed 开发文档](https://zed.dev/docs/development)。许可条款为 GPL-3.0-or-later，标注的组件使用 Apache-2.0。 [Wu 上游源码](https://github.com/dstrml/wu) · [GPL](LICENSE-GPL) · [Apache License](LICENSE-APACHE)

## 候选构建与应用内更新

GitHub Actions 的 **Wu Plus candidate** 可以手动触发，也会定时检查 Wu 上游版本。工作流使用仓库中的 Wu 打包脚本构建 macOS（Apple Silicon、Intel）、Linux（x86_64、ARM64）和 Windows（x86_64）的候选。macOS 使用 DMG，Linux 使用 `.tar.gz`，Windows 使用 Inno Setup 安装程序。

macOS 的签名条件沿用 Wu。GitHub Actions 根据以下机密选择打包方式。

| 机密 | 用途 |
| --- | --- |
| `MACOS_CERTIFICATE`、`MACOS_CERTIFICATE_PASSWORD`、`MACOS_SIGNING_IDENTITY` | 配置签名证书、导出密码和签名标识 |
| `APPLE_NOTARIZATION_KEY`、`APPLE_NOTARIZATION_KEY_ID`、`APPLE_NOTARIZATION_ISSUER_ID`、`APPLE_NOTARIZATION_TEAM` | 在签名后提交 Apple 公证 |
| `MACOS_PROVISIONING_PROFILE` | 可选的描述文件 |

缺少签名证书时，macOS 包使用临时签名。配置证书但缺少公证凭据时，只签名 App。Windows 和 Linux 沿用 Wu 现有打包方式。应用内更新使用 Wu 原生更新器，从本项目的 GitHub Releases 下载资产，并验证 GitHub 提供的 SHA-256。

产品版本取自 `crates/wu/Cargo.toml`。同一上游版本也可以发 Wu Plus 修复版，发布时递增产品版本，并同步 `Cargo.lock`。正式标签使用 `wu-plus-v<版本>`，以便和同步进来的上游标签共存。

在候选运行的 **Artifacts** 下载各平台安装包。用独立配置验收后，在 **Wu Plus publish** 提供候选运行 ID 和候选摘要显示的 `SHA256SUMS` 文件摘要。发布任务逐一复核文件哈希，再把同一组文件发布至 Releases。正式版通过「检查更新」更新，自动检查遵循 `auto_update` 设置，版本生效需要重启。

### 首次安装与回退

macOS 可挂载 DMG 并将 `Wu Plus.app` 拖入 Applications，Linux 可用 `script/install.sh` 安装，Windows 可运行 x86_64 安装程序。需要在本机生成临时签名的 macOS 安装包时，运行 `scripts/wu.sh bundle`。

在 macOS 上首次迁移设置前，退出 Wu 和 Wu Plus，然后运行 `scripts/migrate-profile.sh`。它会复制官方 Wu 的设置和扩展到 `~/Library/Application Support/Wu Plus`。应用升级沿用现有资料目录。需要回退时，先退出 Wu Plus，使用上一版安装包，必要时恢复升级前的资料备份。
