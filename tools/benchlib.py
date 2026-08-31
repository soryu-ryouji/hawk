"""压测公共库：hawk-server 生命周期、素材生成/采样、指标采集。

被 bench-startup / bench-ingest / bench-images / bench-scale 共用。
所有脚本输出 JSON（带 git SHA 与时间戳），改代码前后各跑一次即可对比。
"""
import hashlib
import json
import os
import random
import shutil
import socket
import struct
import subprocess
import sys
import time
import zlib
from urllib.request import Request, urlopen

sys.stdout.reconfigure(encoding="utf-8")  # Windows 控制台默认非 UTF-8

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUST_TARGET = {"win32": "x86_64-pc-windows-msvc", "darwin": "aarch64-apple-darwin", "linux": "x86_64-unknown-linux-gnu"}


def find_server_bin():
    exe = "hawk-server.exe" if sys.platform == "win32" else "hawk-server"
    triple = RUST_TARGET.get(sys.platform)
    candidates = []
    if triple:
        candidates.append(os.path.join(REPO, "hawk-server-rs", "target", triple, "release", exe))
    candidates += [
        os.path.join(REPO, "hawk-server-rs", "target", "release", exe),
        os.path.join(REPO, "hawk-server-rs", "target", "debug", exe),
    ]
    existing = [c for c in candidates if os.path.exists(c)]
    if not existing:
        raise SystemExit("未找到 hawk-server 构建产物，请先 cargo build --release")
    # 同名产物可能同时存在（cargo build 与 build-server.mjs 输出位置不同），取最新修改的
    return max(existing, key=os.path.getmtime)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def cache_dir_for(lib_abs, label):
    """库外派生缓存目录（与 server 的命名规则一致：<库文件夹名>_<根路径SHA-256前16位>）"""
    key = hashlib.sha256(lib_abs.encode()).hexdigest()[:16]
    if sys.platform == "win32":
        parent = os.environ["LOCALAPPDATA"]
    elif sys.platform == "darwin":
        parent = os.path.expanduser("~/Library/Application Support")
    else:
        parent = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    return os.path.join(parent, "hawk", "cache", f"{label}_{key}")


def repo_sha():
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"], cwd=REPO, capture_output=True, text=True, timeout=10
        )
        return out.stdout.strip() or "unknown"
    except Exception:
        return "unknown"


def report(name, metrics):
    """人类可读摘要 + 一行 JSON（带 git SHA / 时间戳，供历史对比）"""
    print(f"== {name} ==")
    for k, v in metrics.items():
        print(f"  {k}: {v}")
    line = {"bench": name, "sha": repo_sha(), "ts": time.strftime("%Y-%m-%dT%H:%M:%S"), **metrics}
    print("RESULT " + json.dumps(line, ensure_ascii=False))


class Server:
    """hawk-server 子进程生命周期 + API 客户端 + 资源采样"""

    def __init__(self, lib_dir, port=None, token="bench-token", stdout=subprocess.DEVNULL):
        self.lib_dir = os.path.abspath(lib_dir)
        self.port = port or free_port()
        self.token = token
        self.base = f"http://127.0.0.1:{self.port}"
        self.bin = find_server_bin()
        self.proc = None
        self.stdout = stdout

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, *exc):
        self.stop()

    def start(self, timeout_s=300):
        self.proc = subprocess.Popen(
            [self.bin, "--library", self.lib_dir, "--port", str(self.port)],
            stdout=self.stdout,
            stderr=subprocess.STDOUT,
            env={**os.environ, "HAWK_TOKEN": self.token},
        )
        t0 = time.perf_counter()
        while True:
            try:
                if self.health() == 200:
                    return time.perf_counter() - t0
            except Exception:
                pass
            if self.proc.poll() is not None:
                raise SystemExit(f"hawk-server 过早退出（code {self.proc.returncode}）")
            if time.perf_counter() - t0 > timeout_s:
                raise SystemExit(f"启动超时（{timeout_s}s）")
            time.sleep(0.2)

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait()
        self.proc = None

    def health(self):
        return urlopen(f"{self.base}/health", timeout=3).status

    def api(self, path, body=None):
        if body is None:
            req = Request(f"{self.base}{path}", headers={"Authorization": f"Bearer {self.token}"})
        else:
            req = Request(
                f"{self.base}{path}",
                data=json.dumps(body).encode(),
                headers={"Authorization": f"Bearer {self.token}", "Content-Type": "application/json"},
                method="POST",
            )
        with urlopen(req, timeout=60) as r:
            return json.load(r)

    def api_ms(self, path, body=None):
        t0 = time.perf_counter()
        self.api(path, body)
        return (time.perf_counter() - t0) * 1000

    def backlog(self):
        s = self.api("/api/v1/app/status")["data"]
        return s["thumbnail"]["pending"] + s["thumbnail"]["active"], s["index"]["pending"] + s["index"]["active"]

    def wait_idle(self, timeout_s=1200, poll_s=0.3):
        """缩略图 + 索引积压全部归零"""
        t0 = time.perf_counter()
        while True:
            t, i = self.backlog()
            if t == 0 and i == 0:
                return time.perf_counter() - t0
            if time.perf_counter() - t0 > timeout_s:
                raise SystemExit(f"积压清空超时（{timeout_s}s）：thumbnail={t} index={i}")
            time.sleep(poll_s)

    def count(self):
        return self.api("/api/v1/item/count")["data"]

    def rss_mb(self):
        return sample_process(self.proc.pid)[0]

    def cpu_time_s(self):
        return sample_process(self.proc.pid)[1]


def sample_process(pid):
    """采样进程资源：(RSS MB, CPU 秒)。进程不存在返回 (0, 0)"""
    if not pid:
        return 0.0, 0.0
    try:
        out = subprocess.run(
            [
                "powershell", "-NoProfile", "-Command",
                f"$p = Get-Process -Id {pid} -ErrorAction SilentlyContinue; "
                "if ($p) { '{0:N1} {1:N3}' -f ($p.WorkingSet64 / 1MB), $p.TotalProcessorTime.TotalSeconds } else { '0 0' }",
            ],
            capture_output=True, text=True, timeout=15,
        )
        parts = out.stdout.strip().split()
        return float(parts[0]), float(parts[1])
    except Exception:
        return 0.0, 0.0


def png_bytes(w, h, rng=None):
    """快速合成 PNG（内容随机，保证哈希唯一）。os.urandom 比逐像素生成快约 10 倍"""
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    raw = b"".join(b"\x00" + os.urandom(w * 3) for _ in range(h))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 1))
        + chunk(b"IEND", b"")
    )


def make_pngs(dir_path, count, sizes=(96, 128, 160), seed=42, subdirs=0):
    """生成 count 张随机 PNG；返回文件路径列表"""
    os.makedirs(dir_path, exist_ok=True)
    rng = random.Random(seed)
    paths = []
    for i in range(count):
        if subdirs:
            d = os.path.join(dir_path, f"batch{i % subdirs}")
            os.makedirs(d, exist_ok=True)
        else:
            d = dir_path
        w = rng.choice(sizes)
        p = os.path.join(d, f"img{i:06d}.png")
        with open(p, "wb") as f:
            f.write(png_bytes(w, w, rng))
        paths.append(p)
    return paths


def sample_copy(src_library, dst_dir, count, seed=42, exts=(".jpg", ".jpeg", ".png", ".webp", ".bmp", ".gif")):
    """从真实素材库随机采样 count 张图复制到 dst_dir（图像管线压测用真实照片才有意义）"""
    files = []
    for root, _, names in os.walk(src_library):
        if ".hawk" in root.split(os.sep):
            continue
        for n in names:
            if n.lower().endswith(exts):
                files.append(os.path.join(root, n))
    if not files:
        raise SystemExit(f"{src_library} 下未找到图像文件")
    rng = random.Random(seed)
    picked = rng.sample(files, min(count, len(files)))
    os.makedirs(dst_dir, exist_ok=True)
    total = 0
    for p in picked:
        dst = os.path.join(dst_dir, os.path.basename(p))
        if os.path.exists(dst):  # 同名不同目录的文件，加序号避免覆盖
            stem, ext = os.path.splitext(os.path.basename(p))
            dst = os.path.join(dst_dir, f"{stem}-{total}{ext}")
        shutil.copy2(p, dst)
        total += 1
    return total, sum(os.path.getsize(p) for p in picked)


def dir_bytes(dir_path):
    total = 0
    for root, _, names in os.walk(dir_path):
        for n in names:
            total += os.path.getsize(os.path.join(root, n))
    return total


def percentile(values, p):
    if not values:
        return 0
    values = sorted(values)
    return values[min(len(values) - 1, int(len(values) * p))]


def index_db_stats(cache_dir):
    """直读派生缓存 index.db：镜像行数 / 已提炼调色板数"""
    import sqlite3

    db_file = os.path.join(cache_dir, "index.db")
    if not os.path.exists(db_file):
        return {"items": 0, "palette_done": 0}
    db = sqlite3.connect(db_file)
    try:
        items = db.execute("SELECT COUNT(*) FROM items").fetchone()[0]
        palette = db.execute("SELECT COUNT(*) FROM items WHERE palette IS NOT NULL").fetchone()[0]
        return {"items": items, "palette_done": palette}
    finally:
        db.close()
