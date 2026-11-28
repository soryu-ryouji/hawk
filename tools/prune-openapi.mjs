// 从旧契约中移除广域网/加密相关端点与字段，生成纯局域网 OpenAPI 契约。
// 用法：node tools/prune-openapi.mjs
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const file = path.join(root, 'hawk-daemon', 'openapi.json');
const doc = JSON.parse(fs.readFileSync(file, 'utf8'));
doc.info = { title: 'hawk-daemon | v1', version: '0.1.0' };
doc.servers = [{ url: 'http://127.0.0.1:27371/' }];

for (const key of Object.keys(doc.paths)) {
  if (key.startsWith('/api/v1/remote') || key.startsWith('/api/v1/rendezvous')) {
    delete doc.paths[key];
  }
}

const removedSchemas = [
  'AccessConnectBody',
  'AccessRendezvousBody',
  'AccessSessionBody',
  'AccessSessionDto',
  'ActiveSessionDto',
  'CreateKeyBody',
  'CreateKeyDto',
  'LeaseDto',
  'PatchDeviceBody',
  'PatchKeyBody',
  'PatchKeyDto',
  'PutRemoteBody',
  'RemoteDeviceDto',
  'RemoteDevicesDto',
  'RemoteDto',
  'RemoteKeyDto',
  'RemoteKeysDto',
  'RemoteSessionPayload',
  'RemoteState',
  'RemoteStatus',
  'RotateDeviceDto',
];
for (const name of removedSchemas) {
  delete doc.components.schemas[name];
}
for (const name of Object.keys(doc.components.schemas)) {
  if (/^Envelope_(AccessSession|ActiveSession|CreateKey|Lease|PatchKey|Remote|RotateDevice)/.test(name)) {
    delete doc.components.schemas[name];
  }
}

// 递归清理所有对象属性中的 WAN/iroh/TLS 字段（含 Envelope_* 的内联 data 结构）。
const removedFields = new Set(['target_identity', 'tls_fingerprint', 'target_device_id']);
function pruneNode(node) {
  if (Array.isArray(node)) {
    node.forEach(pruneNode);
    return;
  }
  if (!node || typeof node !== 'object') {
    return;
  }
  if (node.properties && typeof node.properties === 'object') {
    for (const field of removedFields) {
      delete node.properties[field];
    }
    if (Array.isArray(node.required)) {
      node.required = node.required.filter((field) => !removedFields.has(field));
    }
  }
  for (const value of Object.values(node)) {
    pruneNode(value);
  }
}
pruneNode(doc);

for (const [name, schema] of Object.entries(doc.components.schemas)) {
  if (name === 'AppConnectionDto' || name.startsWith('Envelope_AppConnectionDto')) {
    const target = name === 'AppConnectionDto' ? schema : schema?.properties?.data;
    if (target?.properties) {
      for (const field of ['api_base', 'credential', 'device_id', 'label']) {
        delete target.properties[field];
      }
      target.required = ['mode', 'role'];
      target.description = '当前连接描述符：纯局域网版本只有本地素材库（mode=local）。';
      if (target.properties.mode) {
        target.properties.mode.description = 'local（本地素材库）';
      }
    }
  }
  if (name === 'LanConnectionInfoDto' || name.startsWith('Envelope_LanConnectionInfoDto')) {
    const target = name === 'LanConnectionInfoDto' ? schema : schema?.properties?.data;
    if (target) {
      target.description = '目标端公开的连接信息：只暴露展示元数据，不含任何密钥。';
    }
  }
  if (name === 'AppInfo' || name.startsWith('Envelope_AppInfo')) {
    const target = name === 'AppInfo' ? schema : schema?.properties?.data;
    if (target?.properties?.channel) {
      target.properties.channel.description = '当前请求进入 daemon 的通道（loopback/lan_web/lan_peer）';
    }
  }
}

if (doc.components.schemas.LanAccessKeyDto?.properties?.access_key) {
  doc.components.schemas.LanAccessKeyDto.properties.access_key.description =
    '明文只在创建/配对成功时返回一次；列表接口为空字符串';
}

const sse = doc.components.schemas.SseEvents;
if (sse?.properties) {
  delete sse.properties['remote.session'];
  delete sse.properties['remote.status'];
  sse.required = sse.required.filter((name) => name !== 'remote.session' && name !== 'remote.status');
}

let text = JSON.stringify(doc, null, 2);
text = text
  .replaceAll('false = 远程查看模式：库端点不可用，仅远程管理面/会话代理', 'Go 版恒为 true')
  .replaceAll('响应不带 CORS 头且 Host 限定环回；远程隧道端必须直接拒绝本端点。', '响应不带 CORS 头且 Host 限定环回。')
  .replaceAll('（远程查看模式下同样可用：无库 daemon 只暴露远程管理面与会话代理）', '');
fs.writeFileSync(file, text + '\n');
console.log(`已生成纯局域网契约: ${file}`);
