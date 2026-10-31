// 局域网互联域：源端发现/配对/连接另一台 hawk-app。连接切换本身由主进程驱动
// （lanPair/lanConnect → connection-changed → startup 重注水），本 store 只管面板数据与动作。
import { ref } from 'vue';
import { defineStore } from 'pinia';
import { shell } from '@/shared/lib/platform';
import { ipcMessage } from '@/domains/remote';
import type { LanPeer, LanPeerProfile } from '@/shared/types';

/** IPC 只接受可 structured-clone 的普通对象；绝不能把 Vue reactive Proxy 传给 Electron。 */
function ipcPeer(peer: LanPeer): LanPeer {
  return {
    id: String(peer.id),
    name: String(peer.name),
    library_name: String(peer.library_name),
    url: String(peer.url),
    writable: Boolean(peer.writable),
    requires_pairing: Boolean(peer.requires_pairing),
    target_identity: String(peer.target_identity),
    target_device_id: peer.target_device_id ?? null,
    target_library_id: String(peer.target_library_id),
    tls_fingerprint: peer.tls_fingerprint ?? null,
  };
}

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

  /** 发起自动配对并等待目标端确认；成功后新连接由主进程推送，前端 boot 自动重注水 */
  async function pair(peer: LanPeer, writable: boolean, reverseWritable = false): Promise<boolean> {
    if (connectingId.value) {
      return false;
    }
    connectingId.value = peer.id;
    error.value = null;
    try {
      await shell.lanPair({ peer: ipcPeer(peer), writable: Boolean(writable), reverseWritable: Boolean(reverseWritable) });
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
