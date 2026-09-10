// 展示格式化小工具（检查器等组件共用）。

export function formatSize(bytes: number): string {
  if (bytes >= 1 << 30) return (bytes / (1 << 30)).toFixed(2) + ' GB';
  if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(2) + ' MB';
  if (bytes >= 1 << 10) return (bytes / (1 << 10)).toFixed(1) + ' KB';
  return bytes + ' B';
}

export function formatTime(ms: number): string {
  return new Date(Number(ms)).toLocaleString();
}
