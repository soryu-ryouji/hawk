import { defineConfig } from 'wxt';

// 跨浏览器清单：Chrome/Firefox/Safari 共用一份代码，WXT 负责各平台差异
//（Firefox 自动产出 MV2 + event page，Safari 同理）
export default defineConfig({
  // Firefox 附加组件需要显式 ID；数据收集声明告警与本插件无关，按官方指引抑制
  suppressWarnings: { firefoxDataCollection: true },
  manifest: ({ browser }) => ({
    name: 'hawk 图片收集',
    description: '保存网页图片到 hawk 素材库',
    permissions: ['contextMenus', 'storage', 'notifications'],
    // 只直连本机 hawk-server；扩展后台请求不受 CORS 限制
    host_permissions: ['http://127.0.0.1:27371/*', 'http://localhost:27371/*'],
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
