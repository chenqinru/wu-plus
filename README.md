# Wu Plus

Wu Plus 是基于 Wu 的原生编辑器，在 macOS、Windows 和 Linux 的本地工作区支持 Git 和 Subversion。你可以在同一个窗口打开多个 SVN 工作副本，查看差异和日志、提交、更新及处理冲突。

## 安装与使用

Wu Plus 目前处于首发验收阶段。想提前体验，可在 **Wu Plus verify** 或 **Wu Plus candidate** 的运行详情中，从 **Artifacts** 下载对应平台和架构的包。正式包发布后可在 GitHub Releases 下载。 [全平台验收](https://github.com/chenqinru/wu-plus/actions/workflows/wu-plus-verify.yml) · [正式下载](https://github.com/chenqinru/wu-plus/releases)

| 平台 | 开发验收包 | 发布候选包 |
| --- | --- | --- |
| macOS Apple Silicon / Intel | 内含 `Wu Plus Dev.app` 的 ZIP | DMG |
| Linux x86_64 / ARM64 | `wu-plus-dev-linux-<架构>.tar.gz` | `wu-plus-linux-<架构>.tar.gz` |
| Windows x86_64 | `Wu-Plus-Dev-x86_64.exe` | `Wu-Plus-x86_64.exe` |

1. 选用与机器架构匹配的 Wu Plus 包，并确认系统可运行 SVN 1.14 或更新版本。验收包建议使用独立的数据目录启动。 [安装与 SVN 使用说明](docs/INSTALL.md) · [验收包启动方式](docs/MAINTAINING.md#github-actions)
2. 打开包含 SVN 工作副本的项目，点活动栏的 SVN 图标。Wu Plus 会按需启动随包安装的后台，可以在面板中切换多个工作副本。

## SVN 工作流

- **工作副本**　可同时管理多个仓库、同仓库的多份检出、嵌套工作副本和 externals。
- **更改与提交**　支持文件和工作副本 Diff、选中文件提交、批量更新。可用已配置的智能体为所选变更生成提交说明。
- **历史与追溯**　支持日志、修订详情、历史文件 Diff 和 Blame。
- **仓库操作**　支持检出、分支与标签、切换、合并预览与执行、属性、锁、Changelist，以及文本、属性和树冲突处理。

## 开发与发布

在 GitHub Actions 手动运行 **Wu Plus verify**，`platform` 默认选 `all`，也可只验收 `macos`、`linux` 或 `windows`。各任务运行真实 SVN 集成测试，并验证随包分发的 SVN 后台能启动、握手和取消操作。准备发布时，运行 **Wu Plus candidate** 生成完整候选，再用 **Wu Plus publish** 发布通过验收的候选。 [维护手册](docs/MAINTAINING.md#github-actions)

Wu Plus 基于 Wu，Wu 基于 Zed。项目采用 GPL-3.0-or-later，标注的组件采用 Apache-2.0。 [Wu 上游](https://github.com/dstrml/wu) · [Zed 上游](https://github.com/zed-industries/zed) · [GPL](LICENSE-GPL) · [Apache-2.0](LICENSE-APACHE)
