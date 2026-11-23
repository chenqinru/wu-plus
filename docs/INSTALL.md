# Wu Plus 安装与使用

正式包发布后可在 Wu Plus 的 GitHub Releases 下载。首发验收期间，从对应候选运行的 **Artifacts** 下载，优先选择包含所需平台和架构的产物。 [Wu Plus Releases](https://github.com/chenqinru/wu-plus/releases) · [构建与验收入口](MAINTAINING.md#github-actions)

## 安装 IDE

### macOS

Apple Silicon 下载 `Wu-Plus-aarch64.dmg`，Intel 下载 `Wu-Plus-x86_64.dmg`。打开 DMG，将 `Wu Plus.app` 拖入「应用程序」。如果手上是快速验收产物 `Wu-Plus-Dev-arm64.zip`，解压后按独立配置的方式启动。 [快速验收](MAINTAINING.md#github-actions)

从终端打开项目时，在 Wu Plus 菜单选择「安装命令行工具」。安装完成后可运行 `wu-plus <项目路径>`。

### Linux

下载对应架构的 `wu-plus-linux-x86_64.tar.gz` 或 `wu-plus-linux-aarch64.tar.gz`。在本仓库根目录运行下面的命令，将包路径换成实际文件。脚本会安装到 `~/.local`，并配置命令行入口和应用菜单。

~~~bash
WU_BUNDLE_PATH="/path/to/wu-plus-linux-x86_64.tar.gz" sh script/install.sh
~~~

正式版本发布后，也可在仓库根目录直接运行 `sh script/install.sh` 下载并安装。通过该脚本安装的 Wu Plus 可用 `sh script/uninstall.sh` 卸载。

### Windows

下载并运行 `Wu-Plus-x86_64.exe`。如果安装时选择将命令行目录加入 PATH，重新打开终端后可使用 `wu-plus`。通过系统的已安装应用列表卸载。

## 安装 Subversion 扩展

扩展支持 macOS 本地工作区，要求 SVN 1.14 或更新版本。使用 Homebrew 时运行 `brew install subversion`。可访问 `file://`、`svn://`、`svn+ssh://`、`http://` 和 `https://` 仓库。

1. 下载与宿主来自同一次构建且架构匹配的扩展。完整候选使用 `Subversion-macos-aarch64.zip` 或 `Subversion-macos-x86_64.zip`，快速验收和本地构建使用 `Subversion.zip`。
2. 把扩展解压到固定位置。在 Wu Plus 扩展页选择「安装开发扩展」，选中包含 `extension.toml` 的 `Subversion` 目录。IDE 通过符号链接加载该目录，使用期间请保留它。
3. 打开包含 SVN 工作副本的项目。点活动栏的 SVN 图标，也可在命令面板运行「切换 SVN 面板」。用更改面板底部或历史面板顶部的工作副本名称切换仓库。

在扩展页移除 Subversion 即可卸载。若直接用新文件替换已安装扩展目录里的后台程序，重启 Wu Plus 后会加载新程序。

## 使用 SVN

### 差异、提交和历史

- 在「更改」中点文件查看单文件 Diff，可切换左右对比或统一视图，跳到差异块并直接编辑工作文件。点「查看差异」打开当前工作副本的差异汇总。
- 勾选要提交的变更并填写说明。也可点击提交框左下角的星形按钮，或在提交框按 `Option+Tab`，让已配置的智能体根据所选变更生成说明。每个工作副本有独立的会话草稿。勾选移动后的文件时，原路径会一并选中。
- 未跟踪文件先点行尾 `+`。缺失文件可登记删除。提交新目录中的文件时会加入必要的目录节点。
- 「历史记录」可以展开修订并查看其中的文件 Diff。面板右下角的日志图标会在编辑区打开完整日志。

### 更新和解决冲突

工具栏提供检出、指定修订更新、批量更新、远端状态、Blame、分支与标签、切换、合并、属性、锁和 Changelist。需要更新 externals 时，在「更新到指定修订」中勾选「包含 Externals」。

文本冲突可以在 Diff 中逐块采用本地、传入或双方内容，也可以直接编辑合并结果。处理完后点「标记为已解决」。属性冲突可对照本地、传入和基础值。双方新增同名文本的树冲突可对照并选择保留版本，其他树冲突可用内置终端里的 SVN 交互选项处理。

### 变更标记和认证

在「设置 → 版本控制 → Subversion（SVN）」中可以调整编辑器变更标记、行内 Blame 和 Blame 栏。变更标记与行内 Blame 默认启用。编辑器右上角的开关可临时调整当前标签页的 Blame 显示。

SVN 沿用用户的 SVN 配置、认证缓存和 SSH Agent。认证失败时，可在工作副本的「SVN 凭据」中填写账号密码，并选择保存到 macOS 钥匙串。已保存的账号可以留空密码来读取钥匙串。检出表单也可填写凭据。证书验证失败时，在内置终端按 SVN 提示核对服务器指纹，确认信任后重试。

## 设置和应用更新

macOS 上，Wu Plus 默认将配置、扩展、状态、日志和缓存存放在 `~/Library/Application Support/Wu Plus`。从官方 Wu 首次迁移时，先退出两个应用，再从仓库根目录运行 `scripts/migrate-profile.sh`。脚本会将已有设置和扩展复制到尚未创建的 Wu Plus 数据目录。迁移应在首次启动 Wu Plus 前完成。

正式版通过应用内「检查更新」获取本项目 Releases 的版本，自动检查遵循 `auto_update` 设置，更新需要重启。SVN 扩展应使用与新宿主匹配的构建包。 [更新与回退](MAINTAINING.md#发布与回退)
