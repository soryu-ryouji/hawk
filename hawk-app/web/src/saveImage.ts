// 原图保存（浏览器端局域网查看）：拉原图 blob 后按平台能力分流。
// iOS Safari 忽略 <a download>（点击直接开新标签页），带文件的 Web Share 是系统级
// 「存储图像/存储到文件」路径（iOS 15+）；Android/桌面浏览器走 <a download>。
import { api } from './api/endpoints';
import type { Item } from './types';

export type SaveImageResult = 'saved' | 'cancelled';

export async function saveImageToDisk(item: Item): Promise<SaveImageResult> {
  const filename = `${item.name}.${item.ext}`;
  const blob = await (await fetch(api.fileUrl(item.id))).blob();
  const file = new File([blob], filename, { type: blob.type || 'application/octet-stream' });

  // Web Share（带文件）：iOS 系统分享面板，可存入相册/文件 App；canShare false 时静默走 download
  if (navigator.canShare?.({ files: [file] })) {
    try {
      await navigator.share({ files: [file] });
      return 'saved';
    } catch (error) {
      // 用户取消分享面板不是失败，静默返回
      if ((error as Error)?.name === 'AbortError') {
        return 'cancelled';
      }
      throw error;
    }
  }

  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  // revoke 定时兜底：个别浏览器 click 后异步才开始下载，立即 revoke 会拿到空文件
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
  return 'saved';
}
