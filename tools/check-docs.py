#!/usr/bin/env python3
"""文档链接检查：仓库内 Markdown 的相对链接必须能解析到真实文件。

用法：python3 tools/check-docs.py
CI：rust job 执行。外链（http/https/mailto）与纯锚点（#…）跳过；
仓库间引用（指向 hawk-server 等兄弟仓库）按约定写成文字描述而非链接，因此不在此放行。
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LINK = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
SKIP_PREFIXES = ("http://", "https://", "mailto:", "#", "file://")


def main() -> int:
    files = [ROOT / "README.md", ROOT / "AGENTS.md"]
    files += sorted((ROOT / "docs").rglob("*.md"))
    files += sorted(ROOT.glob("*/README.md"))
    files += sorted(ROOT.glob("*/docs/**/*.md"))

    checked = 0
    broken: list[str] = []
    for f in files:
        if not f.is_file():
            continue
        for m in LINK.finditer(f.read_text(encoding="utf-8")):
            target = m.group(1).split("#", 1)[0].strip()
            if not target or target.startswith(SKIP_PREFIXES):
                continue
            checked += 1
            if not (f.parent / target).resolve().exists():
                broken.append(f"{f.relative_to(ROOT)} -> {target}")

    if broken:
        print("失效链接：")
        for b in broken:
            print("  ", b)
        return 1
    print(f"文档链接检查通过（{checked} 条）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
