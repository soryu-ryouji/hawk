#!/bin/bash
# 一次性探针：relay 会话编排（注册模拟设备 → 发起到 TARGET 的 relay 会话 → 轮询状态
# → ready 后测试 role=b 中继 WS 升级 → 取消会话 → 注销模拟设备）。
# 用法: KEY=hs_xxx TARGET=d_xxx BASE=https://hawk.soryu-ryouji.cc ./tools/probe-relay-session.sh
set -u
KEY=${KEY:?需要 KEY 环境变量}
TARGET=${TARGET:?需要 TARGET 环境变量}
BASE=${BASE:-https://hawk.soryu-ryouji.cc}

jget() { node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{try{const j=JSON.parse(s);console.log(eval('j.'+process.argv[1])??'')}catch(e){console.log('')}})" "$1"; }

SIM=$(curl -s --max-time 15 -X POST -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"name":"中继探针","platform":"probe","fingerprint":"bbbb56789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","library_name":"探针","allow_remote":true}' \
  "$BASE/api/v1/devices" | jget data.device_id)
echo "模拟设备: ${SIM:-注册失败}"
[ -z "$SIM" ] && exit 1

SID=$(curl -s --max-time 15 -X POST -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d "{\"source_device_id\":\"$SIM\",\"target_device_id\":\"$TARGET\",\"mode\":\"relay\"}" \
  "$BASE/api/v1/sessions" | jget data.session_id)
echo "会话: ${SID:-创建失败}"
[ -z "$SID" ] && { curl -s -X DELETE -H "Authorization: Bearer $KEY" "$BASE/api/v1/devices/$SIM" >/dev/null; exit 1; }

STATE=""
for i in $(seq 1 15); do
  BODY=$(curl -s --max-time 15 -H "Authorization: Bearer $KEY" "$BASE/api/v1/sessions/$SID")
  STATE=$(echo "$BODY" | jget data.state)
  echo "[${i}s] state=$STATE $(echo "$BODY" | jget data.connect.fingerprint | cut -c1-16)"
  case "$STATE" in ready|rejected|expired|cancelled|unreachable) break;; esac
  sleep 1
done

if [ "$STATE" = "ready" ]; then
  echo "--- role=b 中继 WS 升级测试 ---"
  curl -s --max-time 8 --http1.1 -o /dev/null -w "升级响应: HTTP %{http_code}（%{time_total}s；101=升级成功，连接保持）\n" \
    -H "Authorization: Bearer $KEY" -H "Connection: Upgrade" -H "Upgrade: websocket" \
    -H "Sec-WebSocket-Version: 13" -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
    "$BASE/api/v1/relay?device_id=$SIM&session_id=$SID&role=b" || echo "（curl 28=超时正常：101 后连接保持，curl 等不到响应体结束）"
fi

curl -s --max-time 15 -X POST -H "Authorization: Bearer $KEY" "$BASE/api/v1/sessions/$SID/cancel" >/dev/null
curl -s --max-time 15 -X DELETE -H "Authorization: Bearer $KEY" "$BASE/api/v1/devices/$SIM" >/dev/null
echo "已收尾（cancel + 注销模拟设备）"
