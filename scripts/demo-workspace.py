#!/usr/bin/env python3
"""Create a disposable, offline workspace with real SVN and Git repositories."""

from pathlib import Path
import subprocess
import sys
import tempfile


def run(*args, cwd=None):
    subprocess.run(args, check=True, cwd=cwd, stdout=sys.stderr)


def svn(worktree, *args):
    run("svn", *args, cwd=worktree)


def write(worktree, path, text):
    file = worktree / path
    file.parent.mkdir(parents=True, exist_ok=True)
    file.write_text(text, encoding="utf-8")


def replace(worktree, path, before, after):
    file = worktree / path
    text = file.read_text(encoding="utf-8")
    if before not in text:
        raise ValueError(f"{path}: {before!r} missing from test fixture")
    file.write_text(text.replace(before, after, 1), encoding="utf-8")


def commit(worktree, author, message):
    svn(worktree, "commit", "-q", "--username", author, "-m", message)


def repository(root, name):
    location = root / "repositories" / name
    run("svnadmin", "create", location)
    url = location.as_uri()
    svn(
        root, "mkdir", "-q", f"{url}/trunk", f"{url}/branches", f"{url}/tags",
        "--username", "build.team", "-m", "初始化 trunk、branches 和 tags",
    )
    seed = root / "upstream" / name
    svn(root, "checkout", "-q", f"{url}/trunk", seed)
    return url, seed


def checkout(root, url, name, branch="trunk"):
    dest = root / "workspace" / name
    svn(root, "checkout", "-q", f"{url}/{branch}", dest)
    return dest


def version(root, url, dest, author, message):
    svn(root, "copy", "-q", f"{url}/trunk", f"{url}/{dest}", "--username", author, "-m", message)


def api(root):
    url, seed = repository(root, "portal-api")
    write(seed, "README.md", "# Portal API\n\n演示报名、审核和名单导出的服务端仓库。\n")
    write(seed, "service/enrollment.py", """from datetime import datetime

CAPACITY = 30
CUTOFF_HOURS = 24


def can_register(event, current_time: datetime):
    if event.archived:
        return False
    if current_time >= event.starts_at:
        return False
    if event.confirmed_count >= CAPACITY:
        return False
    return True


def validate_student(student):
    if student is None:
        return "student_not_found"
    if student.status != "active":
        return "student_not_active"
    return None


def build_receipt(student, event):
    return {
        "student_id": student.id,
        "event_id": event.id,
        "campus": event.campus,
        "status": "pending",
    }


def export_row(receipt):
    return (
        receipt["student_id"],
        receipt["event_id"],
        receipt["campus"],
        receipt["status"],
    )


def display_status(receipt):
    if receipt["status"] == "pending":
        return "Waiting for review"
    return "Confirmed"
""")
    write(seed, "service/approval.py", 'def approval_mode():\n    return "manual"\n')
    write(seed, "service/campus.py", 'DEFAULT_CAMPUS = "north"\n')
    write(seed, "config/permissions.json", '{"role": "reviewer", "scope": "campus"}\n')
    write(seed, "config/application.yml", "database:\n  pool_size: 8\nreporting:\n  enabled: false\n")
    write(seed, "sql/schema.sql", "CREATE TABLE enrollment (id BIGINT PRIMARY KEY, status VARCHAR(32));\n")
    write(seed, "docs/export-draft.md", "# 导出草稿\n\n使用 UTF-8 编码。\n")
    write(seed, "docs/obsolete-endpoint.md", "# 旧接口\n\nGET /legacy/enrollment\n")
    write(seed, "docs/checklist.md", "# 检查清单\n\n- 核对参与名单\n")
    write(seed, "tests/test_enrollment.py", 'EXPECTED = ["pending", "confirmed"]\n')
    svn(seed, "add", "-q", "README.md", "service", "config", "sql", "docs", "tests")
    svn(seed, "propset", "-q", "svn:ignore", "cache.tmp", ".")
    svn(seed, "propset", "-q", "review-profile", "campus-only", "config/permissions.json")
    commit(seed, "li.ming", "建立报名、审核及导出服务骨架")

    revisions = [
        ("sun.yan", "提高连接池容量以支持批量审核", "config/application.yml", "pool_size: 8", "pool_size: 12"),
        ("li.ming", "将报名容量调到四十人", "service/enrollment.py", "CAPACITY = 30", "CAPACITY = 40"),
        ("zhao.qi", "在数据表记录报名校区", "sql/schema.sql", "status VARCHAR(32)", "status VARCHAR(32), campus VARCHAR(32)"),
        ("sun.yan", "开放基础统计报表", "config/application.yml", "enabled: false", "enabled: true"),
        ("wang.lin", "补充审核状态的回归用例", "tests/test_enrollment.py", '"confirmed"]', '"confirmed", "rejected"]'),
        ("li.ming", "明确待审核状态的提示文案", "service/enrollment.py", "Waiting for review", "Review pending"),
        ("zhao.qi", "在验收清单增加报名截止校验", "docs/checklist.md", "- 核对参与名单", "- 核对参与名单\n- 核对报名截止时间"),
        ("wang.lin", "约定导出列顺序", "docs/export-draft.md", "使用 UTF-8 编码。", "使用 UTF-8 编码。列顺序为学号、活动、校区、状态。"),
        ("sun.yan", "将默认校区切换为主校区", "service/campus.py", '"north"', '"main"'),
        ("li.ming", "为权限配置增加只读角色", "config/permissions.json", '"scope": "campus"', '"scope": "campus", "observer": true'),
    ]
    for author, message, path, before, after in revisions:
        replace(seed, path, before, after)
        commit(seed, author, message)

    version(root, url, "branches/release-2026", "release.team", "创建 2026 发布分支")
    version(root, url, "branches/reporting", "release.team", "创建报表改进分支")
    version(root, url, "tags/v1.0.0", "release.team", "标记 v1.0.0")
    wc = checkout(root, url, "portal-api")
    release = checkout(root, url, "portal-api-release", "branches/release-2026")

    # Incoming edits and local edits are committed on opposite sides of one update.
    replace(seed, "service/approval.py", '"manual"', '"rules_engine"')
    svn(seed, "propset", "-q", "review-profile", "central-review", "config/permissions.json")
    write(seed, "docs/rollout-plan.md", "# 发布方案\n\n服务端先灰度到主校区。\n")
    svn(seed, "add", "-q", "docs/rollout-plan.md")
    commit(seed, "zhao.qi", "接入规则审核并发布灰度方案")

    replace(wc, "service/approval.py", '"manual"', '"delegate"')
    svn(wc, "propset", "-q", "review-profile", "campus-delegate", "config/permissions.json")
    write(wc, "docs/rollout-plan.md", "# 发布方案\n\n先安排两个试点活动，再逐校区开放。\n")
    svn(wc, "add", "-q", "docs/rollout-plan.md")
    svn(wc, "update", "-q", "--accept", "postpone")

    replace(wc, "service/enrollment.py", "CAPACITY = 40", "CAPACITY = 60")
    replace(
        wc, "service/enrollment.py",
        '    return "Confirmed"\n',
        '    if receipt["status"] == "rejected":\n        return "Changes requested"\n    return "Confirmed"\n',
    )
    replace(wc, "config/application.yml", "pool_size: 12", "pool_size: 16")
    replace(wc, "sql/schema.sql", "campus VARCHAR(32)", "campus VARCHAR(32), apply_time TIMESTAMP")
    replace(wc, "tests/test_enrollment.py", '"rejected"]', '"rejected", "cancelled"]')
    svn(wc, "changelist", "-q", "registration", "service/enrollment.py", "tests/test_enrollment.py")
    svn(wc, "move", "-q", "docs/export-draft.md", "docs/export-guide.md")
    svn(wc, "delete", "-q", "docs/obsolete-endpoint.md")
    (wc / "docs/checklist.md").unlink()
    svn(wc, "propset", "-q", "svn:eol-style", "LF", "README.md")
    svn(wc, "lock", "-q", "-m", "演示校区配置文件锁", "service/campus.py")
    write(wc, "sql/migrations/2026_11_add_receipt.sql", "ALTER TABLE enrollment ADD receipt_id BIGINT;\n")
    write(wc, "sql/migrations/2026_12_add_audit.sql", "ALTER TABLE enrollment ADD audited_at TIMESTAMP;\n")
    svn(wc, "add", "-q", "sql/migrations")
    write(wc, "docs/验收说明 @待补.md", "# 本地未跟踪说明\n\n整理报名、审核和导出的验收用例。\n")
    write(wc, "cache.tmp", "ignored fixture\n")

    write(release, "docs/release-2026.md", "# 2026 发布说明\n\n- 审核接口兼容旧客户端\n")
    svn(release, "add", "-q", "docs/release-2026.md")
    replace(release, "config/application.yml", "pool_size: 12", "pool_size: 14")
    return wc


def web(root):
    url, seed = repository(root, "portal-web")
    write(seed, "README.md", "# Portal Web\n\n学生报名和管理员审核界面。\n")
    write(seed, "src/enrollment.ts", 'export const defaultFilter = "all";\nexport const pageSize = 20;\n')
    write(seed, "src/legacy.ts", 'export const legacyRoute = "/legacy";\n')
    write(seed, "src/theme.css", ":root {\n  --surface: #f8fafc;\n  --accent: #2563eb;\n}\n")
    write(seed, "src/messages.json", '{"pending": "待审核", "confirmed": "已确认"}\n')
    write(seed, "tests/filter.test.ts", 'export const expectedFilters = ["all", "pending"];\n')
    svn(seed, "add", "-q", "README.md", "src", "tests")
    commit(seed, "he.jia", "建立报名列表与审核页面")

    for author, message, path, before, after in [
        ("he.jia", "分页改为每页三十条", "src/enrollment.ts", "pageSize = 20", "pageSize = 30"),
        ("liu.ran", "补充审核退回文案", "src/messages.json", '"已确认"}', '"已确认", "rejected": "已退回"}'),
        ("he.jia", "统一列表背景色", "src/theme.css", "#f8fafc", "#f9fafb"),
        ("liu.ran", "补充筛选回归用例", "tests/filter.test.ts", '"pending"]', '"pending", "rejected"]'),
        ("he.jia", "更新新版审核操作说明", "README.md", "管理员审核界面。", "管理员审核界面。支持按状态筛选。"),
    ]:
        replace(seed, path, before, after)
        commit(seed, author, message)

    version(root, url, "branches/review-redesign", "release.team", "建立审核界面改造分支")
    version(root, url, "tags/v0.9.0", "release.team", "标记 v0.9.0")
    wc = checkout(root, url, "portal-web")
    replace(wc, "src/enrollment.ts", '"all"', '"pending"')
    replace(wc, "src/theme.css", "#2563eb", "#1d4ed8")
    replace(wc, "src/messages.json", '"已退回"', '"请修改后重试"')
    svn(wc, "delete", "-q", "src/legacy.ts")
    write(wc, "src/components/ReviewFilter.ts", 'export const reviewFilter = ["pending", "confirmed", "rejected"];\n')
    svn(wc, "add", "-q", "src/components")
    write(wc, "docs/待定交互.md", "# 待定交互\n\n确认是否保留全部状态选项。\n")


def operations(root):
    shared_url, shared = repository(root, "shared-config")
    write(shared, "logging.yml", "level: info\nformat: json\n")
    svn(shared, "add", "-q", "logging.yml")
    commit(shared, "ops.team", "建立共享日志配置")

    url, seed = repository(root, "operations")
    write(seed, "nginx/site.conf", "location /api/ {\n  proxy_read_timeout 30s;\n}\n")
    write(seed, "deploy/portal.yml", "replicas: 2\nimage: portal:v1.0.0\n")
    write(seed, "README.md", "# Operations\n\n包含部署、反向代理和独立维护的共享配置。\n")
    svn(seed, "add", "-q", "nginx", "deploy", "README.md")
    svn(seed, "propset", "-q", "svn:externals", f"{shared_url}/trunk shared-config", ".")
    commit(seed, "ops.team", "添加部署清单和共享配置 external")
    replace(seed, "nginx/site.conf", "30s", "45s")
    commit(seed, "sun.yan", "增加反向代理读取超时")
    wc = checkout(root, url, "operations")
    replace(wc, "nginx/site.conf", "45s", "60s")
    replace(wc, "deploy/portal.yml", "replicas: 2", "replicas: 3")
    replace(wc, "shared-config/logging.yml", "level: info", "level: debug")


def git_commit(worktree, message):
    run(
        "git", "-c", "user.name=Demo", "-c", "user.email=demo@example.invalid",
        "commit", "-qm", message, cwd=worktree,
    )


def git_seed(workspace, name, files):
    worktree = workspace / name
    worktree.mkdir()
    run("git", "init", "-q", "-b", "main", cwd=worktree)
    for path, content in files.items():
        write(worktree, path, content)
    run("git", "add", ".", cwd=worktree)
    git_commit(worktree, "初始化演示工作副本")
    readme = worktree / "README.md"
    with readme.open("a", encoding="utf-8") as file:
        file.write("\n包含常用操作和回滚方法。\n")
    run("git", "add", "README.md", cwd=worktree)
    git_commit(worktree, "补充操作和回滚说明")
    return worktree


def multi_git_fixtures(workspace):
    web = git_seed(workspace, "git-web", {
        "README.md": "# 审核页面\n\n初版操作说明。\n",
        "src/review.ts": "export const PAGE_SIZE = 20;\n",
        "tests/review.test.ts": "// Initial smoke test\n",
    })
    run("git", "switch", "-qc", "feature/review-filter", cwd=web)
    replace(web, "src/review.ts", "20", "40")
    run("git", "add", "src/review.ts", cwd=web)
    write(web, "tests/review.test.ts", "// Add a filtering regression case\n")
    write(web, "review-checklist.md", "# 筛选验收清单\n")

    ops = git_seed(workspace, "git-ops", {
        "README.md": "# 运行手册\n\n部署环境说明。\n",
        "nginx/site.conf": "location /api/ {\n    proxy_read_timeout 30s;\n}\n",
        "deploy/portal.yml": "replicas: 2\n",
        "docs/old-runbook.md": "# 旧版运维说明\n",
    })
    run("git", "switch", "-qc", "release/2026", cwd=ops)
    replace(ops, "deploy/portal.yml", "replicas: 2", "replicas: 3")
    replace(ops, "nginx/site.conf", "30s", "45s")
    run("git", "rm", "-q", "docs/old-runbook.md", cwd=ops)


def comparison_fixtures(root):
    original = "# 工作说明\n\n两份工作副本使用相同的起始内容。\n"
    git_copy = root / "workspace" / "git-notes"
    git_copy.mkdir()
    run("git", "init", "-q", "-b", "main", cwd=git_copy)
    write(git_copy, "README.md", original)
    run("git", "add", "README.md", cwd=git_copy)
    git_commit(git_copy, "初始化工作说明")

    url, seed = repository(root, "review-svn")
    write(seed, "README.md", original)
    svn(seed, "add", "-q", "README.md")
    svn(seed, "propset", "-q", "svn:mime-type", "text/plain", "README.md")
    commit(seed, "Demo", "初始化工作说明")
    svn_copy = checkout(root, url, "review-svn")

    for worktree in (git_copy, svn_copy):
        write(worktree, "README.md", original + "\n新增一行用于面板对照。\n")
        write(worktree, "notes.md", "# 工作备忘\n")


DEMO_GUIDE = """# Wu Plus 演示工作区

这是本机生成的离线数据，提交和更新只会写入这套演示仓库。

| 工作副本 | 可以验证 |
| --- | --- |
| `portal-api` | 两处差异、修订历史、文本/属性/树冲突、移动、删除、缺失、文件锁、Changelist |
| `portal-api-release` | 同一仓库的独立发布分支工作副本 |
| `portal-web` | TS、CSS 和 JSON 变更、待添加目录、删除及未跟踪文件 |
| `operations` | 独立部署仓库和目录 external |
| `operations/shared-config` | 独立识别和操作的外部工作副本 |
| `review-svn`、`git-notes` | 用相同的已跟踪和未跟踪文件对照两种面板 |
| `git-web`、`git-ops` | 与 `git-notes` 一起验证 Git 多仓库切换、不同分支、暂存和未暂存状态 |

切到 `portal-api` 后打开侧栏「历史记录」，展开修订可查看变更文件和对应 Diff。
在「更改」里点 `service/enrollment.py`，可以对照相隔较远的两处改动。
`service/approval.py`、`config/permissions.json`、`docs/rollout-plan.md` 分别演示三类冲突。
"""

def main():
    default = Path(tempfile.gettempdir()) / "wu-plus-fixtures"
    base = Path(sys.argv[1]).expanduser().resolve() if len(sys.argv) > 1 else default
    base.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="wu-plus-demo.", dir=base))
    for directory in ("workspace", "repositories", "upstream"):
        (root / directory).mkdir()

    api(root)
    web(root)
    operations(root)
    comparison_fixtures(root)
    multi_git_fixtures(root / "workspace")
    write(root / "workspace", "验收导览.md", DEMO_GUIDE)
    print(root / "workspace")


if __name__ == "__main__":
    main()
