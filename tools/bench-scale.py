#!/usr/bin/env python3
"""大规模库全链路压测：启动、批量入库、缩略图洪峰期 API 读延迟、查询延迟、内存。

用法: python3 tools/bench-scale.py [--items 30000]
产出: tools/.tmp/bench-scale-lib（结束时清理，含系统缓存目录）

指标:
  - t_ready_s        热启动就绪（SQLite 注水）
  - t_done_s         全链路完成（扫描 + 缩略图 + 调色板）
  - flood_read_p99   后台洪峰期间 API 读 p99（UI 卡死代理指标）
  - skeleton_ms      全量骨架（虚拟网格布局；前端重连重同步路径）
  - list_page_ms     首页 50 条（视口窗口）
  - sort_*_ms        各排序字段的 skeleton 延迟
  - color_ms         颜色检索延迟（ΔE 全库扫描）
  - thumbs_missing   派生缓存丢失回归检测（应恒 0）
  - rss_*_mb         内存水位
"""
import argparse
import os
import shutil
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from benchlib import (
    Server, cache_dir_for, dir_bytes, index_db_stats, make_pngs, percentile, report,
)

WORK_ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".tmp")
LIB = os.path.join(WORK_ROOT, "bench-scale-lib")
LABEL = "bench-scale-lib"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--items", type=int, default=30000)
    args = ap.parse_args()

    shutil.rmtree(WORK_ROOT, ignore_errors=True)
    os.makedirs(LIB)
    t0 = time.perf_counter()
    make_pngs(LIB, args.items, sizes=(96, 128, 160, 224), subdirs=64)
    print(f"生成 {args.items} 张 PNG: {time.perf_counter() - t0:.1f}s")

    lib_abs = os.path.abspath(LIB).replace("\\", "/")
    cache_dir = cache_dir_for(lib_abs, LABEL)

    with Server(LIB) as srv:
        t_ready = srv.start()
        rss_ready = srv.rss_mb()

        # 洪峰期轮询读延迟（缩略图/调色板后台生成中）
        flood = []
        t0 = time.perf_counter()
        while True:
            tp, ip = srv.backlog()
            if tp == 0 and ip == 0 and len(flood) > 3:
                break
            try:
                flood.append(srv.api_ms("/api/v1/item/count"))
            except Exception:
                pass
            if time.perf_counter() - t0 > 1800:
                raise SystemExit("积压清空超时")
            time.sleep(0.2)
        t_done = time.perf_counter() - t0
        # 调色板批量回写按 2s 时限冲刷：再等一拍让暂存回写落盘
        time.sleep(2.5)
        # 稳态读延迟对照
        steady = [srv.api_ms("/api/v1/item/count") for _ in range(10)]

    # 重新起 server 做查询延迟（库已就绪，热启动）
    with Server(LIB) as srv:
        srv.start()
        rss_peak = srv.rss_mb()
        skeleton = sorted(srv.api_ms("/api/v1/item/skeleton", {}) for _ in range(3))[1]
        list_page = sorted(srv.api_ms("/api/v1/item/list", {}) for _ in range(3))[1]
        folder_list = sorted(srv.api_ms("/api/v1/folder/list") for _ in range(3))[1]
        sort_name = sorted(srv.api_ms("/api/v1/item/skeleton", {"order_by": "name"}) for _ in range(3))[1]
        sort_size = sorted(srv.api_ms("/api/v1/item/skeleton", {"order_by": "size"}) for _ in range(3))[1]
        color = sorted(srv.api_ms("/api/v1/item/list", {"color": "#808080"}) for _ in range(3))[1]
        count = srv.count()

    stats = index_db_stats(cache_dir)
    thumbs_256 = (
        len(os.listdir(os.path.join(cache_dir, "thumbnails", "256")))
        if os.path.isdir(os.path.join(cache_dir, "thumbnails", "256"))
        else 0
    )

    shutil.rmtree(WORK_ROOT, ignore_errors=True)
    shutil.rmtree(cache_dir, ignore_errors=True)

    report("scale", {
        "items": count,
        "t_ready_s": round(t_ready, 1),
        "t_done_s": round(t_done, 1),
        "flood_read_p50_ms": round(percentile(flood, 0.5), 1),
        "flood_read_p99_ms": round(percentile(flood, 0.99), 1),
        "steady_read_p99_ms": round(percentile(steady, 0.99), 1),
        "skeleton_ms": round(skeleton, 1),
        "list_page_ms": round(list_page, 1),
        "folder_list_ms": round(folder_list, 1),
        "sort_name_ms": round(sort_name, 1),
        "sort_size_ms": round(sort_size, 1),
        "color_ms": round(color, 1),
        "rss_ready_mb": round(rss_ready, 1),
        "rss_peak_mb": round(rss_peak, 1),
        "palette_done_pct": round(stats["palette_done"] * 100 / max(count, 1), 1),
        "thumbs_missing": count - thumbs_256,
    })


if __name__ == "__main__":
    main()
