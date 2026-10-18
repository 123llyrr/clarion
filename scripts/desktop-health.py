#!/usr/bin/env python3
"""Read-only checks for Clarion launch paths, configuration and the live session.

Never starts/stops services, reloads compositor configuration, or prints config
contents. Exit 1 on an error; --strict also treats warnings as a failure.
"""

import argparse
from dataclasses import asdict, dataclass
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess


MAX_BINARY_BYTES = 512 * 1024 * 1024


@dataclass
class Check:
    name: str
    status: str
    message: str


def run(args, timeout=5):
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
        return result.returncode, result.stdout
    except (OSError, subprocess.TimeoutExpired, UnicodeError):
        return None, ""


def binary_digest(path):
    # Refuse FIFOs/devices even if an executable or /proc link was replaced.
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > MAX_BINARY_BYTES:
            raise OSError("not a bounded regular executable")
        digest = hashlib.sha256()
        total = 0
        while block := stream.read(1024 * 1024):
            total += len(block)
            if total > MAX_BINARY_BYTES:
                raise OSError("executable grew beyond the size limit")
            digest.update(block)
        return digest.hexdigest()


def check_launcher(name, resolved, installed):
    if resolved is None:
        return Check(name, "error", f"PATH 中没有 {name}")
    resolved = Path(resolved)
    installed = Path(installed)
    try:
        if not resolved.is_file() or not os.access(resolved, os.X_OK):
            return Check(name, "error", f"启动入口不可执行：{resolved}")
        if installed.exists() and not os.path.samefile(resolved, installed):
            if binary_digest(resolved) != binary_digest(installed):
                return Check(name, "warn", f"PATH 启动入口与 /usr/local/bin 安装版本不同：{resolved}")
        return Check(name, "ok", f"启动入口：{resolved.resolve()}")
    except OSError:
        return Check(name, "warn", f"无法核对启动入口：{resolved}")


def parse_service(text):
    values = dict(line.split("=", 1) for line in text.splitlines() if "=" in line)
    value = values.get("MainPID", "0")
    pid = int(value) if value.isascii() and value.isdigit() and len(value) <= 10 else 0
    if not 0 < pid < 2**31:
        pid = 0
    return values.get("ActiveState", "unknown"), pid


def check_running_binary(pid, installed, proc_root=Path("/proc")):
    if not pid:
        return Check("运行版本", "warn", "没有检测到正在运行的 Clarion 会话")
    try:
        if binary_digest(proc_root / str(pid) / "exe") != binary_digest(installed):
            return Check("运行版本", "warn", "运行会话与 PATH 启动版本不同；如需使用该版本，请保存工作后重新登录")
        return Check("运行版本", "ok", "当前会话与 PATH 中的启动版本一致")
    except OSError:
        return Check("运行版本", "warn", "当前会话的可执行文件无法核对")


def collect_checks(config=None, runner=run, which=shutil.which):
    checks = []
    compositor = which("clarion")
    for name in ("clarion", "clarion-settings", "clarion-session"):
        checks.append(check_launcher(name, which(name), Path("/usr/local/bin") / name))
    if compositor:
        code, _ = runner([compositor, "--version"])
        checks.append(Check("版本命令", "ok" if code == 0 else "error", "版本命令正常" if code == 0 else "版本命令失败或超时"))
        command = [compositor, "validate"]
        if config is not None:
            command.extend(["--config", str(config)])
        code, _ = runner(command)
        checks.append(Check("配置", "ok" if code == 0 else "error", "配置校验通过（未写入文件）" if code == 0 else "配置校验失败或超时；请运行 clarion validate 查看详情"))
        code, content = runner([compositor, "msg", "--json", "outputs"])
        try:
            outputs = json.loads(content) if code == 0 else None
        except (ValueError, TypeError):
            outputs = None
        if isinstance(outputs, dict):
            checks.append(Check("桌面 IPC", "ok" if outputs else "warn", f"IPC 可用，连接输出 {len(outputs)} 个"))
        else:
            checks.append(Check("桌面 IPC", "warn", "当前环境无法连接 Clarion IPC；会话可能未启动或环境变量不同"))
    service = which("systemctl")
    pid = 0
    if service:
        code, content = runner([service, "--user", "show", "clarion.service", "-p", "ActiveState", "-p", "MainPID"])
        state, pid = parse_service(content if code == 0 else "")
        checks.append(Check("用户服务", "ok" if state == "active" else "warn", "Clarion 用户服务正在运行" if state == "active" else "Clarion 用户服务未运行或无法查询；也可能是手动启动会话"))
    if not pid and which("pgrep"):
        code, content = runner([which("pgrep"), "-u", str(os.getuid()), "-x", "clarion"])
        candidates = content.split() if code == 0 else []
        if len(candidates) == 1:
            _, pid = parse_service("MainPID=" + candidates[0])
        elif len(candidates) > 1:
            checks.append(Check("运行版本", "warn", "检测到多个 Clarion 进程，无法确定当前会话版本"))
    if compositor and pid:
        # Validate, IPC and --version above use this same resolved entry point.
        # A managed per-user release can legitimately be newer than /usr/local.
        # check_launcher separately reports differing system copies; do not
        # misreport a successfully updated user session as requiring re-login.
        checks.append(check_running_binary(pid, compositor))
    elif compositor:
        checks.append(Check("运行版本", "warn", "未确定当前会话进程；不据此宣称运行版本已更新"))
    return checks


def main(argv=None):
    parser = argparse.ArgumentParser(description="Clarion 桌面状态自检（只读，不改配置或重启会话）")
    parser.add_argument("--json", action="store_true", help="输出结构化结果，不包含配置内容")
    parser.add_argument("--strict", action="store_true", help="警告也返回失败状态")
    parser.add_argument("--config", type=Path, help="校验指定配置；默认使用 Clarion 的配置搜索规则")
    args = parser.parse_args(argv)
    checks = collect_checks(args.config)
    failed = any(check.status == "error" or (args.strict and check.status == "warn") for check in checks)
    if args.json:
        print(json.dumps({"checks": [asdict(check) for check in checks], "failed": failed}, ensure_ascii=False, indent=2))
    else:
        for check in checks:
            print(f"[{check.status.upper():5}] {check.name}：{check.message}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
