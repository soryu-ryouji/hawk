#!/usr/bin/env python3
"""缩略图/调色板管线压测：吞吐、CPU 效率、WebP 压缩率、palette 覆盖率。

用法: python3 tools/bench-images.py [--count 500] [--from D:/Materials | --synthetic]
默认从真实素材库随机采样（解码/编码真实照片才有对比意义；合成小图可用 --synthetic）。
产出: tools/.tmp/bench-images-lib（结束时清理，含系统缓存目录）

管线两阶段（缩略图为惰性缓存，读取端触发）：
  1. 入库 + 调色板即时生成（颜色搜索依赖全量 palette，wait_idle 等待完成）
  2. 逐个请求 /item/thumbnail 触发缩略图后台生成（未命中先回源原图并入队）

指标:
  - img_per_s        全管线吞吐（调色板 + 缩略图两阶段合计）
  - cpu_ms_per_img   每图 CPU 毫秒（换缩放算法/编码器/质量参数时的核心对比项）
  - webp_ratio_*     各尺寸 webp 总字节 / 源图总字节（压缩效率）
  - palette_done_pct 调色板覆盖率（丢失/失败检测；应恒 100%）
  - thumbs_missing   素材总数 - 256 缩略图数（worker 丢失回归检测）
"""
import argparse
import os
import shutil
import sys
import time
from urllib.request import Request, urlopen

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from benchlib import (
    Server, cache_dir_for, dir_bytes, index_db_stats, make_pngs, report, sample_copy,
)

WORK_ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".tmp")
LIB = os.path.join(WORK_ROOT, "bench-images-lib")
LABEL = "bench-images-lib"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--count", type=int, default=500)
    ap.add_argument("--from", dest="src", default=r"D:\Materials", help="真实采样源库")
    ap.add_argument("--synthetic", action="store_true", help="用合成 PNG 代替真实采样")
    ap.add_argument("--sizes", default="256,512,1024", help="config.toml 的 thumbnail_sizes")
    args = ap.parse_args()

    shutil.rmtree(WORK_ROOT, ignore_errors=True)
    os.makedirs(LIB)
    if args.synthetic:
        make_pngs(LIB, args.count, sizes=(640, 1280))
        src_bytes = dir_bytes(LIB)
    else:
        n, src_bytes = sample_copy(args.src, LIB, args.count)
        print(f"采样 {n} 张真实图片，共 {src_bytes / 1e6:.1f} MB")
    # 缩略图尺寸配置（LibraryConfig 对已存在的 config.toml 不覆盖，先写后启）
    os.makedirs(os.path.join(LIB, ".hawk"), exist_ok=True)
    with open(os.path.join(LIB, ".hawk", "config.toml"), "w") as f:
        f.write(f'thumbnail_sizes = [{args.sizes}]\n')

    lib_abs = os.path.abspath(LIB).replace("\\", "/")
    cache_dir = cache_dir_for(lib_abs, LABEL)

    # server 日志落盘：区分「解码失败」（真实素材的边角格式）与「任务丢失」（回归）
    log_file = open(os.path.join(WORK_ROOT, "bench-images-server.log"), "w")
    with Server(LIB, stdout=log_file) as srv:
        cpu0 = srv.cpu_time_s()
        t0 = time.perf_counter()
        srv.wait_idle()  # 阶段1：入库 + 调色板即时管线
        # 调色板批量回写按 2s 时限冲刷：积压归零后再等一拍，确保暂存的回写落盘后再统计
        time.sleep(2.5)

        # 阶段2：缩略图惰性生成——逐个请求端点触发（未命中回源原图并入队，in-flight 去重）
        listed = srv.api("/api/v1/item/list", {"limit": args.count + 1000, "in_trash": False})["data"]
        expected = [it["id"] for it in listed["items"]]
        t1 = time.perf_counter()
        for item_id in expected:
            req = Request(
                f"{srv.base}/api/v1/item/thumbnail?id={item_id}&size=256",
                headers={"Authorization": f"Bearer {srv.token}"},
            )
            with urlopen(req, timeout=60) as r:
                r.read()  # 响应为原图（未命中回源）或已生成 webp，丢弃即可
        srv.wait_idle()  # 缩略图生成完毕
        t_thumb = time.perf_counter() - t1
        wall = time.perf_counter() - t0
        cpu1 = srv.cpu_time_s()
    log_file.close()
    log_lines = []
    with open(os.path.join(WORK_ROOT, "bench-images-server.log"), encoding="utf-8", errors="replace") as f:
        log_lines = f.readlines()
    decode_failures = sum(1 for line in log_lines if "缩略图解码失败" in line)
    batch_failures = sum(1 for line in log_lines if ("批量写入失败" in line or "熔断" in line or "TOML" in line and "失败" in line))
    palette_flushes = sum(1 for line in log_lines if "元数据缓存批量" in line or "调色板" in line and "冲刷" in line)
    if batch_failures:
        print("!! 批量写库失败日志（前 3 行）:")
        for line in [l for l in log_lines if "批量写入失败" in l or "熔断" in l][:3]:
            print("   " + line.strip()[:200])

    # 直读派生缓存统计（索引镜像行数 / 已提炼调色板数 / 缩略图数）
    stats = index_db_stats(cache_dir)
    thumbs_256 = len(os.listdir(os.path.join(cache_dir, "thumbnails", "256"))) if os.path.isdir(
        os.path.join(cache_dir, "thumbnails", "256")) else 0

    ratios = {}
    for size in args.sizes.split(","):
        d = os.path.join(cache_dir, "thumbnails", size)
        if os.path.isdir(d):
            ratios[f"webp_ratio_{size}"] = round(dir_bytes(d) / max(src_bytes, 1), 3)

    shutil.rmtree(WORK_ROOT, ignore_errors=True)
    shutil.rmtree(cache_dir, ignore_errors=True)

    report("images", {
        "count": stats["items"],
        "src_mb": round(src_bytes / 1e6, 1),
        "wall_s": round(wall, 1),
        "wall_thumb_s": round(t_thumb, 1),
        "img_per_s": round(stats["items"] / wall, 1),
        "cpu_ms_per_img": round((cpu1 - cpu0) * 1000 / max(stats["items"], 1), 1),
        **ratios,
        "palette_done_pct": round(stats["palette_done"] * 100 / max(stats["items"], 1), 1),
        "thumb_expected": len(expected),
        "thumbs_missing": len(expected) - thumbs_256,
        "decode_failures": decode_failures,
        "batch_write_failures": batch_failures,
    })


if __name__ == "__main__":
    main()
