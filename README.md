# Wu Plus

Wu Plus 是基于 Wu 的独立 IDE 分支，首版面向 Apple Silicon macOS 的本地工作区，并提供 Subversion 扩展。使用系统安装的 SVN 1.14 或更新版本，支持 `file://`、`svn://`、`svn+ssh://`、`http://` 和 `https://`。宿主与扩展进程在同一仓库维护，扩展进程可独立构建和测试。

## 安装与使用

1. 打开 `Wu Plus.app`。应用默认使用独立的 `~/Library/Application Support/Wu Plus` 存放配置、扩展、状态、日志和缓存，可与官方 Wu 并排运行。
2. 解压 `Subversion.zip` 到固定目录。在 Wu 扩展页点「安装开发扩展」，选择包含 `extension.toml` 的 `Subversion` 目录。开发扩展通过符号链接加载，安装后请保留该目录。
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

验收期间可用 `scripts/wu.sh build` 只构建本地二进制。需要生成安装包时再运行 `scripts/wu.sh bundle`。此 Fork 以 Wu 官方 `v1.5.0` 为基线。构建直接使用当前检出的宿主源码，采用 Apple Silicon 的运行时 Metal 着色器路径。打包使用匹配版本的官方应用包提供上游资源与 Git 辅助程序，通过 `WU_PLUS_APP_TEMPLATE` 可指定该应用的位置。构建产物在 `dist/`，可通过 `WU_PLUS_DIST_DIR` 改写。只更新扩展时运行 `scripts/package-plugin.sh`，然后重启 Wu Plus 加载新的后台进程。应用和扩展使用本地临时签名。

在 Wu 编辑器中打开仓库根目录，在调试视图选择「调试 Wu Plus」。启动前会构建宿主和 SVN 后台，自动注册开发扩展，并用 `.build/dev-profile` 启动调试实例。首次调试会下载并校验 CodeLLDB，后续使用项目内缓存，可以在宿主 Rust 源码中设置断点。

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

上游发布新版后，先在集成分支将对应提交合入 Wu Plus，并同步更新 `scripts/wu-upstream.json` 中的上游标签、提交和官方 DMG 摘要。验收后再触发候选构建。脚本直接检查和构建当前 Git 检出，并运行 Subversion 集成测试。新包交付前，使用独立配置验收。升级日常配置前保留数据目录副本，以便数据格式变化后回退。

插件使用版本化的 JSON-RPC 2.0 标准输入输出接口。宿主通过 `initialize` 协商版本，通过 `discover`、`status`、`read`、`write` 调用业务操作，通过 `$/cancelRequest` 取消。Zed 的宿主适配推迟到 Wu 验收后。

许可条款见 [GPL](LICENSE-GPL) 和 [Apache License](LICENSE-APACHE)。Wu 上游源码和历史见 [dstrml/wu](https://github.com/dstrml/wu)。

## 候选构建与正式更新

GitHub Actions 的 **Wu Plus candidate** 每天检查上游正式发布。也可手动重复构建当前提交。发现上游新版时，工作流报告版本差异。候选仍使用当前提交固定的上游版本，检查其提交关系和官方 Apple Silicon DMG 的 SHA-256，再编译、运行 SVN 集成测试并打包。失败时查看同一运行的日志附件。候选版本号使用「运行编号.重试编号」，同一上游版本也可以通过 Sparkle 升级。

候选附件包含 `Wu-Plus-arm64.zip`、`Subversion.zip` 和 `candidate.json`。签名候选还包含 `appcast.xml`。下载并核对 `candidate.json` 的安装包 SHA-256，然后使用独立配置打开多 Git、多 SVN 演示工作区。在打算沿用的插件版本上检查扩展加载、仓库切换、Diff、日志、提交、更新和原生 Git 面板。提交与更新使用演示副本。

~~~bash
python3 scripts/demo-workspace.py
open -a "/path/to/Wu Plus.app" --args --user-data-dir "/path/to/review-profile" "/path/to/demo/workspace"
~~~

配置下面的 GitHub Actions 变量和机密后，设 `WU_PLUS_SIGN_CANDIDATES` 为 `1`。默认构建本地签名候选，供验收使用。

| 类型 | 配置项 | 用途 |
| --- | --- | --- |
| 变量 | `WU_PLUS_CODESIGN_IDENTITY` | `Developer ID Application: …` |
| 变量 | `WU_PLUS_SPARKLE_PUBLIC_KEY` | Sparkle Ed25519 公钥，写入应用包 |
| 机密 | `APPLE_CERTIFICATE_P12`、`APPLE_CERTIFICATE_PASSWORD` | Base64 编码的 Developer ID P12 和导出密码 |
| 机密 | `APPLE_NOTARY_KEY`、`APPLE_NOTARY_KEY_ID`、`APPLE_NOTARY_ISSUER` | Base64 编码的公证 API 密钥及对应的 Key ID、Issuer ID |
| 机密 | `WU_PLUS_SPARKLE_PRIVATE_KEY` | Sparkle 导出的原始私钥文本，用于签署更新 ZIP 和 appcast |
| 可选变量 | `WU_PLUS_RELEASE_REPOSITORY` | 单独承载公开更新的 `owner/repo`。默认使用构建仓库 |
| 可选机密 | `WU_PLUS_PUBLISH_TOKEN` | 使用单独的公开更新仓库时，提供仅对该仓库有 Contents 写权限的令牌 |

公证成功后才会生成正式渠道的候选。验收通过后，手动运行 **Wu Plus publish**，提供候选运行 ID 和已验收安装包的原始 SHA-256。工作流会复核候选的来源、签名、公证和更新元数据，将同一份 ZIP 及其 appcast 上传至草稿，再切换正式更新源。可以在 GitHub 的 `wu-plus-release` 环境设置额外审核。更新源使用公开仓库的 `releases/latest/download/appcast.xml`，构建仓库可以是私有仓库。应在首次正式发包前选定更新仓库，已经安装的应用会继续使用构建时写入的地址。

### 迁移与回退

首次正式启用前，退出 Wu 和 Wu Plus，然后运行 `scripts/migrate-profile.sh`。它会复制 Wu 的用户设置和已安装扩展到 `~/Library/Application Support/Wu Plus`，保留开发扩展链接。目标已存在时脚本会停下来保留现有配置。SVN 扩展通过「安装开发扩展」安装到固定位置一次。应用升级沿用同一个应用标识、数据目录和扩展位置。macOS 可能会对新应用标识的受保护文件夹访问单独询问一次。

Wu Plus 的应用标识为 `io.github.chenqinru.wu-plus`，终端命令为 `wu-plus`，链接方案为 `wu-plus://`。

正式包通过「检查更新」进入 Sparkle，使用 Wu 的 `auto_update` 设置控制自动检查。Sparkle 2.10.1 需要 macOS 12 或更新版本。正式 Releases 保留旧版安装包，回退时先退出 Wu Plus，再换回上一版应用。涉及数据迁移时，应在升级前退出应用，用 `ditto` 备份整个 `~/Library/Application Support/Wu Plus`，并在需要回退数据结构时恢复相应备份。

本地演练可设置 `WU_PLUS_UPDATE_TEST=1`，使用 `http://localhost` 的签名 appcast 和两份递增构建号的应用包。测试应用使用专用标识与 `~/Library/Application Support/Wu Plus Update Test`，Sparkle 重启后也会继续使用该目录。正式更新包要求 HTTPS、Developer ID、公证和 Sparkle 签名。
