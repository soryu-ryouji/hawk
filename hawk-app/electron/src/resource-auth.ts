// 渲染进程的图片与 EventSource 无法自行设置 Authorization；由主进程在网络层为当前 API 来源补充 Bearer。
// 当前 API 可能是本地环回，也可能是局域网直连的 Peer 端口（http://192.168.x.x:27375），
// 因此监听所有 http(s) 请求，但只对与当前 API 完全同源的请求附加凭证，其他来源不会拿到 token。
import { session } from 'electron';

let current: { origin: string; credential: string } | null = null;
let registered = false;

export function setResourceAuth(api?: string, credential?: string): void {
  current = api && credential ? { origin: new URL(api).origin, credential } : null;
  if (registered) {
    return;
  }
  registered = true;
  session.defaultSession.webRequest.onBeforeSendHeaders({ urls: ['http://*/*', 'https://*/*'] }, (details, callback) => {
    const requestHeaders = { ...details.requestHeaders };
    if (current && new URL(details.url).origin === current.origin) {
      requestHeaders.Authorization = `Bearer ${current.credential}`;
    }
    callback({ requestHeaders });
  });
}
