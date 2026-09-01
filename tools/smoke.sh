#!/usr/bin/env bash
# hawk-daemon 端到端冒烟测试：临时素材库 + curl 覆盖主要 API 流程（契约测试）。
# 用法: tools/smoke.sh（需先 cargo build --release）
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
SERVER=("$PWD/hawk-daemon/target/release/hawk-daemon.exe")
if [[ ! -x "${SERVER[0]}" ]]; then
  echo "server 二进制不存在，请先 cargo build --release: ${SERVER[0]}"; exit 2
fi

rm -rf "$WORK"
mkdir -p "$LIB/海报"

# 生成三张不同内容的 PNG(4x2 / 2x4 / 8x8)
python3 - "$LIB" <<'PYEOF'
import struct, zlib, sys, os

def png(w, h, rgb):
    def chunk(t, d):
        c = struct.pack('>I', len(d)) + t + d
        return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    ihdr = struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)
    raw = b''.join(b'\x00' + bytes(rgb) * w for _ in range(h))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', ihdr) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')

def png_gray_noise(w, h):
    """灰度随机噪声：体积 >500KB 的大图（缩略图惰性生成测试对象），调色板为灰色系，不干扰颜色检索断言"""
    def chunk(t, d):
        c = struct.pack('>I', len(d)) + t + d
        return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    ihdr = struct.pack('>IIBBBBB', w, h, 8, 0, 0, 0, 0)  # 8-bit 灰度
    raw = b''.join(b'\x00' + os.urandom(w) for _ in range(h))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', ihdr) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')

lib = sys.argv[1]
open(os.path.join(lib, 'sunset.png'), 'wb').write(png(4, 2, (255, 0, 0)))
open(os.path.join(lib, '海报', 'cat.png'), 'wb').write(png(2, 4, (0, 255, 0)))
# 大图：缩略图惰性生成的测试对象（未命中回源原图 + 后台生成）
open(os.path.join(lib, '海报', 'logo.png'), 'wb').write(png_gray_noise(800, 800))
PYEOF

HAWK_TOKEN=$TOKEN "${SERVER[@]}" --library "$LIB" --port $PORT >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true; rm -rf "$WORK"' EXIT

for _ in $(seq 1 60); do
  curl -sf "$BASE/health" >/dev/null 2>&1 && break
  sleep 0.5
done

# 库外派生缓存目录(随平台:Windows 为 %LOCALAPPDATA%\hawk\cache,见 LibraryPaths 构造)。
# 缓存子目录 = <库文件夹名>_<库根路径 SHA-256 前16位>。
# 库标识以 server 报告的库根路径计算:server 看到的 argv 路径经 MSYS 转换,与 shell 变量可能不同,
# 而 SHA-256 对路径字符串逐字节敏感
LIB_ABS=$(curl -s -H "$AUTH" "$BASE/api/v1/library/info" | jq -r .data.path)
CACHE_KEY=$(printf '%s' "$LIB_ABS" | sha256sum | cut -c1-16)
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) CACHE_PARENT="$(cygpath -u "$LOCALAPPDATA")/hawk/cache" ;;
  Darwin*) CACHE_PARENT="$HOME/Library/Application Support/hawk/cache" ;;
  *) CACHE_PARENT="${XDG_DATA_HOME:-$HOME/.local/share}/hawk/cache" ;;
esac
CACHE="$CACHE_PARENT/library_$CACHE_KEY"

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
LOGO_ID=$(echo "$LIST" | jq -r '.data.items[] | select(.name=="logo") | .id')

# --- 元数据读写 ---
post_json "$BASE/api/v1/item/update" "{\"id\":\"$SUNSET_ID\",\"tags\":[\"nature\",\"sunset\"],\"star\":4,\"annotation\":\"Beautiful sunset\"}" >/dev/null
DETAIL=$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID")
check "update tags 生效" "$(echo "$DETAIL" | jq -r '.data.tags | join(",")')" "nature,sunset"
check "update star 生效" "$(echo "$DETAIL" | jq -r '.data.star')" 4
check "按 tags 过滤" "$(post_json "$BASE/api/v1/item/list" '{"tags":["nature"]}' | jq -r .data.total)" 1
check "按 keywords 过滤" "$(post_json "$BASE/api/v1/item/list" '{"keywords":["beautiful"]}' | jq -r .data.total)" 1
check "元数据文件已落盘" "$(ls "$LIB/.hawk/metadata/$SUNSET_ID.toml" >/dev/null 2>&1 && echo yes)" yes

# --- 分类(Category,扁平名字) ---
CAT_API="$BASE/api/v1/category"
check "创建空分类" "$(post_json "$CAT_API/create" '{"name":"灵感"}' | jq -r .status)" success
check "重复创建分类返回 CATEGORY_EXISTS" "$(post_json "$CAT_API/create" '{"name":"灵感"}' | jq -r .error.code)" CATEGORY_EXISTS
check "空分类出现在分类列表" "$(curl -s -H "$AUTH" "$CAT_API/list" | jq -r '.data[] | select(.count==0) | .name')" "灵感"
post_json "$BASE/api/v1/item/update" "{\"id\":\"$SUNSET_ID\",\"categories\":[\"灵感\",\"参考\"]}" >/dev/null
check "item 分类赋值生效" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.categories | join(",")')" "灵感,参考"
check "按分类过滤" "$(post_json "$BASE/api/v1/item/list" '{"categories":["灵感"]}' | jq -r .data.total)" 1
check "分类过滤 all 语义" "$(post_json "$BASE/api/v1/item/list" '{"categories":["灵感","参考"],"categories_match":"all"}' | jq -r .data.total)" 1
check "排除分类" "$(post_json "$BASE/api/v1/item/list" '{"exclude_categories":["灵感"]}' | jq -r .data.total)" 2
check "分类重命名跟随" "$(post_json "$CAT_API/update" '{"name":"灵感","new_name":"灵感库"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.categories[0]')" "灵感库"
check "分类删除清除赋值" "$(post_json "$CAT_API/delete" '{"name":"灵感库"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.categories | join(",")')" "参考"
check "注册表文件已落盘" "$(ls "$LIB/.hawk/categories.toml" >/dev/null 2>&1 && echo yes)" yes

# --- 标签注册表（Tag） ---
TAG_API="$BASE/api/v1/tag"
check "创建空标签" "$(post_json "$TAG_API/create" '{"name":"待审核"}' | jq -r .status)" success
check "空标签出现在标签列表" "$(curl -s -H "$AUTH" "$TAG_API/list" | jq -r '.data[] | select(.count==0) | .name')" "待审核"
check "标签重命名跟随 item" "$(post_json "$TAG_API/update" '{"name":"sunset","new_name":"晚霞"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.tags | join(",")')" "nature,晚霞"
check "排除标签过滤" "$(post_json "$BASE/api/v1/item/list" '{"exclude_tags":["nature"]}' | jq -r .data.total)" 2
check "标签删除同步清除" "$(post_json "$TAG_API/delete" '{"name":"晚霞"}' >/dev/null; curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.tags | join(",")')" "nature"

# --- 缩略图（惰性生成：未命中回源原图并后台入队，大小图同逻辑） ---
# 删缓存制造「未命中」前提：reindex 全量导入通道会同步生成缩略图（item 入库即完整可显示），
# 不删的话断言与后台生成存在时序竞态（flaky）
rm -f "$CACHE/thumbnails/256/$LOGO_ID.webp" "$CACHE/thumbnails/512/$LOGO_ID.webp" "$CACHE/thumbnails/1024/$LOGO_ID.webp"
# 大图未命中 → 直接回源原图（200 + image/png），同时后台入队生成
LOGO_CT=$(curl -s -o /dev/null -w '%{content_type}' -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$LOGO_ID&size=256")
check "thumbnail 未命中先回源原图" "$LOGO_CT" "image/png"
# 轮询至后台生成完毕（webp 缓存就绪）
for _ in $(seq 1 40); do
  LOGO_CT=$(curl -s -o /dev/null -w '%{content_type}' -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$LOGO_ID&size=256")
  [ "$LOGO_CT" = "image/webp" ] && break
  sleep 0.5
done
check "thumbnail 后台生成后命中缓存" "$LOGO_CT" "image/webp"
check "thumbnail 支持 ?token=（<img> 场景）" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/item/thumbnail?id=$LOGO_ID&size=256&token=$TOKEN")" 200
check "thumbnail 错误 token 返回 401" "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/item/thumbnail?id=$LOGO_ID&token=wrong")" 401
check "thumbnail cache-control" "$(curl -s -D - -o /dev/null -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$LOGO_ID" | grep -i cache-control | tr -d '\r' | tr 'A-Z' 'a-z')" "cache-control: public, max-age=31536000, immutable"
check "refresh_thumbnail" "$(post_json "$BASE/api/v1/item/refresh_thumbnail" "{\"id\":\"$LOGO_ID\"}" | jq -r .status)" success
# 小图与大图同逻辑：删缓存后回源原图，后台重建（同上：删缓存消除时序竞态）
rm -f "$CACHE/thumbnails/256/$SUNSET_ID.webp" "$CACHE/thumbnails/512/$SUNSET_ID.webp" "$CACHE/thumbnails/1024/$SUNSET_ID.webp"
SUNSET_CT=$(curl -s -o /dev/null -w '%{content_type}' -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID&size=256")
check "小图未命中回源原图" "$SUNSET_CT" "image/png"
for _ in $(seq 1 40); do
  SUNSET_CT=$(curl -s -o /dev/null -w '%{content_type}' -H "$AUTH" "$BASE/api/v1/item/thumbnail?id=$SUNSET_ID&size=256")
  [ "$SUNSET_CT" = "image/webp" ] && break
  sleep 0.5
done
check "小图同样后台生成缓存" "$SUNSET_CT" "image/webp"

# --- 调色板与颜色检索 ---
for _ in $(seq 1 20); do
  P=$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.palette | length')
  G=$(post_json "$BASE/api/v1/item/list" '{"color":"#00ff00"}' | jq -r '.data.total')
  [[ "$P" -ge 1 && "$G" == "1" ]] && break
  sleep 0.5
done
check "调色板提炼（纯色图为单色）" "$P" 1
check "cat 调色板就绪（绿色检索命中）" "$G" 1
# 主色允许 ±8 的编解码噪声（有损 WebP 往返：libwebp 对饱和纯色可能 ±1，ImageSharp 恰好往返精确）
check "主色为红色（容差±8）" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | python3 -c '
import json, sys
c = json.load(sys.stdin)["data"]["palette"][0]["color"]
r, g, b = int(c[1:3], 16), int(c[3:5], 16), int(c[5:7], 16)
print("true" if r >= 250 and g <= 8 and b <= 8 else "false")')" true
check "主色占比 100" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.palette[0].percentage * 10 | round / 10')" 100
check "颜色检索命中同色" "$(post_json "$BASE/api/v1/item/list" '{"color":"#ff0000"}' | jq -r .data.total)" 1
check "颜色检索相近色命中" "$(post_json "$BASE/api/v1/item/list" '{"color":"#ee0000"}' | jq -r .data.total)" 1
check "颜色检索异色不命中" "$(post_json "$BASE/api/v1/item/list" '{"color":"#ffff00"}' | jq -r .data.total)" 0
check "颜色检索限定文件夹范围" "$(post_json "$BASE/api/v1/item/list" '{"color":"#00ff00","folders":["海报"]}' | jq -r .data.total)" 1
check "颜色检索范围外不命中" "$(post_json "$BASE/api/v1/item/list" '{"color":"#ff0000","folders":["海报"]}' | jq -r .data.total)" 0
check "非法颜色值返回 400" "$(post_json "$BASE/api/v1/item/list" '{"color":"red"}' -o /dev/null -w '%{http_code}')" 400
check "调色板已入元数据 TOML" "$(grep -q '^\[\[palette\]\]' "$LIB/.hawk/metadata/$SUNSET_ID.toml" 2>/dev/null && echo yes || echo no)" yes

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
post_json "$BASE/api/v1/folder/create" '{"name":"sse-dir","parent_path":""}' >/dev/null
sleep 0.5
kill $SSE_PID 2>/dev/null || true
[[ $(grep -c 'event: item.updated' "$WORK/sse.log" || echo 0) -ge 1 ]] && PASS=$((PASS+1)) && echo "ok   - SSE 收到 item.updated" || { FAIL=$((FAIL+1)); echo "FAIL - SSE item.updated 未收到"; }
[[ $(grep -c 'event: folder.changed' "$WORK/sse.log" || echo 0) -ge 1 ]] && PASS=$((PASS+1)) && echo "ok   - SSE 收到 folder.changed" || { FAIL=$((FAIL+1)); echo "FAIL - SSE folder.changed 未收到"; }

# --- 批量更新(batch_update) ---
CAT_ID=$(echo "$LIST" | jq -r '.data.items[] | select(.name=="cat") | .id')
BATCH=$(post_json "$BASE/api/v1/item/batch_update" "{\"ids\":[\"$SUNSET_ID\",\"$CAT_ID\"],\"add_tags\":[\"批量\"],\"star\":2}")
check "batch_update updated=2" "$(echo "$BATCH" | jq -r .data.updated)" 2
check "batch_update 标签并集追加" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.tags | join(",")')" "nature,批量"
check "batch_update 评分生效" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$CAT_ID" | jq -r .data.star)" 2
ZERO_ID="0000000000000000000000000000000000000000000000000000000000000000"
BATCH2=$(post_json "$BASE/api/v1/item/batch_update" "{\"ids\":[\"$CAT_ID\",\"$ZERO_ID\"],\"add_tags\":[\"x\"]}")
check "batch_update missing_ids 报告不存在的 id" "$(echo "$BATCH2" | jq -r '.data.missing_ids | length')" 1
check "batch_update 无更新字段返回 400" "$(post_json "$BASE/api/v1/item/batch_update" "{\"ids\":[\"$CAT_ID\"]}" -o /dev/null -w '%{http_code}')" 400

# --- 回收站 ---
check "item/delete 移入回收站" "$(post_json "$BASE/api/v1/item/delete" "{\"id\":\"$DOT_ID\"}" | jq -r .status)" success
check "回收站视图可见" "$(post_json "$BASE/api/v1/item/list" '{"in_trash":true}' | jq -r .data.total)" 1
# dot 同内容有两条路径（海报/dot.png、dot2.png）：卡片级删除（无 path）应全部回收，
# 否则卡片残留在网格（用户感知为「删除不生效」）
check "卡片级删除回收全部路径（count 6→5）" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 5
check "回收站文件落盘（路径 1）" "$(ls "$LIB/.hawk/trash/海报/dot.png" >/dev/null 2>&1 && echo yes)" yes
check "回收站文件落盘（路径 2）" "$(ls "$LIB/.hawk/trash/dot2.png" >/dev/null 2>&1 && echo yes)" yes
check "item/restore 恢复" "$(post_json "$BASE/api/v1/item/restore" "{\"id\":\"$DOT_ID\"}" | jq -r .status)" success
check "恢复后文件归位（路径 1）" "$(ls "$LIB/海报/dot.png" >/dev/null 2>&1 && echo yes)" yes
check "恢复后文件归位（路径 2）" "$(ls "$LIB/dot2.png" >/dev/null 2>&1 && echo yes)" yes
check "恢复后 count=6" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 6

# folder/delete + restore
post_json "$BASE/api/v1/folder/delete" '{"path":"icons"}' >/dev/null
check "folder/delete 后树不含 icons" "$(curl -s -H "$AUTH" "$BASE/api/v1/folder/list" | jq -r '.data.children | map(.name) | index("icons") == null')" true
check "folder/restore 恢复" "$(post_json "$BASE/api/v1/folder/restore" '{"path":"icons"}' | jq -r .status)" success

# trash/clear：删除 logo（有缩略图缓存）后清空，元数据与派生缓存应被清理
post_json "$BASE/api/v1/item/delete" "{\"id\":\"$LOGO_ID\"}" >/dev/null
check "trash/clear" "$(post_json "$BASE/api/v1/trash/clear" '{}' | jq -r .status)" success
check "清空后回收站为空" "$(post_json "$BASE/api/v1/item/list" '{"in_trash":true}' | jq -r .data.total)" 0
check "元数据已清理" "$(ls "$LIB/.hawk/metadata/$LOGO_ID.toml" >/dev/null 2>&1 && echo yes || echo no)" no
check "缩略图已清理" "$(ls "$CACHE/thumbnails/256/$LOGO_ID.webp" >/dev/null 2>&1 && echo yes || echo no)" no

# --- 重启验证：哈希复用（mtime 不变不重算）且元数据保持 ---
kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true
HAWK_TOKEN=$TOKEN "${SERVER[@]}" --library "$LIB" --port $PORT >>"$WORK/server.log" 2>&1 &
PID=$!
for _ in $(seq 1 60); do curl -sf "$BASE/health" >/dev/null 2>&1 && break; sleep 0.5; done
DETAIL2=$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$DOT_ID")
check "重启后 tags 保持" "$(echo "$DETAIL2" | jq -r '.data.tags | join(",")')" "imported"
check "重启后 count 一致" "$(curl -s -H "$AUTH" "$BASE/api/v1/item/count" | jq -r .data)" 5
check "重启后颜色检索仍可用（缓存载入）" "$(post_json "$BASE/api/v1/item/list" '{"color":"#00ff00"}' | jq -r .data.total)" 1

# --- refresh_cache：按范围补全派生缓存（补缺失模式，不重建已有文件）---
wait_thumbs_idle() { # 轮询 thumbnail 队列空闲，避免 in-flight 去重干扰 dispatched 断言
  for _ in $(seq 1 60); do
    B=$(curl -s -H "$AUTH" "$BASE/api/v1/app/status" | jq -r '.data.thumbnail | "\(.pending),\(.active)"')
    [[ "$B" == "0,0" ]] && return 0
    sleep 0.5
  done
  return 1
}
check "refresh_cache 文件夹范围派发" "$(post_json "$BASE/api/v1/library/refresh_cache" '{"type":"folder","value":"海报"}' | jq -r .data.dispatched)" 2
wait_thumbs_idle
check "refresh_cache 整库派发" "$(post_json "$BASE/api/v1/library/refresh_cache" '{"type":"library"}' | jq -r .data.dispatched)" 5
wait_thumbs_idle
check "refresh_cache 分类范围派发" "$(post_json "$BASE/api/v1/library/refresh_cache" '{"type":"category","value":"参考"}' | jq -r .data.dispatched)" 1
wait_thumbs_idle
check "refresh_cache 标签范围派发" "$(post_json "$BASE/api/v1/library/refresh_cache" '{"type":"tag","value":"nature"}' | jq -r .data.dispatched)" 1
wait_thumbs_idle
check "refresh_cache 未知类型 400" "$(post_json "$BASE/api/v1/library/refresh_cache" '{"type":"nope"}' -o /dev/null -w '%{http_code}')" 400
check "refresh_cache 缺 value 400" "$(post_json "$BASE/api/v1/library/refresh_cache" '{"type":"category"}' -o /dev/null -w '%{http_code}')" 400

# --- 宽高自愈闭环：模拟入库时 identify 暂时失败落 0 的遗留 ---
# 停库后从 TOML 删除宽高字段（width=0 序列化时即省略），重启后对账把 0 搬进索引，
# 自愈三路（周期对账 / item·list 读取端 / refresh_cache）任一收敛即闭环：宽高恢复且回写 TOML
kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true
sed -i '/^width = /d; /^height = /d' "$LIB/.hawk/metadata/$SUNSET_ID.toml"
HAWK_TOKEN=$TOKEN "${SERVER[@]}" --library "$LIB" --port $PORT >>"$WORK/server.log" 2>&1 &
PID=$!
for _ in $(seq 1 60); do curl -sf -H "$AUTH" "$BASE/api/v1/app/status" >/dev/null 2>&1 && break; sleep 0.5; done
W=0
for _ in $(seq 1 60); do
  W=$(curl -s -H "$AUTH" "$BASE/api/v1/item/detail?id=$SUNSET_ID" | jq -r '.data.width')
  [[ "$W" == "4" ]] && break
  post_json "$BASE/api/v1/item/list" '{}' >/dev/null # 模拟前端拉列表，触发读取端自愈
  sleep 0.5
done
check "宽高自愈恢复（0 × 0 → 4）" "$W" 4
check "自愈结果回写 TOML" "$(grep -c '^width = 4$' "$LIB/.hawk/metadata/$SUNSET_ID.toml")" 1

# --- 局域网写权限（[web] writable）：viewer token 默认只读，开启后可写，热生效 ---
LAN_PORT=27398
LAN_BASE="http://127.0.0.1:$LAN_PORT"
VIEWER_AUTH="Authorization: Bearer viewer-token"
# 原生 Windows curl 拿不到 MSYS 路径，上传源转 Windows 形式；内容与库内任何项不同（唯一哈希，删除语义确定）
printf 'hawk-lan-upload-probe-unique' > "$WORK/lan-src.png"
UPLOAD_SRC="$WORK/lan-src.png"
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) UPLOAD_SRC=$(cygpath -w "$WORK/lan-src.png") ;;
esac
cat > "$LIB/.hawk/config.toml" <<EOF
[web]
enabled = true
port = $LAN_PORT
token = "viewer-token"
EOF
for _ in $(seq 1 20); do curl -sf "$LAN_BASE/health" >/dev/null 2>&1 && break; sleep 0.5; done
check "admin app/info 恒可写" "$(curl -s -H "$AUTH" "$BASE/api/v1/app/info" | jq -r .data.writable)" true
check "viewer app/info access" "$(curl -s -H "$VIEWER_AUTH" "$LAN_BASE/api/v1/app/info" | jq -r .data.access)" viewer
check "viewer 默认 writable=false" "$(curl -s -H "$VIEWER_AUTH" "$LAN_BASE/api/v1/app/info" | jq -r .data.writable)" false
check "viewer GET 放行" "$(curl -s -H "$VIEWER_AUTH" "$LAN_BASE/api/v1/item/count" | jq -r .data)" 5
check "viewer 上传被拒（READ_ONLY）" "$(curl -s -H "$VIEWER_AUTH" -F "file=@$UPLOAD_SRC" -F "name=lan-upload" "$LAN_BASE/api/v1/item/upload" | jq -r .error.code)" READ_ONLY

echo 'writable = true' >> "$LIB/.hawk/config.toml"
W=false
for _ in $(seq 1 20); do
  W=$(curl -s -H "$VIEWER_AUTH" "$LAN_BASE/api/v1/app/info" | jq -r .data.writable)
  [[ "$W" == "true" ]] && break
  sleep 0.5
done
check "writable 热生效" "$W" true
UPLOAD=$(curl -s -H "$VIEWER_AUTH" -F "file=@$UPLOAD_SRC" -F "name=lan-upload" "$LAN_BASE/api/v1/item/upload")
check "viewer 上传成功" "$(echo "$UPLOAD" | jq -r .data.item.name)" lan-upload
check "首次上传 already_existed=false" "$(echo "$UPLOAD" | jq -r .data.already_existed)" false
check "上传后 count=6" "$(curl -s -H "$VIEWER_AUTH" "$LAN_BASE/api/v1/item/count" | jq -r .data)" 6
check "同名上传被拒（FILE_EXISTS）" "$(curl -s -H "$VIEWER_AUTH" -F "file=@$UPLOAD_SRC" -F "name=lan-upload" "$LAN_BASE/api/v1/item/upload" | jq -r .error.code)" FILE_EXISTS
check "上传文件已落盘" "$(ls "$LIB/lan-upload.png" >/dev/null 2>&1 && echo yes)" yes
LAN_ID=$(echo "$UPLOAD" | jq -r .data.item.id)
check "viewer 删除成功" "$(curl -s -H "$VIEWER_AUTH" -X POST "$LAN_BASE/api/v1/item/delete" -H 'Content-Type: application/json' --data-binary @- <<< "{\"id\":\"$LAN_ID\"}" | jq -r .status)" success
check "删除后 count=5" "$(curl -s -H "$VIEWER_AUTH" "$LAN_BASE/api/v1/item/count" | jq -r .data)" 5

# 关闭写权限：全量覆写（sed -i 是重命名替换，watcher 对 config 的 rename 不发 ConfigChanged；
# Electron 实际用 writeFileSync 原地写，与此处 cat > 同路径）
cat > "$LIB/.hawk/config.toml" <<EOF
[web]
enabled = true
port = $LAN_PORT
token = "viewer-token"
EOF
W=true
for _ in $(seq 1 20); do
  W=$(curl -s -H "$VIEWER_AUTH" "$LAN_BASE/api/v1/app/info" | jq -r .data.writable)
  [[ "$W" == "false" ]] && break
  sleep 0.5
done
check "关闭 writable 后再次只读" "$(curl -s -H "$VIEWER_AUTH" -F "file=@$UPLOAD_SRC" -F "name=lan-upload2" "$LAN_BASE/api/v1/item/upload" | jq -r .error.code)" READ_ONLY

# --- 拆分模式（separate_write_token + write_token）：主 token 降只读，可写 token 可写 ---
cat > "$LIB/.hawk/config.toml" <<EOF
[web]
enabled = true
port = $LAN_PORT
token = "viewer-token"
writable = true
separate_write_token = true
write_token = "viewer-write-token"
EOF
RW_AUTH="Authorization: Bearer viewer-write-token"
for _ in $(seq 1 20); do
  W=$(curl -s -H "$RW_AUTH" "$LAN_BASE/api/v1/app/info" | jq -r '.data.access + ":" + (.data.writable|tostring)')
  [[ "$W" == "viewer:true" ]] && break
  sleep 0.5
done
check "拆分热生效（可写 token 就位）" "$W" "viewer:true"
check "拆分后主 token 降只读" "$(curl -s -H "$VIEWER_AUTH" "$LAN_BASE/api/v1/app/info" | jq -r .data.writable)" false
check "拆分后主 token 上传被拒" "$(curl -s -H "$VIEWER_AUTH" -F "file=@$UPLOAD_SRC" -F "name=lan-split" "$LAN_BASE/api/v1/item/upload" | jq -r .error.code)" READ_ONLY
SPLIT=$(curl -s -H "$RW_AUTH" -F "file=@$UPLOAD_SRC" -F "name=lan-split" "$LAN_BASE/api/v1/item/upload")
check "可写 token 上传成功" "$(echo "$SPLIT" | jq -r .data.item.name)" lan-split
check "可写 token GET 正常" "$(curl -s -H "$RW_AUTH" "$LAN_BASE/api/v1/item/count" | jq -r .data)" 6
SPLIT_ID=$(echo "$SPLIT" | jq -r .data.item.id)
check "可写 token 删除成功" "$(curl -s -H "$RW_AUTH" -X POST "$LAN_BASE/api/v1/item/delete" -H 'Content-Type: application/json' --data-binary @- <<< "{\"id\":\"$SPLIT_ID\"}" | jq -r .status)" success
# 拆分但未启用写：write_token 不生效（不算合法 token）
cat > "$LIB/.hawk/config.toml" <<EOF
[web]
enabled = true
port = $LAN_PORT
token = "viewer-token"
writable = false
separate_write_token = true
write_token = "viewer-write-token"
EOF
RW_INVALID=401
for _ in $(seq 1 20); do
  RW_INVALID=$(curl -s -o /dev/null -w '%{http_code}' -H "$RW_AUTH" "$LAN_BASE/api/v1/item/count")
  [[ "$RW_INVALID" == "401" ]] && break
  sleep 0.5
done
check "未启用写时 write_token 不生效（401）" "$RW_INVALID" 401

echo
echo "通过 $PASS 项，失败 $FAIL 项"
[[ $FAIL -eq 0 ]]
