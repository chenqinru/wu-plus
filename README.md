# Wu Plus

Wu Plus 是基于 Wu 的原生编辑器，保留 Git 工作流，并在 macOS 本地工作区提供 Subversion 扩展。你可以在同一个窗口打开多个 SVN 工作副本，查看差异和日志、提交、更新及处理冲突。

## 安装与使用

Wu Plus 目前处于首发验收阶段，正式安装包发布后可在 [GitHub Releases](https://github.com/chenqinru/wu-plus/releases) 下载。想提前体验，可从候选构建的 **Artifacts** 下载。三平台候选的构建与验收入口见 [开发与发布](docs/MAINTAINING.md#github-actions)。

| 平台 | IDE 候选包 | SVN 扩展 |
| --- | --- | --- |
| macOS Apple Silicon / Intel | 对应架构的 DMG | 支持本地工作区，需单独安装扩展和 SVN 1.14+ |
| Linux x86_64 / ARM64 | 对应架构的 `.tar.gz` | 暂未提供 |
| Windows x86_64 | `.exe` 安装程序 | 暂未提供 |

在 macOS 上，先用 Homebrew 安装 SVN 1.14 或更新版本，或使用已安装的兼容版本。

~~~bash
brew install subversion
~~~

1. 安装与机器架构匹配的 Wu Plus，下载同一次构建的 Subversion 扩展包。
2. 把扩展解压到准备长期保留的目录。在 Wu Plus 扩展页选择「安装开发扩展」，选择包含 `extension.toml` 的 `Subversion` 目录。
3. 打开包含 SVN 工作副本的项目，点活动栏的 SVN 图标。多个工作副本可以在 SVN 面板切换。

平台安装命令、扩展卸载、配置迁移和 SVN 操作说明见 [安装与 SVN 使用说明](docs/INSTALL.md)。

## SVN 工作流

- **工作副本**　可同时管理多个仓库、同仓库的多份检出、嵌套工作副本和 externals。
- **更改与提交**　支持文件和工作副本 Diff、选中文件提交、批量更新。可用已配置的智能体为所选变更生成提交说明。
- **历史与追溯**　支持日志、修订详情、历史文件 Diff 和 Blame。
- **仓库操作**　支持检出、分支与标签、切换、合并预览与执行、属性、锁、Changelist，以及文本、属性和树冲突处理。

## 开发与发布

日常在 Apple Silicon 上验收 SVN，可运行 **Wu Plus verify macOS**。准备发布时，**Wu Plus candidate** 生成完整三平台候选，**Wu Plus publish** 核对已验收的候选文件并将其发布。调试、构建缓存、上游同步、签名和回退见 [维护手册](docs/MAINTAINING.md)。

Wu Plus 基于 Wu，Wu 基于 Zed。项目采用 GPL-3.0-or-later，标注的组件采用 Apache-2.0。 [Wu 上游](https://github.com/dstrml/wu) · [Zed 上游](https://github.com/zed-industries/zed) · [GPL](LICENSE-GPL) · [Apache-2.0](LICENSE-APACHE)
