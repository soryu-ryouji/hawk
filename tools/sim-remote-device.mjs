#!/usr/bin/env node
// 模拟 hawk-app（daemon remote 模块）接入 hawk-server 控制面：
//   POST /api/v1/devices 注册 → wss /api/v1/ws 控制通道（hello/hello_ack、ping/pong）
//   → POST /api/v1/devices/heartbeat 周期心跳 → GET /api/v1/devices 自查在线状态。
// 每一步打印耗时，用于定位「接入慢」发生在哪一环（DNS/TLS/注册/WS 升级/hello/心跳）。
// 零依赖（Node >= 18，手写 WS 握手与帧编解码以携带 Authorization 头）。
//
// 用法：
//   node tools/sim-remote-device.mjs --key hs_xxx \
//        [--server https://hawk.soryu-ryouji.cc] [--device-id d_42] \
//        [--seconds 75] [--name 本地模拟] [--cleanup]
//
//   --device-id  复用已注册设备（跳过注册，避免设备列表堆积）
//   --cleanup    结束时 DELETE 注销本次注册的设备
//
// 协议契约见 docs/backend/remote-protocol.md。

import crypto from 'node:crypto';
import net from 'node:net';
import os from 'node:os';
import tls from 'node:tls';
import { performance } from 'node:perf_hooks';

// ---------- 参数 ----------

const args = {};
for (let i = 2; i < process.argv.length; i++) {
  const k = process.argv[i];
  if (k === '--cleanup') {
    args.cleanup = true;
  } else if (k.startsWith('--')) {
    args[k.slice(2)] = process.argv[++i];
  }
}
const KEY = args.key ?? process.env.HAWK_SERVICE_KEY;
if (!KEY) {
  console.error('缺少 --key <service-key>（或环境变量 HAWK_SERVICE_KEY）');
  process.exit(2);
}
const base = (args.server ?? 'https://hawk.soryu-ryouji.cc').replace(/\/+$/, '');
const SECONDS = Number(args.seconds ?? 75);
const LIBRARY_NAME = '模拟素材库';

const t0 = performance.now();
const elapsed = () => `+${(performance.now() - t0).toFixed(0)}ms`;
const log = (msg) => console.log(`[${elapsed()}] ${msg}`);

// ---------- REST ----------

async function api(method, path, body) {
  const s = performance.now();
  const res = await fetch(`${base}${path}`, {
    method,
    headers: { Authorization: `Bearer ${KEY}`, 'Content-Type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(15000),
  });
  const text = await res.text();
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    parsed = text;
  }
  const ms = (performance.now() - s).toFixed(0);
  return { status: res.status, body: parsed, ms };
}

function heartbeatBody(deviceId) {
  return {
    device_id: deviceId,
    library_name: LIBRARY_NAME,
    allow_write: false,
    reachability: {
      public_ip: null,
      mapped_port: null,
      mapping_type: 'none',
      public_ipv6: null,
      ipv6_port: null,
      nat_type: 'unknown',
    },
  };
}

// ---------- 最小 WebSocket 客户端（支持自定义握手头） ----------

function encodeFrame(opcode, payload) {
  const data = Buffer.isBuffer(payload) ? payload : Buffer.from(payload);
  const mask = crypto.randomBytes(4);
  let header;
  if (data.length < 126) {
    header = Buffer.from([0x80 | opcode, 0x80 | data.length]);
  } else if (data.length < 65536) {
    header = Buffer.alloc(4);
    header[0] = 0x80 | opcode;
    header[1] = 0x80 | 126;
    header.writeUInt16BE(data.length, 2);
  } else {
    header = Buffer.alloc(10);
    header[0] = 0x80 | opcode;
    header[1] = 0x80 | 127;
    header.writeUInt32BE(0, 2);
    header.writeUInt32BE(data.length, 6);
  }
  const masked = Buffer.from(data);
  for (let i = 0; i < masked.length; i++) masked[i] ^= mask[i & 3];
  return Buffer.concat([header, mask, masked]);
}

class FrameParser {
  constructor(onFrame) {
    this.buf = Buffer.alloc(0);
    this.frags = [];
    this.fragOp = 0;
    this.onFrame = onFrame;
  }

  push(chunk) {
    this.buf = Buffer.concat([this.buf, chunk]);
    for (;;) {
      if (this.buf.length < 2) return;
      const b0 = this.buf[0];
      const b1 = this.buf[1];
      const fin = (b0 & 0x80) !== 0;
      const op = b0 & 0x0f;
      const masked = (b1 & 0x80) !== 0;
      let len = b1 & 0x7f;
      let off = 2;
      if (len === 126) {
        if (this.buf.length < 4) return;
        len = this.buf.readUInt16BE(2);
        off = 4;
      } else if (len === 127) {
        if (this.buf.length < 10) return;
        len = Number(this.buf.readBigUInt64BE(2));
        off = 10;
      }
      let maskKey = null;
      if (masked) {
        if (this.buf.length < off + 4) return;
        maskKey = this.buf.subarray(off, off + 4);
        off += 4;
      }
      if (this.buf.length < off + len) return;
      const payload = Buffer.from(this.buf.subarray(off, off + len));
      if (maskKey) for (let i = 0; i < payload.length; i++) payload[i] ^= maskKey[i & 3];
      this.buf = this.buf.subarray(off + len);
      if (op === 0x0) {
        this.frags.push(payload);
        if (fin) {
          const full = Buffer.concat(this.frags);
          this.frags = [];
          this.onFrame(this.fragOp, full);
        }
      } else if (op === 0x1 || op === 0x2) {
        if (fin) this.onFrame(op, payload);
        else {
          this.frags = [payload];
          this.fragOp = op;
        }
      } else {
        this.onFrame(op, payload); // 控制帧（close/ping/pong）不分片
      }
    }
  }
}

function wsConnect(wsUrl, headers, timeoutMs = 15000) {
  return new Promise((resolve, reject) => {
    const u = new URL(wsUrl);
    const secure = u.protocol === 'wss:';
    const port = u.port ? Number(u.port) : secure ? 443 : 80;
    const path = `${u.pathname}${u.search}`;
    const s = performance.now();
    let settled = false;
    const done = (err, value) => {
      if (settled) return;
      settled = true;
      if (err) reject(err);
      else resolve(value);
    };
    const onConnect = (socket) => {
      const tcpMs = (performance.now() - s).toFixed(0);
      const wsKey = crypto.randomBytes(16).toString('base64');
      const lines = [
        `GET ${path} HTTP/1.1`,
        `Host: ${u.host}`,
        'Upgrade: websocket',
        'Connection: Upgrade',
        `Sec-WebSocket-Key: ${wsKey}`,
        'Sec-WebSocket-Version: 13',
        ...Object.entries(headers).map(([k, v]) => `${k}: ${v}`),
        '\r\n',
      ];
      socket.write(lines.join('\r\n'));
      let buf = Buffer.alloc(0);
      const onData = (chunk) => {
        buf = Buffer.concat([buf, chunk]);
        const idx = buf.indexOf('\r\n\r\n');
        if (idx === -1) return;
        socket.off('data', onData);
        const head = buf.subarray(0, idx).toString();
        const rest = buf.subarray(idx + 4);
        const statusLine = head.split('\r\n')[0];
        if (!statusLine.includes('101')) {
          socket.destroy();
          return done(new Error(`WS 升级被拒绝: ${statusLine}`));
        }
        done(null, { socket, rest, tcpMs, upgradeMs: (performance.now() - s).toFixed(0) });
      };
      socket.on('data', onData);
    };
    const socket = secure
      ? tls.connect({ host: u.hostname, port, servername: u.hostname }, () => onConnect(socket))
      : net.connect({ host: u.hostname, port }, () => onConnect(socket));
    socket.setTimeout(timeoutMs, () => {
      socket.destroy();
      done(new Error(`WS 连接超时（${timeoutMs}ms，含 TLS 与升级）`));
    });
    socket.on('error', (e) => done(new Error(`WS 连接失败: ${e.message}`)));
  });
}

// ---------- 主流程 ----------

async function main() {
  // 1. 注册设备（或复用）
  let deviceId = args['device-id'];
  if (deviceId) {
    log(`复用设备 ${deviceId}（跳过注册）`);
  } else {
    const r = await api('POST', '/api/v1/devices', {
      name: args.name ?? `本地模拟-${os.hostname()}`,
      platform: process.platform,
      fingerprint: crypto.randomBytes(32).toString('hex'), // 64 位小写十六进制
      library_name: LIBRARY_NAME,
      allow_remote: true,
      allow_write: false,
    });
    if (r.status !== 200) {
      log(`注册失败（${r.ms}ms）: HTTP ${r.status} ${JSON.stringify(r.body)}`);
      process.exit(1);
    }
    deviceId = r.body.data.device_id;
    log(`注册成功（${r.ms}ms）: device_id=${deviceId}`);
  }

  // 2. WSS 控制通道
  const wsUrl = `${base.replace(/^http/, 'ws')}/api/v1/ws?device_id=${deviceId}`;
  const { socket, rest, tcpMs, upgradeMs } = await wsConnect(wsUrl, {
    Authorization: `Bearer ${KEY}`,
  });
  log(`控制通道已升级（TCP+TLS ${tcpMs}ms，含升级共 ${upgradeMs}ms）`);
  socket.setTimeout(0);
  let intentionalClose = false; // 主动收尾时不走 close 处理的 exit，让给清理逻辑

  const sendText = (obj) => socket.write(encodeFrame(0x1, JSON.stringify(obj)));
  const parser = new FrameParser((op, payload) => {
    if (op === 0x1) {
      let msg;
      try {
        msg = JSON.parse(payload.toString());
      } catch {
        return;
      }
      switch (msg.type) {
        case 'hello':
          log(`收到 hello（server_time_ms=${msg.server_time_ms}），回 hello_ack`);
          sendText({ type: 'hello_ack' });
          break;
        case 'ping':
          log('收到 ping，回 pong');
          sendText({ type: 'pong' });
          break;
        default:
          log(`收到推送: ${payload.toString()}`);
      }
    } else if (op === 0x9) {
      socket.write(encodeFrame(0xa, payload)); // 协议级 ping → pong
    } else if (op === 0x8) {
      log('服务端关闭控制通道');
      socket.end();
    }
  });
  socket.on('data', (chunk) => parser.push(chunk));
  socket.on('close', () => {
    log('控制通道 socket 已关闭');
    if (!intentionalClose) process.exit(0); // 服务端断开：直接结束
  });
  socket.on('error', (e) => log(`控制通道错误: ${e.message}`));
  if (rest.length > 0) parser.push(rest);

  // 3. 心跳（连接建立即一跳，随后 30s 周期；协议最小间隔 10s）
  const beat = async () => {
    const r = await api('POST', '/api/v1/devices/heartbeat', heartbeatBody(deviceId));
    if (r.status === 200) {
      log(`心跳成功（${r.ms}ms）: public_ip_seen=${r.body.data?.public_ip_seen}`);
    } else {
      log(`心跳失败（${r.ms}ms）: HTTP ${r.status} ${JSON.stringify(r.body)}`);
    }
  };
  await beat();
  const timer = setInterval(beat, 30000);

  // 4. 服务端视角自查
  const list = await api('GET', '/api/v1/devices');
  const me = list.body?.data?.devices?.find((d) => d.device_id === deviceId);
  log(`GET /devices（${list.ms}ms）: 本机 online=${me?.online}（以心跳为准）`);

  log(`保持 ${SECONDS}s（Ctrl-C 提前结束）……`);
  await new Promise((r) => setTimeout(r, SECONDS * 1000));
  clearInterval(timer);

  // 5. 收尾
  intentionalClose = true;
  socket.write(encodeFrame(0x8, Buffer.alloc(0)));
  socket.end();
  if (args.cleanup && !args['device-id']) {
    const r = await api('DELETE', `/api/v1/devices/${deviceId}`);
    log(`已注销设备: HTTP ${r.status}`);
  } else {
    log(`设备保留在池中。清理：curl -X DELETE -H "Authorization: Bearer $KEY" ${base}/api/v1/devices/${deviceId}`);
  }
  process.exit(0);
}

process.on('SIGINT', () => process.exit(130));
main().catch(async (e) => {
  const { inspect } = await import('node:util');
  log(`失败: ${e?.stack ?? inspect(e, { depth: 5 })}`);
  process.exit(1);
});
