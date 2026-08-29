#!/usr/bin/env python3
"""缩略图生成基准:临时库 + N 张 PNG,测初始索引完成后缩略图/调色板队列清空耗时。

用法: python3 tools/bench-thumbnails.py [张数] [尺寸]
产物: hawk-server/tools/.tmp/bench-lib(脚本结束时清理)

采样方式:
  - t_ready: /health 转 200(初始索引完成,缩略图仍在后台生成)
  - t_done:  /api/v1/app/status 的 pending+active 归零(缩略图与调色板全部就绪)
  - CPU:    两次快照 dotnet 进程的 TotalProcessorTime 差 / 墙钟差(≈ 占用核数)
"""
import json
import struct
import subprocess
import sys
import tempfile
import time
import zlib
import os
from urllib.request import Request, urlopen

sys.stdout.reconfigure(encoding="utf-8")  # Windows 控制台默认非 UTF-8

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SERVER_DLL = os.path.join(REPO, "hawk-server", "bin", "Debug", "net10.0", "hawk-server.dll")
PORT = 27398
TOKEN = "bench-token"
BASE = f"http://127.0.0.1:{PORT}"

COUNT = int(sys.argv[1]) if len(sys.argv) > 1 else 96
SIZE = int(sys.argv[2]) if len(sys.argv) > 2 else 1600


def png_gradient(w, h, seed):
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    rows = []
    for y in range(h):
        row = bytearray(b"\x00")
        for x in range(w):
            row += bytes(((x * 255) // w, (y * 255) // h, (seed * 37) % 256))
        rows.append(bytes(row))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(b"".join(rows), 1)) + chunk(b"IEND", b"")


def api(path, method="GET"):
    req = Request(BASE + path, method=method, headers={"Authorization": f"Bearer {TOKEN}"})
    with urlopen(req) as res:
        return json.load(res)


def cpu_seconds(pid):
    out = subprocess.run(
        ["powershell", "-NoProfile", "-Command",
         f"(Get-Process -Id {pid}).TotalProcessorTime.TotalSeconds"],
        capture_output=True, text=True)
    return float(out.stdout.strip())


def main():
    lib = tempfile.mkdtemp(prefix="hawk-bench-")
    print(f"生成 {COUNT} 张 {SIZE}x{int(SIZE*0.75)} PNG → {lib}")
    t0 = time.time()
    for i in range(COUNT):
        with open(os.path.join(lib, f"img-{i:03d}.png"), "wb") as f:
            f.write(png_gradient(SIZE, int(SIZE * 0.75), i))
    print(f"图片生成耗时 {time.time() - t0:.1f}s")

    env = dict(os.environ, HAWK_TOKEN=TOKEN)
    proc = subprocess.Popen(["dotnet", SERVER_DLL, "--library", lib, "--port", str(PORT)],
                            env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        # 等就绪:初始索引(哈希+应用)完成
        while True:
            try:
                with urlopen(BASE + "/health", timeout=1) as res:
                    if res.status == 200:
                        break
            except Exception:
                pass
            time.sleep(0.2)
        t_ready = time.time()
        cpu0 = cpu_seconds(proc.pid)
        wall0 = time.time()

        # 等缩略图/调色板队列清空
        while True:
            status = api("/api/v1/app/status")["data"]["thumbnail"]
            if status["pending"] == 0 and status["active"] == 0:
                break
            time.sleep(0.2)
        t_done = time.time()
        cpu1 = cpu_seconds(proc.pid)
        wall1 = time.time()

        dt = wall1 - wall0
        cores = (cpu1 - cpu0) / dt
        print(f"初始索引完成 → 缩略图全部就绪: {dt:.1f}s  ({COUNT} 张, {COUNT/dt:.1f} 张/秒)")
        print(f"缩略图阶段 CPU 占用 ≈ {cores:.1f} 核")
    finally:
        proc.kill()
        import shutil
        shutil.rmtree(lib, ignore_errors=True)


if __name__ == "__main__":
    main()
