# Wu Plus 安装与使用

正式包发布后可在 Wu Plus 的 GitHub Releases 下载。首发验收期间，从 **Wu Plus verify** 或 **Wu Plus candidate** 运行详情的 **Artifacts** 下载匹配平台和架构的包。下载 `Wu Plus Dev` 验收包后，可用独立配置启动。 [Wu Plus Releases](https://github.com/chenqinru/wu-plus/releases) · [验收包启动方式](MAINTAINING.md#github-actions)

## 安装 IDE

### macOS

Apple Silicon 下载 `Wu-Plus-aarch64.dmg`，Intel 下载 `Wu-Plus-x86_64.dmg`。打开 DMG，将 `Wu Plus.app` 拖入「应用程序」。

从终端打开项目时，在 Wu Plus 菜单选择「安装命令行工具」。安装完成后可运行 `wu-plus <项目路径>`。

### Linux

下载对应架构的 `wu-plus-linux-x86_64.tar.gz` 或 `wu-plus-linux-aarch64.tar.gz`。在本仓库根目录运行下面的命令，将包路径换成实际文件。脚本会安装到 `~/.local`，并配置命令行入口和应用菜单。

~~~bash
WU_BUNDLE_PATH="/path/to/wu-plus-linux-x86_64.tar.gz" sh script/install.sh
~~~

正式版本发布后，也可在仓库根目录直接运行 `sh script/install.sh` 下载并安装。通过该脚本安装的 Wu Plus 可用 `sh script/uninstall.sh` 卸载。

### Windows

下载并运行 `Wu-Plus-x86_64.exe`。如果安装时选择将命令行目录加入 PATH，重新打开终端后可使用 `wu-plus`。通过系统的已安装应用列表卸载。

## 启用 Subversion

Wu Plus 在 macOS、Windows 和 Linux 的本地工作区自动检测 SVN 工作副本，按需启动随 IDE 安装的后台。后台调用系统的 SVN 1.14 或更新版本，可访问 `file://`、`svn://`、`svn+ssh://`、`http://` 和 `https://` 仓库。

- macOS 使用 Homebrew 时运行 `brew install subversion`。
- Windows 安装 SlikSVN 等命令行客户端，确认 `svn.exe` 在 PATH 中。使用中文文件名时，客户端的系统代码页需要能表示这些字符。IDE 可以在英文系统上提交中文说明。
- Linux 使用发行版的包管理器安装 `subversion`，例如 Debian 和 Ubuntu 上的 `sudo apt install subversion`。

如果 SVN 面板提示缺少兼容版本，安装后点「重试」重新检测。Windows 上若刚修改 PATH，可重启 Wu Plus 以加载新的环境变量。

打开包含 SVN 工作副本的项目，点活动栏的 SVN 图标，也可在命令面板运行「切换 SVN 面板」。用更改面板底部或历史面板顶部的工作副本名称切换仓库。在「扩展 → 已安装」找到 **Subversion**，点击「设置」可直接跳到「启用 Subversion」。也可以从「设置 → 版本控制 → Subversion（SVN）」进入。

## 使用 SVN

### 差异、提交和历史

- 在「更改」中点文件查看单文件 Diff，可切换左右对比或统一视图，跳到差异块并直接编辑工作文件。点「查看差异」打开当前工作副本的差异汇总。
- 勾选要提交的变更并填写说明。也可点击提交框左下角的星形按钮，让已配置的智能体根据所选变更生成说明。提交框内的默认快捷键为 macOS 上的 `Option+Tab`，以及 Windows 和 Linux 上的 `Alt+L`。每个工作副本有独立的会话草稿。勾选移动后的文件时，原路径会一并选中。
- 未跟踪文件先点行尾 `+`。缺失文件可登记删除。提交新目录中的文件时会加入必要的目录节点。
- 「历史记录」可以展开修订并查看其中的文件 Diff。面板右下角的日志图标会在编辑区打开完整日志。

### 更新和解决冲突

工具栏提供检出、指定修订更新、批量更新、远端状态、Blame、分支与标签、切换、合并、属性、锁和 Changelist。需要更新 externals 时，在「更新到指定修订」中勾选「包含 Externals」。

文本冲突可以在 Diff 中逐块采用本地、传入或双方内容，也可以直接编辑合并结果。处理完后点「标记为已解决」。属性冲突可对照本地、传入和基础值。双方新增同名文本的树冲突可对照并选择保留版本，其他树冲突可用内置终端里的 SVN 交互选项处理。

### 变更标记和认证

在「设置 → 版本控制 → Subversion（SVN）」中可以调整编辑器变更标记、行内 Blame 和 Blame 栏。变更标记与行内 Blame 默认启用。编辑器右上角的开关可临时调整当前标签页的 Blame 显示。

SVN 沿用用户的 SVN 配置、认证缓存和 SSH Agent。可在「SVN 凭据」或检出表单中输入当前会话使用的账号密码。macOS 还可以选择保存到钥匙串，下次输入同一账号后留空密码即可读取。Windows 和 Linux 的跨会话认证由系统 SVN 客户端管理。证书验证失败时，在内置终端按 SVN 提示核对服务器指纹，确认信任后重试。

## 设置和应用更新

macOS 上，Wu Plus 默认将配置、扩展、状态、日志和缓存存放在 `~/Library/Application Support/Wu Plus`。从官方 Wu 首次迁移时，先退出两个应用，再从仓库根目录运行 `scripts/migrate-profile.sh`。脚本会将已有设置和扩展复制到尚未创建的 Wu Plus 数据目录。迁移应在首次启动 Wu Plus 前完成。

从三段版本 `1.5.0` 升级到四段版本 `1.5.0.1` 时，请下载匹配平台的安装包并覆盖安装一次。IDE 的配置和工作区数据存放在独立的数据目录。

从 `1.5.0.1` 开始，正式版通过应用内「检查更新」获取本项目 Releases 的版本。自动检查遵循 `auto_update` 设置，更新需要重启，SVN 后台随 App 一起更新。 [更新与回退](MAINTAINING.md#发布与回退)
