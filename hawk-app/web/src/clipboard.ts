// 图片写入系统剪贴板（预览右键菜单与 Ctrl/Cmd+C 快捷键共用）。
// Chromium 的 clipboard.write 只接受 image/png：非 PNG 统一经 canvas 转码（动图取首帧，
// 元数据不保留），边长超 canvas 上限时等比降采样。失败以异常上抛，调用方负责 toast 原因。
import { api } from './api/endpoints';

/** canvas 单边长上限（与 imageEdit 的保守取值一致），超出时等比降采样避免编码失败 */
const MAX_CANVAS_SIDE = 16384;

/** 复制指定素材的原图到剪贴板（fetch 经本机 daemon，keydown 的用户激活窗口内完成） */
export async function copyImageToClipboard(id: string): Promise<void> {
  const blob = await (await fetch(api.fileUrl(id))).blob();
  await navigator.clipboard.write([new ClipboardItem({ 'image/png': await toPngBlob(blob) })]);
}

/** 任意图片 Blob → PNG Blob：Chromium 剪贴板只收 image/png */
export async function toPngBlob(blob: Blob): Promise<Blob> {
  if (blob.type === 'image/png') {
    return blob;
  }
  const url = URL.createObjectURL(blob);
  try {
    const img = await new Promise<HTMLImageElement>((resolve, reject) => {
      const el = new Image();
      el.onload = () => resolve(el);
      el.onerror = () => reject(new Error('图片解码失败'));
      el.src = url;
    });
    const w = img.naturalWidth;
    const h = img.naturalHeight;
    if (!w || !h) {
      throw new Error('无法确定图片尺寸');
    }
    const scale = Math.min(1, MAX_CANVAS_SIDE / Math.max(w, h));
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, Math.round(w * scale));
    canvas.height = Math.max(1, Math.round(h * scale));
    canvas.getContext('2d')!.drawImage(img, 0, 0, canvas.width, canvas.height);
    return await new Promise<Blob>((resolve, reject) => canvas.toBlob((b) => (b ? resolve(b) : reject(new Error('PNG 编码失败'))), 'image/png'));
  } finally {
    URL.revokeObjectURL(url);
  }
}
