#!/usr/bin/env python3
"""低带宽传输量基准：实测图片档位/原图与 skeleton gzip 字节数。

用法: python3 tools/bench-wan.py [--items 50] [--bandwidth 3]
需先构建 hawk-daemon（release 或 debug）。素材、缓存和日志均放 tools/.tmp，结束清理。

输出中的秒数是按指定带宽换算的理论下限（1 MB = 1,000,000 bytes），
不是 loopback 请求耗时，也不包含 RTT、丢包、中继或浏览器解码成本。
三图预览按当前实现的当前/上一张/下一张估算：旧策略三份原图，新策略三份 1024 WebP。
"""
import argparse
import json
import os
import shutil
import sys
import urllib.error
from urllib.request import Request, urlopen

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from benchlib import Server, make_pngs, png_bytes, report

WORK_ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".tmp", "bench-wan")
LIB = os.path.join(WORK_ROOT, "library")
CACHE = os.path.join(WORK_ROOT, "cache")


def response_bytes(srv, path, body=None, accept_encoding=None):
    headers = {"Authorization": f"Bearer {srv.token}"}
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    if accept_encoding:
        headers["Accept-Encoding"] = accept_encoding
    req = Request(f"{srv.base}{path}", data=data, headers=headers, method="POST" if body is not None else "GET")
    with urlopen(req, timeout=120) as response:
        return response.read(), response.headers


def thumbnail_bytes(srv, item_id, size):
    path = f"/api/v1/item/thumbnail?id={item_id}&size={size}"
    try:
        return response_bytes(srv, path)
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
    # 增量素材允许惰性生成：404 已入队，等全部缺失档完成后重试
    srv.wait_idle()
    return response_bytes(srv, path)


def saved_pct(before, after):
    return round((1 - after / max(before, 1)) * 100, 1)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--items", type=int, default=50, help="小图数量；另加一张 1280×1280 大图")
    parser.add_argument("--bandwidth", type=float, default=3.0, help="理论换算带宽（Mb/s）")
    args = parser.parse_args()
    if args.items < 1 or args.bandwidth <= 0:
        parser.error("--items 须大于 0，--bandwidth 须大于 0")

    shutil.rmtree(WORK_ROOT, ignore_errors=True)
    os.makedirs(LIB)
    make_pngs(LIB, args.items, sizes=(96, 128, 160))
    with open(os.path.join(LIB, "large.png"), "wb") as large:
        large.write(png_bytes(1280, 1280))

    try:
        with Server(LIB, cache_parent=CACHE) as srv:
            srv.wait_idle()
            listed = srv.api("/api/v1/item/list", {"limit": args.items + 1})["data"]
            item = max(listed["items"], key=lambda entry: entry["size"])
            item_id = item["id"]

            grid, grid_headers = thumbnail_bytes(srv, item_id, 512)
            preview, preview_headers = thumbnail_bytes(srv, item_id, 1024)
            original, _ = response_bytes(srv, f"/api/v1/item/file?id={item_id}")
            plain, plain_headers = response_bytes(srv, "/api/v1/item/skeleton", {})
            compressed, compressed_headers = response_bytes(
                srv, "/api/v1/item/skeleton", {}, accept_encoding="gzip"
            )

            assert grid_headers.get_content_type() == "image/webp"
            assert preview_headers.get_content_type() == "image/webp"
            assert json.loads(plain)["status"] == "success"
            assert compressed_headers.get("Content-Encoding") == "gzip"
            assert compressed[:2] == b"\x1f\x8b"
            assert plain_headers.get("Content-Encoding") != "gzip"

        bps = args.bandwidth * 1_000_000
        seconds = lambda size: round(size * 8 / bps, 2)
        old_carousel = len(original) * 3
        new_carousel = len(preview) * 3
        report("wan", {
            "items": len(listed["items"]),
            "bandwidth_mbps": args.bandwidth,
            "grid_512_bytes": len(grid),
            "preview_1024_bytes": len(preview),
            "original_bytes": len(original),
            "grid_vs_1024_saved_pct": saved_pct(len(preview), len(grid)),
            "preview_vs_original_saved_pct": saved_pct(len(original), len(preview)),
            "grid_512_s": seconds(len(grid)),
            "preview_1024_s": seconds(len(preview)),
            "original_s": seconds(len(original)),
            "old_three_original_s": seconds(old_carousel),
            "new_three_preview_s": seconds(new_carousel),
            "skeleton_plain_bytes": len(plain),
            "skeleton_gzip_bytes": len(compressed),
            "skeleton_gzip_saved_pct": saved_pct(len(plain), len(compressed)),
            "note": "秒数为指定带宽理论下限；非真实广域网耗时",
        })
    finally:
        shutil.rmtree(WORK_ROOT, ignore_errors=True)


if __name__ == "__main__":
    main()
