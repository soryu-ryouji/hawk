#!/usr/bin/env bash
# 端到端验证：空白库首扫 —— 流式入库、items.added 批量事件、缩略图/调色板随入库就绪
# 一次性脚本，用完即删（hawk/tools/tmp-e2e/）
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$ROOT/tools/tmp-e2e"
LIB="$TMP/library"
PORT=39771
TOKEN=e2e-token
BIN="$ROOT/hawk-daemon/target/debug/hawk-daemon.exe"

rm -rf "$TMP"
mkdir -p "$LIB"

# 生成 40 张测试图
python - "$LIB" <<'PY'
import os, random, sys
from PIL import Image
root = sys.argv[1]
random.seed()
for i in range(40):
    w, h = random.choice([(800, 600), (1200, 900), (640, 480), (2000, 1500)])
    Image.new("RGB", (w, h), tuple(random.randrange(256) for _ in range(3))).save(os.path.join(root, f"img_{i:03d}.png"))
PY

# 文件落稳（mtime 距今 >1s，跳过写入防抖），且隔离库外缓存（LOCALAPPDATA 指向临时目录）
sleep 1.5
mkdir -p "$TMP/localappdata"

HAWK_TOKEN=$TOKEN LOCALAPPDATA="$TMP/localappdata" "$BIN" --library "$LIB" --port $PORT > "$TMP/server.log" 2>&1 &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null; rm -rf "$TMP"' EXIT

# 等就绪（/health 无需 token：初始索引完成前 503）
for i in $(seq 1 50); do
  code=$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:$PORT/health" || true)
  [ "$code" = "200" ] && break
  sleep 0.2
done
[ "$code" = "200" ] || { echo "FAIL: server not ready"; cat "$TMP/server.log"; exit 1; }

# 订阅 SSE，后台落盘（验证 items.added / items.updated 事件）
(curl -sN "http://127.0.0.1:$PORT/api/v1/events?token=$TOKEN" > "$TMP/events.log") &
CURL_PID=$!

# 等扫描完成（index 积压清零；Envelope 为 {status, data}）
for i in $(seq 1 100); do
  pending=$(curl -s -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:$PORT/api/v1/app/status" | python -c "import json,sys; d=json.load(sys.stdin)['data']['index']; print(d['pending'] + d['active'])" 2>/dev/null || echo -1)
  [ "$pending" = "0" ] && break
  sleep 0.3
done
[ "$pending" = "0" ] || { echo "FAIL: scan did not finish (pending=$pending)"; cat "$TMP/server.log"; exit 1; }

ITEMS=$(curl -s -H "Authorization: Bearer $TOKEN" -X POST "http://127.0.0.1:$PORT/api/v1/item/list" -H "Content-Type: application/json" -d '{"limit":100}')
TOTAL=$(echo "$ITEMS" | python -c "import json,sys; print(len(json.load(sys.stdin)['data']['items']))")
echo "indexed items: $TOTAL (expect 40)"
[ "$TOTAL" = "40" ] || { echo "FAIL: item count"; exit 1; }

# 调色板/宽高全部就绪（首扫单解码通道产出）
echo "$ITEMS" | python -c "
import json, sys
items = json.load(sys.stdin)['data']['items']
no_palette = [i['id'][:8] for i in items if not i.get('palette')]
no_dim = [i['id'][:8] for i in items if not i.get('width')]
assert not no_palette, f'missing palette: {no_palette}'
assert not no_dim, f'missing dim: {no_dim}'
print('palette + dim: all 40 items ready')
"

# 缩略图全部命中（非回源原图）：HTTP 200 + image/webp
echo "$ITEMS" | python -c "
import json, sys
for i in json.load(sys.stdin)['data']['items']:
    print(i['id'])
" > "$TMP/ids.txt"
tr -d '\r' < "$TMP/ids.txt" > "$TMP/ids.clean" && mv "$TMP/ids.clean" "$TMP/ids.txt"
while read -r id; do
  ct=$(curl -s -o /dev/null -w "%{content_type}" -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:$PORT/api/v1/item/thumbnail?id=$id&size=256")
  [ "$ct" = "image/webp" ] || { echo "FAIL: thumbnail not webp for ${id:0:8} (ct=$ct)"; exit 1; }
done < "$TMP/ids.txt"
echo "thumbnails: all 40 hit webp cache"

sleep 1
kill $CURL_PID 2>/dev/null
echo "--- event stats ---"
grep -c "event: item.added" "$TMP/events.log" | xargs -I{} echo "item.added events: {} (expect 0)"
grep -c "event: items.added" "$TMP/events.log" | xargs -I{} echo "items.added events: {} (expect >= 1)"
grep -q "event: items.added" "$TMP/events.log" || { echo "FAIL: no items.added event"; grep "^event:" "$TMP/events.log" | sort | uniq -c; exit 1; }

echo "E2E OK"
