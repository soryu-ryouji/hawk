// 系统通知反馈（保存成功/失败）
import { browser } from 'wxt/browser';

export async function notify(message: string) {
  await browser.notifications.create({
    type: 'basic',
    iconUrl: browser.runtime.getURL('/icons/128.png'),
    title: 'hawk',
    message,
  });
}
