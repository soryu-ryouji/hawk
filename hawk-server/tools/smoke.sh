#!/usr/bin/env bash
# hawk-server 端到端冒烟测试：临时素材库 + curl 覆盖主要 API 流程。
# 用法: tools/smoke.sh（需先 dotnet build）
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

# --- app / 鉴权 ---
check "health 无需 token" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/health")" 200
check "无 token 返回 401" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/app/info")" 401
check "app/info.platform" "$(curl -s -H "$AUTH" "$BASE/api/v1/app/info" | jq -r .data.platform)" macos

# --- library ---
check "library/info.name 缺省为目录名" "$(curl -s -H "$AUTH" "$BASE/api/v1/library/info" | jq -r .data.name)" library
check "reindex 立即返回" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/library/reindex" | jq -r .status)" success

# --- 初始索引 ---
sleep 1 # 等 reindex 流水线跑完
check "item/count = 3" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 3
LIST=$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/list" -H 'Content-Type: application/json' -d '{}')
check "item/list.total = 3" "$(echo "$LIST" | jq -r .data.total)" 3
check "初始宽度识别 (sunset=4)" "$(echo "$LIST" | jq -r '.data.items[] | select(.name=="sunset") | .width')" 4
check "folders 派生" "$(echo "$LIST" | jq -r '.data.items[] | select(.name=="cat") | .folders[0]')" "海报"

SUNSET_ID=$(echo "$LIST" | jq -r '.data.items[] | select(.name=="sunset") | .id')
check "id 为 64 位 hex" "${#SUNSET_ID}" 64

# --- 元数据读写 ---
curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/update" -H 'Content-Type: application/json' \
  -d "{\"id\":\"$SUNSET_ID\",\"tags\":[\"nature\",\"sunset\"],\"star\":4,\"annotation\":\"Beautiful sunset\"}" >/dev/null
DETAIL=$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID")
check "update tags 生效" "$(echo "$DETAIL" | jq -r '.data.tags | join(",")')" "nature,sunset"
check "update star 生效" "$(echo "$DETAIL" | jq -r '.data.star')" 4
check "按 tags 过滤" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/list" -H 'Content-Type: application/json' -d '{"tags":["nature"]}' | jq -r .data.total)" 1
check "按 keywords 过滤" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/list" -H 'Content-Type: application/json' -d '{"keywords":["beautiful"]}' | jq -r .data.total)" 1
check "元数据文件已落盘" "$(ls "$LIB/.hawk/metadata/$SUNSET_ID.toml" >/dev/null 2>&1 && echo yes)" yes

# --- 缩略图 ---
for _ in $(seq 1 20); do
  curl -sf -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID&size=256" -o "$WORK/t.webp" && break
  sleep 0.5
done
check "thumbnail content-type" "$(curl -s -o /dev/null -w '%{content_type}' -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID&size=256")" "image/webp"
check "thumbnail cache-control" "$(curl -s -D - -o /dev/null -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID" | grep -i cache-control | tr -d '\r' | tr 'A-Z' 'a-z')" "cache-control: public, max-age=31536000, immutable"
check "thumbnail 不可缓存尺寸 400" "$(curl -s -o /dev/null -w '%{http_code}' -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID&size=400")" 400
check "refresh_thumbnail" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/refresh_thumbnail" -H 'Content-Type: application/json' -d "{\"id\":\"$SUNSET_ID\"}" | jq -r .status)" success

# --- item/add ---
TINY_PNG="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
ADD=$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/add" -H 'Content-Type: application/json' \
  -d "{\"img_base64\":\"$TINY_PNG\",\"name\":\"dot\",\"folder_path\":\"海报\",\"tags\":[\"imported\"]}")
check "add base64 成功" "$(echo "$ADD" | jq -r .data.item.name)" dot
check "add already_existed=false" "$(echo "$ADD" | jq -r .data.already_existed)" false
DOT_ID=$(echo "$ADD" | jq -r .data.item.id)
ADD2=$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/add" -H 'Content-Type: application/json' \
  -d "{\"img_base64\":\"$TINY_PNG\",\"name\":\"dot2\"}")
check "同内容 add already_existed=true" "$(echo "$ADD2" | jq -r .data.already_existed)" true
check "同内容共享 item" "$(echo "$ADD2" | jq -r '.data.item.paths | length')" 2
check "add 后 count=4" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 4

# --- folder ---
curl -s -H "$AUTH" -X POST "$BASE/api/v1/folder/create" -H 'Content-Type: application/json' -d '{"name":"图标","parent_path":""}' >/dev/null
check "folder/create 后树包含新目录" "$(curl -s -H "$AUTH" "$BASE/api/v1/folder/list" | jq -r '.data.children | map(.name) | join(",")')" "图标,海报"
check "folder/update 重命名" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/folder/update" -H 'Content-Type: application/json' -d '{"path":"图标","name":"icons"}' | jq -r .data.path)" icons
check "重复创建返回 FILE_EXISTS" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/folder/create" -H 'Content-Type: application/json' -d '{"name":"海报"}' | jq -r .error.code)" FILE_EXISTS

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
  SLOW=$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/list" -H 'Content-Type: application/json' -d '{"keywords":["slow"]}')
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
curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/update" -H 'Content-Type: application/json' \
  -d "{\"id\":\"$SUNSET_ID\",\"star\":5}" >/dev/null
sleep 0.5
kill $SSE_PID 2>/dev/null || true
[[ $(grep -c 'event: item.updated' "$WORK/sse.log" || echo 0) -ge 1 ]] && PASS=$((PASS+1)) && echo "ok   - SSE 收到 item.updated" || { FAIL=$((FAIL+1)); echo "FAIL - SSE item.updated 未收到"; }

# --- 回收站 ---
check "item/delete 移入回收站" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/delete" -H 'Content-Type: application/json' -d "{\"id\":\"$DOT_ID\"}" | jq -r .status)" success
check "回收站视图可见" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/list" -H 'Content-Type: application/json' -d '{"in_trash":true}' | jq -r .data.total)" 1
# dot 同内容有两条路径（海报/dot.png、dot2.png），回收一份后 item 仍留在库内
check "回收一份后 item 仍在库内" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 6
check "回收站文件落盘" "$(ls "$LIB/.hawk/trash/海报/dot.png" >/dev/null 2>&1 && echo yes)" yes
check "item/restore 恢复" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/restore" -H 'Content-Type: application/json' -d "{\"id\":\"$DOT_ID\"}" | jq -r .status)" success
check "恢复后文件归位" "$(ls "$LIB/海报/dot.png" >/dev/null 2>&1 && echo yes)" yes
check "恢复后 count=6" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 6

# folder/delete + restore
curl -s -H "$AUTH" -X POST "$BASE/api/v1/folder/delete" -H 'Content-Type: application/json' -d '{"path":"icons"}' >/dev/null
check "folder/delete 后树不含 icons" "$(curl -s -H "$AUTH" "$BASE/api/v1/folder/list" | jq -r '.data.children | map(.name) | join(",")')" "海报"
check "folder/restore 恢复" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/folder/restore" -H 'Content-Type: application/json' -d '{"path":"icons"}' | jq -r .status)" success

# trash/clear：删除 sunset 后清空，元数据应被清理
curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/delete" -H 'Content-Type: application/json' -d "{\"id\":\"$SUNSET_ID\"}" >/dev/null
check "trash/clear" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/trash/clear" | jq -r .status)" success
check "清空后回收站为空" "$(curl -s -H "$AUTH" -X POST "$BASE/api/v1/item/list" -H 'Content-Type: application/json' -d '{"in_trash":true}' | jq -r .data.total)" 0
check "元数据已清理" "$(ls "$LIB/.hawk/metadata/$SUNSET_ID.toml" >/dev/null 2>&1 && echo yes || echo no)" no
check "缩略图已清理" "$(ls "$LIB/.hawk/thumbnails/256/${SUNSET_ID:0:2}/$SUNSET_ID.webp" >/dev/null 2>&1 && echo yes || echo no)" no

# --- 重启验证：哈希复用（mtime 不变不重算）且元数据保持 ---
kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true
HAWK_TOKEN=$TOKEN dotnet bin/Debug/net10.0/hawk-server.dll --library "$LIB" --port $PORT >>"$WORK/server.log" 2>&1 &
PID=$!
for _ in $(seq 1 60); do curl -sf "$BASE/health" >/dev/null 2>&1 && break; sleep 0.5; done
DETAIL2=$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$DOT_ID")
check "重启后 tags 保持" "$(echo "$DETAIL2" | jq -r '.data.tags | join(",")')" "imported"
check "重启后 count 一致" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 5

echo
echo "通过 $PASS 项，失败 $FAIL 项"
[[ $FAIL -eq 0 ]]
