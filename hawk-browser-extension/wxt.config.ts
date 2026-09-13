import { defineConfig } from 'wxt';

// 跨浏览器清单：Chrome/Firefox/Safari 共用一份代码，WXT 负责各平台差异
//（Firefox 自动产出 MV2 + event page，Safari 同理）
export default defineConfig({
  // Firefox 附加组件需要显式 ID；数据收集声明告警与本插件无关，按官方指引抑制
  suppressWarnings: { firefoxDataCollection: true },
  manifest: ({ browser }) => ({
    name: 'hawk 图片收集',
    description: '保存网页图片到 hawk 素材库',
    // 防盗链站点（如 pixiv 的 i.pximg.net）校验 Referer，而 service worker 的 fetch
    // 无法设置跨源 Referer（fetch 规范禁止），需请求头改写能力：Chrome/Safari 用
    // declarativeNetRequest；Firefox(MV2) 不支持 DNR，用 webRequest blocking
    permissions: [
      'contextMenus',
      'storage',
      'notifications',
      ...(browser === 'firefox'
        ? (['webRequest', 'webRequestBlocking'] as const)
        : browser === 'safari'
          ? (['declarativeNetRequest'] as const)
          : (['declarativeNetRequestWithHostAccess'] as const)),
    ],
    // 本机 daemon 直连；http(s) 任意源用于图片下载——内容一律经浏览器网络栈
    // （真实 Chrome TLS 指纹 + 用户代理与会话，服务端无代理无会话拿不到），
    // 下载后转 base64 提交，服务端不再自行下载
    host_permissions: ['http://127.0.0.1:27371/*', 'http://localhost:27371/*', 'http://*/*', 'https://*/*'],
    icons: {
      16: '/icons/16.png',
      32: '/icons/32.png',
      48: '/icons/48.png',
      128: '/icons/128.png',
    },
    // Firefox 要求扩展 ID（MV2 推荐、MV3 必需）
    ...(browser === 'firefox'
      ? { browser_specific_settings: { gecko: { id: 'hawk-app@ryouji.dev' } } }
      : {}),
  }),
});
