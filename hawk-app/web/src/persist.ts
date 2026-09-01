// localStorage 持久化收口：键名集中登记，读写统一走这里（损坏值回退 fallback、写入失败静默）。
// 键名与序列化格式保持既有约定不变——用户已有数据不失效，不做迁移层。
// 新增持久化键必须先在此登记（写明用途与格式），不得散点拼键名。

/** 键注册表（JSDoc 注明用途/格式/损坏回退策略） */
export const STORAGE_KEYS = {
  /** 侧栏/检查器栏宽 { sidebar?: number, inspector?: number }（JSON）；损坏或缺字段回退默认栏宽 */
  panelWidths: 'hawk:panelWidths',
  /** 缩略图尺寸偏好（纯数字字符串，合法范围 120–280）；越界/非数字回退 null（跟随动态默认） */
  thumbSize: 'hawk:thumbSize',
  /** 每素材库的视图记忆 ViewState（JSON），按库路径隔离；损坏/目标已删回退全部素材 */
  lastView: (libPath: string) => `hawk:lastView:${libPath}`,
  /** 局域网查看 token（纯字符串），按 api host 隔离；无记忆时为空串 */
  token: (host: string) => `hawk:token:${host}`,
} as const;

/** 读取 JSON：缺失/损坏回退 fallback（不抛出） */
export function loadJSON<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback; // 损坏的持久化数据
  }
}

/** 写入 JSON：隐私模式等写入失败静默（仅本次会话生效） */
export function saveJSON(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // 静默：仅本次会话生效
  }
}

/** 读取字符串：无记忆/读取失败返回 null */
export function loadText(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** 写入字符串：写入失败静默 */
export function saveText(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // 静默：仅本次会话生效
  }
}

export function removeKey(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    // 静默
  }
}
