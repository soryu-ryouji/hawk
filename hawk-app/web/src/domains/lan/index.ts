// 局域网互联域：源端发现/配对/连接另一台 hawk-app。连接切换本身由主进程驱动
// （lanPair/lanConnect → connection-changed → startup 重注水），本 store 只管面板数据与动作。
import { ref } from 'vue';
import { defineStore } from 'pinia';
import { shell } from '@/shared/lib/platform';
import { ipcMessage } from '@/domains/remote';
import type { LanPeer, LanPeerProfile } from '@/shared/types';

export const useLanStore = defineStore('lan', () => {
  const discovered = ref<LanPeer[]>([]);
  const saved = ref<LanPeerProfile[]>([]);
  const scanning = ref(false);
  const connectingId = ref<string | null>(null);
  const error = ref<string | null>(null);

  async function refreshSaved(): Promise<void> {
    try {
      saved.value = (await shell.lanPeers()).peers;
    } catch (e) {
      error.value = ipcMessage(e);
    }
  }

  async function scan(): Promise<void> {
    if (scanning.value) {
      return;
    }
    scanning.value = true;
    error.value = null;
    try {
      discovered.value = (await shell.lanDiscover()).peers;
    } catch (e) {
      error.value = ipcMessage(e);
    } finally {
      scanning.value = false;
    }
  }

  /** 配对并切换到目标库；成功后新连接由主进程推送，前端 boot 自动重注水 */
  async function pair(peer: LanPeer, code: string, writable: boolean): Promise<boolean> {
    if (connectingId.value) {
      return false;
    }
    connectingId.value = peer.id;
    error.value = null;
    try {
      await shell.lanPair({ peer, code, writable });
      await refreshSaved();
      return true;
    } catch (e) {
      error.value = ipcMessage(e);
      return false;
    } finally {
      connectingId.value = null;
    }
  }

  /** 连接已保存设备（token 在主进程；这里只按 id 发起） */
  async function connect(peerId: string): Promise<boolean> {
    if (connectingId.value) {
      return false;
    }
    connectingId.value = peerId;
    error.value = null;
    try {
      await shell.lanConnect(peerId);
      await refreshSaved();
      return true;
    } catch (e) {
      error.value = ipcMessage(e);
      return false;
    } finally {
      connectingId.value = null;
    }
  }

  async function forget(peerId: string): Promise<void> {
    try {
      await shell.lanForget(peerId);
      await refreshSaved();
    } catch (e) {
      error.value = ipcMessage(e);
    }
  }

  return { discovered, saved, scanning, connectingId, error, refreshSaved, scan, pair, connect, forget };
});
