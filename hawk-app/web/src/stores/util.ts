// store 共用小工具：错误码翻译与防抖。从 library.ts 拆出，taxonomy store 与主 store 共用。
import { ApiError } from '../api/client';

const ERROR_TEXT: Record<string, string> = {
  FILE_EXISTS: '同名文件或文件夹已存在',
  ITEM_NOT_FOUND: '素材不存在或已被移除',
  FOLDER_NOT_FOUND: '文件夹不存在',
  CATEGORY_NOT_FOUND: '分类不存在',
  CATEGORY_EXISTS: '分类已存在',
  TAG_NOT_FOUND: '标签不存在',
  UNSUPPORTED_FORMAT: '不支持的格式',
  INVALID_PARAM: '参数无效',
  NETWORK: '无法连接 hawk-daemon',
};

/** ApiError 错误码翻译（store 与需要错误文案的模块共用） */
export function errorText(e: unknown): string {
  return e instanceof ApiError ? (ERROR_TEXT[e.code] ?? e.message) : String(e);
}

/** 简易防抖（模块级，store 单例无需多实例隔离） */
export function debounce(ms: number) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return (fn: () => void) => {
    clearTimeout(timer);
    timer = setTimeout(fn, ms);
  };
}
