// 图片写入系统剪贴板（预览右键菜单与 Ctrl/Cmd+C 快捷键共用）。
// 能力检测：navigator.clipboard 是 [SecureContext] IDL 属性，非安全上下文（局域网 http://IP）下
// 整个属性不存在，直接调用会报错；安全上下文里个别浏览器也可能缺 write / ClipboardItem（旧 Firefox）。
// write 必须在用户手势窗口内同步发起（Safari 强制）：把取图/转码的 Promise 直接交给 ClipboardItem，
// 而不是 await 之后才 write。不做 execCommand 兜底——脚本复制只能写 text/html/text/plain，
// 写不进 image/png，假兜底会造成「复制成功但粘贴不出图」的更差体验。
// Chromium 的 clipboard.write 只接受 image/png：非 PNG 统一经 canvas 转码（动图取首帧，
// 元数据不保留），边长超 canvas 上限时等比降采样。失败以异常上抛，调用方负责 toast 原因。
import { api } from '@/shared/api/endpoints';

/** canvas 单边长上限（与 imageEdit 的保守取值一致），超出时等比降采样避免编码失败 */
const MAX_CANVAS_SIDE = 16384;

/** 当前环境是否支持脚本写入图片剪贴板（安全上下文，且实现了 clipboard.write 与 ClipboardItem） */
export function canCopyImageToClipboard(): boolean {
  return (
    window.isSecureContext &&
    typeof navigator.clipboard !== 'undefined' &&
    typeof navigator.clipboard.write === 'function' &&
    typeof ClipboardItem !== 'undefined'
  );
}

/** 复制指定素材的原图到剪贴板（fetch 经本机 daemon；write 同步发起，取图经 Promise 异步补齐） */
export async function copyImageToClipboard(id: string): Promise<void> {
  if (!window.isSecureContext) {
    throw new Error('HTTP 访问下浏览器禁止写入剪贴板，无法复制图片；请用「保存图片」，或改用 localhost / HTTPS 访问');
  }
  if (!canCopyImageToClipboard()) {
    throw new Error('当前浏览器不支持复制图片');
  }
  const png = fetch(api.fileUrl(id))
    .then((r) => r.blob())
    .then(toPngBlob);
  await navigator.clipboard.write([new ClipboardItem({ 'image/png': png })]);
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
