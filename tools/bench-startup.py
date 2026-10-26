#!/usr/bin/env python3
"""启动性能压测：首次建库与已有 SQLite 库的热启动。

用法: python3 tools/bench-startup.py [--items 5000] [--runs 3]
产出: tools/.tmp/bench-startup-lib（结束时清理）

指标:
  - t_ready_*_s     /health 转 200（内存索引注水完成；热启动是启动屏用户感知速度）
  - t_scan_s        就绪后首轮对账扫描 + 派生缓存补齐完成（wait_idle）
  - rss_ready_mb    就绪时内存
首次建库额外:
  - t_ready_cold_s  首次创建 metadata.db 并注水
  - t_scan_cold_s   首次全库扫描 + 缩略图/调色板/pHash 生成
"""
import argparse
import os
import shutil
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from benchlib import Server, cache_dir_for, make_pngs, report

WORK_ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".tmp")
LIB = os.path.join(WORK_ROOT, "bench-startup-lib")
LABEL = "bench-startup-lib"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--items", type=int, default=5000)
    ap.add_argument("--runs", type=int, default=3, help="热启动重复次数取中位")
    args = ap.parse_args()

    shutil.rmtree(LIB, ignore_errors=True)
    cache_dir_for(LIB and os.path.abspath(LIB), LABEL)  # 占位无副作用
    make_pngs(LIB, args.items, subdirs=32)
    lib_abs = os.path.abspath(LIB).replace("\\", "/")

    # 首次建库：创建 metadata.db + 全量扫描与派生数据生成
    t0 = time.perf_counter()
    with Server(LIB) as srv:
        t_ready_cold = srv.start(timeout_s=1800)
        srv.wait_idle()
        t_scan_cold = time.perf_counter() - t0
        rss_cold = srv.rss_mb()

    # 热启动（已有 SQLite 库直接注水）
    hot = []
    rss = 0
    for i in range(args.runs):
        with Server(LIB) as srv:
            t_ready = srv.start()
            srv.wait_idle()
            hot.append(round(t_ready, 2))
            rss = srv.rss_mb()
    hot.sort()

    shutil.rmtree(WORK_ROOT, ignore_errors=True)
    shutil.rmtree(cache_dir_for(lib_abs, LABEL), ignore_errors=True)

    report("startup", {
        "items": args.items,
        "t_ready_hot_s": hot[len(hot) // 2],
        "t_ready_hot_all": hot,
        "rss_ready_mb": round(rss, 1),
        "t_ready_cold_s": round(t_ready_cold, 2),
        "t_scan_cold_total_s": round(t_scan_cold, 1),
        "rss_cold_mb": round(rss_cold, 1),
    })


if __name__ == "__main__":
    main()
