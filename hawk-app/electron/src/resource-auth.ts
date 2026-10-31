// 渲染进程的图片与 EventSource 无法自行设置 Authorization；由主进程在网络层为当前 API 来源补充 Bearer。
// 只匹配环回 API：本地 daemon 与 LAN/WAN 会话代理均监听 127.0.0.1，凭证不会随其他来源的请求发出。
import { session } from 'electron';

let current: { origin: string; credential: string } | null = null;
let registered = false;

export function setResourceAuth(api?: string, credential?: string): void {
  current = api && credential ? { origin: new URL(api).origin, credential } : null;
  if (registered) {
    return;
  }
  registered = true;
  session.defaultSession.webRequest.onBeforeSendHeaders({ urls: ['http://127.0.0.1/*', 'http://localhost/*'] }, (details, callback) => {
    const requestHeaders = { ...details.requestHeaders };
    if (current && new URL(details.url).origin === current.origin) {
      requestHeaders.Authorization = `Bearer ${current.credential}`;
    }
    callback({ requestHeaders });
  });
}
