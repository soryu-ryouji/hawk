#!/usr/bin/env python3
"""运行期入库与重建索引压测：watcher 批量入库吞吐、reindex 全量/复用、洪峰期读延迟。

用法: python3 tools/bench-ingest.py [--files 2000]
产出: tools/.tmp/bench-ingest-lib（结束时清理）

指标:
  - ingest_*        向运行中的库复制 N 文件（watcher 事件路径）到 count 达标：
                    吞吐、单文件均延、期间 API 读 p50/p99（UI 卡死代理指标）
  - reindex_s       library/reindex 全量重算哈希（提交 → 积压归零）
  - reindex_reuse_s 紧接着再 reindex（路径+size/mtime 复用，应远快于全量）
  - hash 阶段 CPU 吃满度由任务管理器观察；rss_mb 供内存对比
"""
import argparse
import os
import shutil
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from benchlib import Server, make_pngs, percentile, report

WORK_ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".tmp")
LIB = os.path.join(WORK_ROOT, "bench-ingest-lib")
STAGING = os.path.join(WORK_ROOT, "bench-ingest-staging")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--files", type=int, default=2000)
    args = ap.parse_args()

    shutil.rmtree(WORK_ROOT, ignore_errors=True)
    os.makedirs(LIB)
    # 预生成到库外（避免生成期间被 watcher 计入）
    make_pngs(STAGING, args.files, subdirs=16)

    read_lat = []
    stop_flag = threading.Event()

    def reader(srv):
        while not stop_flag.is_set():
            try:
                read_lat.append(srv.api_ms("/api/v1/item/count"))
            except Exception:
                pass
            time.sleep(0.1)

    with Server(LIB) as srv:
        # watcher 批量入库
        t0 = time.perf_counter()
        stop_flag.clear()
        th = threading.Thread(target=reader, args=(srv,), daemon=True)
        th.start()
        for root, _, names in os.walk(STAGING):
            for n in names:
                shutil.move(os.path.join(root, n), os.path.join(LIB, n))
        # 等待 count 达标（防抖窗口 + 队列消化）
        deadline = t0 + 600
        while srv.count() < args.files and time.perf_counter() < deadline:
            time.sleep(0.5)
        t_ingest = time.perf_counter() - t0
        srv.wait_idle()
        stop_flag.set()

        rss = srv.rss_mb()

        # 全量 reindex（重算全部哈希）
        t0 = time.perf_counter()
        srv.api("/api/v1/library/reindex", {})
        srv.wait_idle()
        t_reindex = time.perf_counter() - t0

        # 复用 reindex（无变化：路径 + size/mtime 命中元数据，不重算哈希）
        t0 = time.perf_counter()
        srv.api("/api/v1/library/reindex", {})
        srv.wait_idle()
        t_reindex_reuse = time.perf_counter() - t0

    shutil.rmtree(WORK_ROOT, ignore_errors=True)
    cache = cache_dir_for_ingest(LIB)
    shutil.rmtree(cache, ignore_errors=True)

    report("ingest", {
        "files": args.files,
        "ingest_s": round(t_ingest, 1),
        "ingest_files_per_s": round(args.files / t_ingest, 1),
        "flood_read_p50_ms": round(percentile(read_lat, 0.5), 1),
        "flood_read_p99_ms": round(percentile(read_lat, 0.99), 1),
        "rss_mb": round(rss, 1),
        "reindex_s": round(t_reindex, 1),
        "reindex_reuse_s": round(t_reindex_reuse, 1),
        "reindex_reuse_ratio": round(t_reindex / max(t_reindex_reuse, 0.01), 1),
    })


def cache_dir_for_ingest(lib):
    from benchlib import cache_dir_for

    return cache_dir_for(os.path.abspath(lib).replace("\\", "/"), "bench-ingest-lib")


if __name__ == "__main__":
    main()
