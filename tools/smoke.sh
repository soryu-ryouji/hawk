#!/usr/bin/env bash
# hawk-daemon 端到端冒烟：临时素材库 + curl 覆盖本地与局域网 HTTP 流程。
# 前置：hawk-daemon/hawk-daemon(.exe) 已构建（cd hawk-daemon && go build -o hawk-daemon.exe .）
# 用法：bash tools/smoke.sh
#
# Windows Git Bash：JSON POST 体经 stdin 传递，避免原生 curl 把 argv 中文转成 GBK。
set -euo pipefail

cd "$(dirname "$0")/.."
WORK="$PWD/tools/.tmp/smoke"
LIB="$WORK/library"
PORT=27399
LAN_PORT=27398
PEER_PORT=27396
TOKEN="smoke-test-token"
BASE="http://127.0.0.1:$PORT"
AUTH="Authorization: Bearer $TOKEN"
EXE="$PWD/hawk-daemon/hawk-daemon.exe"
[[ -x "$EXE" ]] || EXE="$PWD/hawk-daemon/hawk-daemon"
if [[ ! -x "$EXE" ]]; then
  echo "server 二进制不存在，请先在 hawk-daemon/ 执行 go build -o hawk-daemon.exe ."; exit 2
fi

rm -rf "$WORK"
mkdir -p "$LIB/海报"

python3 - "$LIB" <<'PYEOF'
import struct, zlib, sys, os

def png(w, h, rgb):
    def chunk(t, d):
        c = struct.pack('>I', len(d)) + t + d
        return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    ihdr = struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)
    raw = b''.join(b'\x00' + bytes(rgb) * w for _ in range(h))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', ihdr) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')

def gray_noise(w, h):
    def chunk(t, d):
        c = struct.pack('>I', len(d)) + t + d
        return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    ihdr = struct.pack('>IIBBBBB', w, h, 8, 0, 0, 0, 0)
    raw = b''.join(b'\x00' + os.urandom(w) for _ in range(h))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', ihdr) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')

lib = sys.argv[1]
open(os.path.join(lib, 'sunset.png'), 'wb').write(png(4, 2, (255, 0, 0)))
open(os.path.join(lib, '海报', 'cat.png'), 'wb').write(png(2, 4, (0, 255, 0)))
open(os.path.join(lib, '海报', 'logo.png'), 'wb').write(gray_noise(800, 800))
PYEOF

# 文件写入后 1 秒内 mtime 的扫描会被跳过（防半截文件），冒烟等过防抖窗口。
sleep 1.2

HAWK_ADMIN_TOKEN=$TOKEN "$EXE" local "$LIB" --port $PORT --identity-dir "$WORK/identity" --cache-parent "$WORK/cache" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true; rm -rf "$WORK"' EXIT

wait_health() {
  for _ in $(seq 1 80); do
    curl -sf "$1/health" >/dev/null 2>&1 && return 0
    sleep 0.25
  done
  return 1
}
wait_health "$BASE"

PASS=0; FAIL=0
check() {
  if [[ "$2" == "$3" ]]; then PASS=$((PASS+1)); echo "ok   - $1";
  else FAIL=$((FAIL+1)); echo "FAIL - $1: 期望 [$3] 实际 [$2]"; fi
}
post_json() {
  local url="$1" body="$2"; shift 2
  curl -s -H "$AUTH" -X POST "$url" -H 'Content-Type: application/json' --data-binary @- "$@" <<< "$body"
}
db_query() {
  python3 - "$LIB/.hawk/metadata.db" "$1" <<'PY'
import sqlite3, sys
conn = sqlite3.connect(sys.argv[1])
try:
    row = conn.execute(sys.argv[2]).fetchone()
    conn.commit()
    print("" if row is None else row[0])
finally:
    conn.close()
PY
}

# --- app / 鉴权 ---
check "health 无需 token" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/health")" 200
check "无 token 返回 401" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/app/info")" 401
check "app/info has_library" "$(curl -s -H "$AUTH" "$BASE/api/v1/app/info" | jq -r .data.has_library)" true
check "app/info.channel" "$(curl -s -H "$AUTH" "$BASE/api/v1/app/info" | jq -r .data.channel)" loopback

# --- library / 初始索引 ---
check "library/info.name 缺省为目录名" "$(curl -s -H "$AUTH" "$BASE/api/v1/library/info" | jq -r .data.name)" library
check "reindex 立即返回" "$(post_json "$BASE/api/v1/library/reindex" '{}' | jq -r .status)" success
for _ in $(seq 1 60); do
  COUNT=$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)
  [[ "$COUNT" == "3" ]] && break
  sleep 0.5
done
check "item/count = 3" "$COUNT" 3

LIST=$(post_json "$BASE/api/v1/item/list" '{}')
check "item/list.total = 3" "$(echo "$LIST" | jq -r .data.total)" 3
check "初始宽度识别 (sunset=4)" "$(echo "$LIST" | jq -r '.data.items[] | select(.name=="sunset") | .width')" 4
check "folders 派生" "$(echo "$LIST" | jq -r '.data.items[] | select(.name=="cat") | .folders[0]')" "海报"
SUNSET_ID=$(echo "$LIST" | jq -r '.data.items[] | select(.name=="sunset") | .id')
LOGO_ID=$(echo "$LIST" | jq -r '.data.items[] | select(.name=="logo") | .id')
check "id 为 64 位 hex" "${#SUNSET_ID}" 64

# --- 元数据 / 分类 / 标签 ---
post_json "$BASE/api/v1/item/update" "{\"id\":\"$SUNSET_ID\",\"tags\":[\"nature\",\"sunset\"],\"star\":4,\"annotation\":\"Beautiful sunset\"}" >/dev/null
DETAIL=$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID")
check "update tags 生效" "$(echo "$DETAIL" | jq -r '.data.tags | join(",")')" "nature,sunset"
check "update star 生效" "$(echo "$DETAIL" | jq -r .data.star)" 4
check "按 tags 过滤" "$(post_json "$BASE/api/v1/item/list" '{"tags":["nature"]}' | jq -r .data.total)" 1
check "按 keywords 过滤" "$(post_json "$BASE/api/v1/item/list" '{"keywords":["beautiful"]}' | jq -r .data.total)" 1
check "元数据已写入 SQLite" "$(db_query "SELECT COUNT(*) FROM items WHERE hash='$SUNSET_ID'")" 1

check "创建空分类" "$(post_json "$BASE/api/v1/category/create" '{"name":"灵感"}' | jq -r .status)" success
check "空分类出现在分类列表" "$(curl -s -H "$AUTH" "$BASE/api/v1/category/list" | jq -r '.data[] | select(.count==0) | .name')" "灵感"
post_json "$BASE/api/v1/item/update" "{\"id\":\"$SUNSET_ID\",\"categories\":[\"灵感\",\"参考\"]}" >/dev/null
check "分类赋值生效" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.categories | join(",")')" "灵感,参考"
check "分类过滤 all 语义" "$(post_json "$BASE/api/v1/item/list" '{"categories":["灵感","参考"],"categories_match":"all"}' | jq -r .data.total)" 1
check "排除分类" "$(post_json "$BASE/api/v1/item/list" '{"exclude_categories":["灵感"]}' | jq -r .data.total)" 2
check "分类重命名跟随" "$(post_json "$BASE/api/v1/category/update" '{"name":"灵感","new_name":"灵感库"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.categories[0]')" "灵感库"
check "分类删除清除赋值" "$(post_json "$BASE/api/v1/category/delete" '{"name":"灵感库"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.categories | join(",")')" "参考"
check "标签重命名跟随" "$(post_json "$BASE/api/v1/tag/update" '{"name":"sunset","new_name":"晚霞"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.tags | join(",")')" "nature,晚霞"
check "标签删除同步清除" "$(post_json "$BASE/api/v1/tag/delete" '{"name":"晚霞"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.tags | join(",")')" "nature"

# --- 缩略图 / 调色板 / 颜色检索 ---
for _ in $(seq 1 80); do
  CT=$(curl -s -o /dev/null -w '%{content_type}' -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$LOGO_ID&size=512")
  [[ "$CT" == "image/webp" ]] && break
  sleep 0.25
done
check "thumbnail 生成并返回 webp" "$CT" "image/webp"
check "thumbnail 支持 ?token=" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/item/thumbnail?id=$LOGO_ID&size=512&token=$TOKEN")" 200
check "thumbnail 错误 token 返回 401" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/item/thumbnail?id=$LOGO_ID&token=wrong")" 401
check "thumbnail cache-control" "$(curl -s -D - -o /dev/null -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$LOGO_ID" | grep -i cache-control | tr -d '\r' | tr 'A-Z' 'a-z')" "cache-control: public, max-age=31536000, immutable"

for _ in $(seq 1 40); do
  P=$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.palette | length')
  G=$(post_json "$BASE/api/v1/item/list" '{"color":"#00ff00"}' | jq -r '.data.total')
  [[ "$P" -ge 1 && "$G" == "1" ]] && break
  sleep 0.25
done
check "调色板提炼（纯色图为单色）" "$P" 1
check "cat 调色板就绪（绿色检索命中）" "$G" 1
check "颜色检索异色不命中" "$(post_json "$BASE/api/v1/item/list" '{"color":"#ffff00"}' | jq -r .data.total)" 0
check "非法颜色值返回 400" "$(post_json "$BASE/api/v1/item/list" '{"color":"red"}' -o /dev/null -w '%{http_code}')" 400

# --- add / upload / skip ---
TINY_PNG="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
ADD=$(post_json "$BASE/api/v1/item/add" "{\"img_base64\":\"$TINY_PNG\",\"name\":\"dot\",\"folder_path\":\"海报\",\"tags\":[\"imported\"]}")
check "add base64 成功" "$(echo "$ADD" | jq -r .data.item.name)" dot
check "add already_existed=false" "$(echo "$ADD" | jq -r .data.already_existed)" false
DOT_ID=$(echo "$ADD" | jq -r .data.item.id)
ADD2=$(post_json "$BASE/api/v1/item/add" "{\"img_base64\":\"$TINY_PNG\",\"name\":\"dot2\"}")
check "同内容 add already_existed=true" "$(echo "$ADD2" | jq -r .data.already_existed)" true
check "同内容共享 item" "$(echo "$ADD2" | jq -r '.data.item.paths | length')" 2
SKIP=$(post_json "$BASE/api/v1/item/add" "{\"img_base64\":\"$TINY_PNG\",\"name\":\"dot3\",\"skip_existing\":true}")
check "skip_existing 跳过重复" "$(echo "$SKIP" | jq -r .data.skipped)" true

# --- folder ---
post_json "$BASE/api/v1/folder/create" '{"name":"图标","parent_path":""}' >/dev/null
check "folder/create 后树包含新目录" "$(curl -s -H "$AUTH" "$BASE/api/v1/folder/list" | jq -r '.data.children | map(.name) | join(",")')" "图标,海报"
check "folder/update 重命名" "$(post_json "$BASE/api/v1/folder/update" '{"path":"图标","name":"icons"}' | jq -r .data.path)" icons
check "重复创建返回 FILE_EXISTS" "$(post_json "$BASE/api/v1/folder/create" '{"name":"海报"}' | jq -r .error.code)" FILE_EXISTS

# --- 文件监听 ---
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
for _ in $(seq 1 40); do
  N=$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data); [[ "$N" == "5" ]] && break; sleep 0.5
done
check "文件监听自动入库" "$N" 5

# --- SSE ---
curl -s -N "$BASE/api/v1/events?token=$TOKEN" >"$WORK/sse.log" 2>&1 &
SSE_PID=$!
sleep 0.5
post_json "$BASE/api/v1/item/update" "{\"id\":\"$SUNSET_ID\",\"star\":5}" >/dev/null
post_json "$BASE/api/v1/folder/create" '{"name":"sse-dir","parent_path":""}' >/dev/null
sleep 0.6
kill $SSE_PID 2>/dev/null || true
[[ $(grep -c 'event: item.updated' "$WORK/sse.log" || echo 0) -ge 1 ]] && PASS=$((PASS+1)) && echo "ok   - SSE 收到 item.updated" || { FAIL=$((FAIL+1)); echo "FAIL - SSE item.updated 未收到"; }
[[ $(grep -c 'event: folder.changed' "$WORK/sse.log" || echo 0) -ge 1 ]] && PASS=$((PASS+1)) && echo "ok   - SSE 收到 folder.changed" || { FAIL=$((FAIL+1)); echo "FAIL - SSE folder.changed 未收到"; }

# --- 批量更新 / 回收站 ---
CAT_ID=$(echo "$LIST" | jq -r '.data.items[] | select(.name=="cat") | .id')
BATCH=$(post_json "$BASE/api/v1/item/batch_update" "{\"ids\":[\"$SUNSET_ID\",\"$CAT_ID\"],\"add_tags\":[\"批量\"],\"star\":2}")
check "batch_update updated=2" "$(echo "$BATCH" | jq -r .data.updated)" 2
check "batch_update 标签并集追加" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.tags | join(",")')" "nature,批量"
check "batch_update 无更新字段返回 400" "$(post_json "$BASE/api/v1/item/batch_update" "{\"ids\":[\"$CAT_ID\"]}" -o /dev/null -w '%{http_code}')" 400

check "带 path 删除单位置" "$(post_json "$BASE/api/v1/item/delete" "{\"id\":\"$DOT_ID\",\"path\":\"海报/dot.png\"}" | jq -r .status)" success
check "单位置删除后 paths 只剩一条" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$DOT_ID" | jq -r '.data.paths | length')" 1
check "带 path 恢复单位置" "$(post_json "$BASE/api/v1/item/restore" "{\"id\":\"$DOT_ID\",\"path\":\"海报/dot.png\"}" | jq -r .status)" success
check "卡片级删除回收全部路径" "$(post_json "$BASE/api/v1/item/delete" "{\"id\":\"$DOT_ID\"}" | jq -r .status)" success
check "回收站视图可见" "$(post_json "$BASE/api/v1/item/list" '{"in_trash":true}' | jq -r .data.total)" 2
check "item/restore 恢复" "$(post_json "$BASE/api/v1/item/restore" "{\"id\":\"$DOT_ID\"}" | jq -r .status)" success
check "恢复后 count=5" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 5

# --- 局域网密钥 / Web Session / Peer ---
KEY=$(post_json "$BASE/api/v1/app/lan/device-keys" '{"label":"smoke","role":"editor"}')
KEY_SECRET=$(echo "$KEY" | jq -r .data.access_key)
check "创建局域网访问密钥" "${#KEY_SECRET}" 67
check "启用局域网监听" "$(curl -s -H "$AUTH" -X PUT "$BASE/api/v1/app/lan" -H 'Content-Type: application/json' -d "{\"enabled\":true,\"port\":$LAN_PORT,\"peer_port\":$PEER_PORT}" | jq -r .data.active)" true
wait_health "http://127.0.0.1:$PEER_PORT"
check "peer 端口 Bearer 鉴权" "$(curl -s -H "Authorization: Bearer $KEY_SECRET" "http://127.0.0.1:$PEER_PORT/api/v1/app/info" | jq -r .data.channel)" lan_peer
COOKIE_JAR="$WORK/cookies.txt"
curl -s -c "$COOKIE_JAR" -X POST "http://127.0.0.1:$LAN_PORT/api/v1/app/session" -H 'Content-Type: application/json' -d "{\"access_key\":\"$KEY_SECRET\"}" >/dev/null
check "Web Session Cookie 读列表" "$(curl -s -b "$COOKIE_JAR" -X POST "http://127.0.0.1:$LAN_PORT/api/v1/item/list" -H 'Content-Type: application/json' -H 'X-Hawk-Client: web' -H "Origin: http://127.0.0.1:$LAN_PORT" -d '{}' | jq -r .data.total)" 6

# --- 重启：SQLite 注水与元数据保持 ---
kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true
HAWK_ADMIN_TOKEN=$TOKEN "$EXE" local "$LIB" --port $PORT --identity-dir "$WORK/identity" --cache-parent "$WORK/cache" >>"$WORK/server.log" 2>&1 &
PID=$!
wait_health "$BASE"
check "重启后 tags 保持" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$DOT_ID" | jq -r '.data.tags | join(",")')" "imported"
check "重启后 count 一致" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 5
check "重启后颜色检索仍可用" "$(post_json "$BASE/api/v1/item/list" '{"color":"#00ff00"}' | jq -r .data.total)" 1

echo
echo "通过 $PASS 项，失败 $FAIL 项"
[[ $FAIL -eq 0 ]]
