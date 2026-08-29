#!/usr/bin/env bash
# hawk-server 端到端冒烟测试：临时素材库 + curl 覆盖主要 API 流程。
# 用法: tools/smoke.sh（需先 dotnet build）
#
# Windows Git Bash 注意：curl 是原生 Windows 程序，argv 中的中文会被 MSYS2
# 转成 ANSI 编码（GBK）导致 JSON 体非法。因此所有 JSON POST 体一律经 stdin
# 传递（post_json 辅助函数）；jq 过滤程序中也不得出现中文（同样走 argv）。
set -euo pipefail

cd "$(dirname "$0")/.."
WORK="$PWD/tools/.tmp"
LIB="$WORK/library"
PORT=27399
TOKEN="smoke-test-token"
BASE="http://127.0.0.1:$PORT"
AUTH="Authorization: Bearer $TOKEN"

rm -rf "$WORK"
mkdir -p "$LIB/海报"

# 库外派生缓存目录：~/.local/share/hawk/cache/<库标识>（库标识 = 库根路径 SHA-256 前 16 位，见 LibraryPaths.LibraryKey）
CACHE_KEY=$(printf '%s' "$LIB" | sha256sum | cut -c1-16)
CACHE="${XDG_DATA_HOME:-$HOME/.local/share}/hawk/cache/$CACHE_KEY"

# 生成三张不同内容的 PNG（4x2 / 2x4 / 8x8）
python3 - "$LIB" <<'PYEOF'
import struct, zlib, sys, os

def png(w, h, rgb):
    def chunk(t, d):
        c = struct.pack('>I', len(d)) + t + d
        return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    ihdr = struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)
    raw = b''.join(b'\x00' + bytes(rgb) * w for _ in range(h))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', ihdr) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')

lib = sys.argv[1]
open(os.path.join(lib, 'sunset.png'), 'wb').write(png(4, 2, (255, 0, 0)))
open(os.path.join(lib, '海报', 'cat.png'), 'wb').write(png(2, 4, (0, 255, 0)))
open(os.path.join(lib, '海报', 'logo.png'), 'wb').write(png(8, 8, (0, 0, 255)))
PYEOF

HAWK_TOKEN=$TOKEN dotnet bin/Debug/net10.0/hawk-server.dll --library "$LIB" --port $PORT >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true; rm -rf "$WORK"' EXIT

for _ in $(seq 1 60); do
  curl -sf "$BASE/health" >/dev/null 2>&1 && break
  sleep 0.5
done

PASS=0; FAIL=0
check() { # check <描述> <实际> <期望>
  if [[ "$2" == "$3" ]]; then PASS=$((PASS+1)); echo "ok   - $1";
  else FAIL=$((FAIL+1)); echo "FAIL - $1: 期望 [$3] 实际 [$2]"; fi
}

post_json() { # post_json <url> <json-body> [额外 curl 参数...]：JSON 体经 stdin 传递（见文件头说明）
  local url="$1" body="$2"
  shift 2
  curl -s -H "$AUTH" -X POST "$url" -H 'Content-Type: application/json' --data-binary @- "$@" <<< "$body"
}

# --- app / 鉴权 ---
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) EXPECT_PLATFORM=windows ;;
  Darwin*) EXPECT_PLATFORM=macos ;;
  *) EXPECT_PLATFORM=linux ;;
esac
check "health 无需 token" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/health")" 200
check "无 token 返回 401" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/app/info")" 401
check "app/info.platform" "$(curl -s -H "$AUTH" "$BASE/api/v1/app/info" | jq -r .data.platform)" "$EXPECT_PLATFORM"

# --- library ---
check "library/info.name 缺省为目录名" "$(curl -s -H "$AUTH" "$BASE/api/v1/library/info" | jq -r .data.name)" library
check "reindex 立即返回" "$(post_json "$BASE/api/v1/library/reindex" '{}' | jq -r .status)" success

# --- 初始索引 ---
sleep 1 # 等 reindex 流水线跑完
check "item/count = 3" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 3
LIST=$(post_json "$BASE/api/v1/item/list" '{}')
check "item/list.total = 3" "$(echo "$LIST" | jq -r .data.total)" 3
check "初始宽度识别 (sunset=4)" "$(echo "$LIST" | jq -r '.data.items[] | select(.name=="sunset") | .width')" 4
check "folders 派生" "$(echo "$LIST" | jq -r '.data.items[] | select(.name=="cat") | .folders[0]')" "海报"

SUNSET_ID=$(echo "$LIST" | jq -r '.data.items[] | select(.name=="sunset") | .id')
check "id 为 64 位 hex" "${#SUNSET_ID}" 64

# --- 元数据读写 ---
post_json "$BASE/api/v1/item/update" "{\"id\":\"$SUNSET_ID\",\"tags\":[\"nature\",\"sunset\"],\"star\":4,\"annotation\":\"Beautiful sunset\"}" >/dev/null
DETAIL=$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID")
check "update tags 生效" "$(echo "$DETAIL" | jq -r '.data.tags | join(",")')" "nature,sunset"
check "update star 生效" "$(echo "$DETAIL" | jq -r '.data.star')" 4
check "按 tags 过滤" "$(post_json "$BASE/api/v1/item/list" '{"tags":["nature"]}' | jq -r .data.total)" 1
check "按 keywords 过滤" "$(post_json "$BASE/api/v1/item/list" '{"keywords":["beautiful"]}' | jq -r .data.total)" 1
check "元数据文件已落盘" "$(ls "$LIB/.hawk/metadata/$SUNSET_ID.toml" >/dev/null 2>&1 && echo yes)" yes

# --- 分类（Category） ---
CAT_API="$BASE/api/v1/category"
check "创建空分类（含祖先）" "$(post_json "$CAT_API/create" '{"path":"灵感/构图"}' | jq -r .status)" success
check "空分类出现在分类树" "$(curl -s -H "$AUTH" "$CAT_API/list" | jq -r '.data.children[0].children[0].name')" "构图"
check "重复创建分类返回 CATEGORY_EXISTS" "$(post_json "$CAT_API/create" '{"path":"灵感"}' | jq -r .error.code)" CATEGORY_EXISTS
post_json "$BASE/api/v1/item/update" "{\"id\":\"$SUNSET_ID\",\"categories\":[\"灵感/构图\",\"参考\"]}" >/dev/null
check "item 分类赋值生效" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.categories | join(",")')" "灵感/构图,参考"
check "按分类过滤（含子分类）" "$(post_json "$BASE/api/v1/item/list" '{"categories":["灵感"]}' | jq -r .data.total)" 1
check "分类过滤 all 语义" "$(post_json "$BASE/api/v1/item/list" '{"categories":["灵感","参考"],"categories_match":"all"}' | jq -r .data.total)" 1
check "排除分类" "$(post_json "$BASE/api/v1/item/list" '{"exclude_categories":["灵感"]}' | jq -r .data.total)" 2
check "分类重命名子树跟随" "$(post_json "$CAT_API/update" '{"path":"灵感","name":"灵感库"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.categories[0]')" "灵感库/构图"
check "分类删除清除赋值" "$(post_json "$CAT_API/delete" '{"path":"灵感库"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.categories | join(",")')" "参考"
check "注册表文件已落盘" "$(ls "$LIB/.hawk/categories.toml" >/dev/null 2>&1 && echo yes)" yes

# --- 标签注册表（Tag） ---
TAG_API="$BASE/api/v1/tag"
check "创建空标签" "$(post_json "$TAG_API/create" '{"name":"待审核"}' | jq -r .status)" success
check "空标签出现在标签列表" "$(curl -s -H "$AUTH" "$TAG_API/list" | jq -r '.data[] | select(.count==0) | .name')" "待审核"
check "标签重命名跟随 item" "$(post_json "$TAG_API/update" '{"name":"sunset","new_name":"晚霞"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.tags | join(",")')" "nature,晚霞"
check "排除标签过滤" "$(post_json "$BASE/api/v1/item/list" '{"exclude_tags":["nature"]}' | jq -r .data.total)" 2
check "标签删除同步清除" "$(post_json "$TAG_API/delete" '{"name":"晚霞"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.tags | join(",")')" "nature"

# --- 缩略图 ---
for _ in $(seq 1 20); do
  curl -sf -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID&size=256" -o "$WORK/t.webp" && break
  sleep 0.5
done
check "thumbnail content-type" "$(curl -s -o /dev/null -w '%{content_type}' -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID&size=256")" "image/webp"
check "thumbnail 支持 ?token=（<img> 场景）" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID&size=256&token=$TOKEN")" 200
check "thumbnail 错误 token 返回 401" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID&token=wrong")" 401
check "thumbnail cache-control" "$(curl -s -D - -o /dev/null -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID" | grep -i cache-control | tr -d '\r' | tr 'A-Z' 'a-z')" "cache-control: public, max-age=31536000, immutable"
check "thumbnail 不可缓存尺寸 400" "$(curl -s -o /dev/null -w '%{http_code}' -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID&size=400")" 400
check "refresh_thumbnail" "$(post_json "$BASE/api/v1/item/refresh_thumbnail" "{\"id\":\"$SUNSET_ID\"}" | jq -r .status)" success

# --- 调色板与颜色检索 ---
for _ in $(seq 1 20); do
  P=$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.palette | length')
  G=$(post_json "$BASE/api/v1/item/list" '{"color":"#00ff00"}' | jq -r '.data.total')
  [[ "$P" -ge 1 && "$G" == "1" ]] && break
  sleep 0.5
done
check "调色板提炼（纯色图为单色）" "$P" 1
check "cat 调色板就绪（绿色检索命中）" "$G" 1
check "主色为 #ff0000" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.palette[0].color')" "#ff0000"
check "主色占比 100" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.palette[0].percentage')" 100
check "颜色检索命中同色" "$(post_json "$BASE/api/v1/item/list" '{"color":"#ff0000"}' | jq -r .data.total)" 1
check "颜色检索相近色命中" "$(post_json "$BASE/api/v1/item/list" '{"color":"#ee0000"}' | jq -r .data.total)" 1
check "颜色检索异色不命中" "$(post_json "$BASE/api/v1/item/list" '{"color":"#ffff00"}' | jq -r .data.total)" 0
check "颜色检索限定文件夹范围" "$(post_json "$BASE/api/v1/item/list" '{"color":"#00ff00","folders":["海报"]}' | jq -r .data.total)" 1
check "颜色检索范围外不命中" "$(post_json "$BASE/api/v1/item/list" '{"color":"#ff0000","folders":["海报"]}' | jq -r .data.total)" 0
check "非法颜色值返回 400" "$(post_json "$BASE/api/v1/item/list" '{"color":"red"}' -o /dev/null -w '%{http_code}')" 400
check "调色板缓存已落盘" "$(ls "$CACHE/colors/$SUNSET_ID.json" >/dev/null 2>&1 && echo yes)" yes

# --- item/add ---
TINY_PNG="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
ADD=$(post_json "$BASE/api/v1/item/add" "{\"img_base64\":\"$TINY_PNG\",\"name\":\"dot\",\"folder_path\":\"海报\",\"tags\":[\"imported\"]}")
check "add base64 成功" "$(echo "$ADD" | jq -r .data.item.name)" dot
check "add already_existed=false" "$(echo "$ADD" | jq -r .data.already_existed)" false
DOT_ID=$(echo "$ADD" | jq -r .data.item.id)
ADD2=$(post_json "$BASE/api/v1/item/add" "{\"img_base64\":\"$TINY_PNG\",\"name\":\"dot2\"}")
check "同内容 add already_existed=true" "$(echo "$ADD2" | jq -r .data.already_existed)" true
check "同内容共享 item" "$(echo "$ADD2" | jq -r '.data.item.paths | length')" 2
check "add 后 count=4" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 4

# --- folder ---
post_json "$BASE/api/v1/folder/create" '{"name":"图标","parent_path":""}' >/dev/null
check "folder/create 后树包含新目录" "$(curl -s -H "$AUTH" "$BASE/api/v1/folder/list" | jq -r '.data.children | map(.name) | join(",")')" "图标,海报"
check "folder/update 重命名" "$(post_json "$BASE/api/v1/folder/update" '{"path":"图标","name":"icons"}' | jq -r .data.path)" icons
check "重复创建返回 FILE_EXISTS" "$(post_json "$BASE/api/v1/folder/create" '{"name":"海报"}' | jq -r .error.code)" FILE_EXISTS

# --- 文件监听（外部新增文件自动入库）---
python3 - "$LIB" <<'PYEOF'
import struct, zlib, sys, os
def png(w, h, rgb):
    def chunk(t, d):
        c = struct.pack('>I', len(d)) + t + d
        return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    ihdr = struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)
    raw = b''.join(b'\x00' + bytes(rgb) * w for _ in range(h))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', ihdr) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')
open(os.path.join(sys.argv[1], 'watcher.png'), 'wb').write(png(3, 3, (255, 255, 0)))
PYEOF
for _ in $(seq 1 20); do
  N=$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data); [[ "$N" == "5" ]] && break; sleep 0.5
done
check "文件监听自动入库" "$N" 5

# --- 防抖：慢速拷贝中的文件不应以半截内容入库 ---
python3 - "$LIB" "$WORK" <<'PYEOF' &
import struct, zlib, sys, os, time

def png(w, h, rgb):
    def chunk(t, d):
        c = struct.pack('>I', len(d)) + t + d
        return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    ihdr = struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)
    raw = b''.join(b'\x00' + bytes(rgb) * w for _ in range(h))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', ihdr) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')

data = png(5, 5, (10, 20, 200))
open(os.path.join(sys.argv[2], 'slow.size'), 'w').write(str(len(data)))
path = os.path.join(sys.argv[1], 'slow.png')
step = max(1, len(data) // 6)
with open(path, 'wb') as f:
    for i in range(0, len(data), step):
        f.write(data[i:i+step]); f.flush(); time.sleep(0.5)
PYEOF
SLOW_PY=$!
SLOW_SIZE=""
for _ in $(seq 1 40); do
  SLOW=$(post_json "$BASE/api/v1/item/list" '{"keywords":["slow"]}')
  [[ $(echo "$SLOW" | jq -r '.data.items | length') -ge 1 && -f "$WORK/slow.size" ]] && SLOW_SIZE=$(cat "$WORK/slow.size") && break
  sleep 0.5
done
# 写入结束且稳定（1s 防抖窗口）后才会入库
check "慢速拷贝最终入库且尺寸完整" "$(echo "$SLOW" | jq -r '.data.items[0].size // "missing"')" "${SLOW_SIZE:-missing}"
for _ in $(seq 1 20); do
  N=$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data); [[ "$N" == "6" ]] && break; sleep 0.5
done
check "防抖不产生半截 item" "$N" 6
wait $SLOW_PY

# --- SSE 事件 ---
curl -s -N "$BASE/api/v1/events?token=$TOKEN" >"$WORK/sse.log" 2>&1 &
SSE_PID=$!
sleep 0.5
post_json "$BASE/api/v1/item/update" "{\"id\":\"$SUNSET_ID\",\"star\":5}" >/dev/null
sleep 0.5
kill $SSE_PID 2>/dev/null || true
[[ $(grep -c 'event: item.updated' "$WORK/sse.log" || echo 0) -ge 1 ]] && PASS=$((PASS+1)) && echo "ok   - SSE 收到 item.updated" || { FAIL=$((FAIL+1)); echo "FAIL - SSE item.updated 未收到"; }

# --- 回收站 ---
check "item/delete 移入回收站" "$(post_json "$BASE/api/v1/item/delete" "{\"id\":\"$DOT_ID\"}" | jq -r .status)" success
check "回收站视图可见" "$(post_json "$BASE/api/v1/item/list" '{"in_trash":true}' | jq -r .data.total)" 1
# dot 同内容有两条路径（海报/dot.png、dot2.png），回收一份后 item 仍留在库内
check "回收一份后 item 仍在库内" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 6
check "回收站文件落盘" "$(ls "$LIB/.hawk/trash/海报/dot.png" >/dev/null 2>&1 && echo yes)" yes
check "item/restore 恢复" "$(post_json "$BASE/api/v1/item/restore" "{\"id\":\"$DOT_ID\"}" | jq -r .status)" success
check "恢复后文件归位" "$(ls "$LIB/海报/dot.png" >/dev/null 2>&1 && echo yes)" yes
check "恢复后 count=6" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 6

# folder/delete + restore
post_json "$BASE/api/v1/folder/delete" '{"path":"icons"}' >/dev/null
check "folder/delete 后树不含 icons" "$(curl -s -H "$AUTH" "$BASE/api/v1/folder/list" | jq -r '.data.children | map(.name) | join(",")')" "海报"
check "folder/restore 恢复" "$(post_json "$BASE/api/v1/folder/restore" '{"path":"icons"}' | jq -r .status)" success

# trash/clear：删除 sunset 后清空，元数据应被清理
post_json "$BASE/api/v1/item/delete" "{\"id\":\"$SUNSET_ID\"}" >/dev/null
check "trash/clear" "$(post_json "$BASE/api/v1/trash/clear" '{}' | jq -r .status)" success
check "清空后回收站为空" "$(post_json "$BASE/api/v1/item/list" '{"in_trash":true}' | jq -r .data.total)" 0
check "元数据已清理" "$(ls "$LIB/.hawk/metadata/$SUNSET_ID.toml" >/dev/null 2>&1 && echo yes || echo no)" no
check "缩略图已清理" "$(ls "$CACHE/thumbnails/256/$SUNSET_ID.webp" >/dev/null 2>&1 && echo yes || echo no)" no
check "调色板缓存已清理" "$(ls "$CACHE/colors/$SUNSET_ID.json" >/dev/null 2>&1 && echo yes || echo no)" no

# --- 重启验证：哈希复用（mtime 不变不重算）且元数据保持 ---
kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true
HAWK_TOKEN=$TOKEN dotnet bin/Debug/net10.0/hawk-server.dll --library "$LIB" --port $PORT >>"$WORK/server.log" 2>&1 &
PID=$!
for _ in $(seq 1 60); do curl -sf "$BASE/health" >/dev/null 2>&1 && break; sleep 0.5; done
DETAIL2=$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$DOT_ID")
check "重启后 tags 保持" "$(echo "$DETAIL2" | jq -r '.data.tags | join(",")')" "imported"
check "重启后 count 一致" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 5
check "重启后颜色检索仍可用（缓存载入）" "$(post_json "$BASE/api/v1/item/list" '{"color":"#00ff00"}' | jq -r .data.total)" 1

echo
echo "通过 $PASS 项，失败 $FAIL 项"
[[ $FAIL -eq 0 ]]
