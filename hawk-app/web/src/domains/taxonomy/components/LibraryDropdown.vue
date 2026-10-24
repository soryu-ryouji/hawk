<script setup lang="ts">
// 侧栏「素材库」下拉：库名触发按钮 + 下方对齐展开的历史库浮层（下拉模式，非右键菜单）。
// 浮层 Teleport 到 body 且 fixed 定位：侧栏有 overflow-y: auto，组件内绝对定位会被裁剪；
// 视觉与 SelectBox/ContextMenu 统一（底色/边框/阴影/勾选标记）。
// 历史由主进程记录（最近在前），当前库打勾且不可移除；目录已删的置灰不可切换但可移除记录。
// 换库就绪经 hawk:server-started 事件驱动 App 原地重启数据。
import { nextTick, onBeforeUnmount, ref, watch, computed } from 'vue';
import { shell } from '@/shared/lib/platform';
import { useContextMenu } from '@/shared/composables/useContextMenu';
import { useLibraryStore } from '@/domains/library';
import { renameLibrary } from '@/domains/library';
import { useConnectionLabel, useRemoteStore } from '@/domains/remote';
import { useLanStore } from '@/domains/lan';
import Icon from '@/shared/ui/Icon.vue';
import PromptDialog from '@/shared/ui/PromptDialog.vue';
import type { LanPeerProfile, LibraryHistoryItem, MenuItem, RemoteDevice } from '@/shared/types';

const store = useLibraryStore();
const menu = useContextMenu();
const remoteStore = useRemoteStore();
const lanStore = useLanStore();
const connection = useConnectionLabel();

const open = ref(false);
const current = ref<string | null>(null);
const libraries = ref<LibraryHistoryItem[]>([]);
/** 远程素材库（服务端池内其他设备；仅远程接入在线时拉取） */
const peers = ref<RemoteDevice[]>([]);
/** 已配对的局域网素材库（不经服务器） */
const lanPeers = ref<LanPeerProfile[]>([]);
const triggerRef = ref<HTMLElement | null>(null);
const panelRef = ref<HTMLElement | null>(null);
const pos = ref({ x: 0, y: 0, width: 0 });
/** 浮层与触发按钮的间距 */
const PANEL_GAP = 4;
/** 每次打开递增：快速开关时旧请求的结果直接丢弃，不覆盖新一轮的数据 */
let loadSeq = 0;

watch(open, (visible) => {
  if (!visible) {
    unbindOutside();
    return;
  }
  bindOutside();
  // 定位与数据加载解耦：先挂出面板再去拉数据——远程设备列表要绕 hawk-server 一圈（服务端
  // 不可达时能磨到超时），等它回来再定位会让浮层先在旧坐标/零尺寸上错位一下
  void placePanel();
  void loadPanelData();
});

function close(): void {
  open.value = false;
}

/** 浮层数据：历史库（IPC，快）+ 远程分区（需拉服务端设备列表，慢）。任一步失败不拖塌浮层，
 *  远程分区缺席即可（接入未启用/离线也不占面板） */
async function loadPanelData(): Promise<void> {
  const seq = ++loadSeq;
  try {
    const res = await shell.listLibraries();
    if (seq !== loadSeq) {
      return;
    }
    current.value = res.current;
    libraries.value = res.libraries;
    try {
      lanPeers.value = (await shell.lanPeers()).peers;
    } catch {
      lanPeers.value = [];
    }
    const status = await shell.remoteStatus();
    if (seq !== loadSeq) {
      return;
    }
    if (!status.enabled || status.status.state !== 'online') {
      peers.value = [];
      return;
    }
    // 本机设备不在其列
    const { devices } = await shell.remoteDevices();
    if (seq !== loadSeq) {
      return;
    }
    peers.value = devices.filter((d) => d.device_id !== status.status.device_id);
  } catch {
    if (seq === loadSeq) {
      peers.value = [];
    }
  }
}

/** 锚点定位：位置在面板挂载前同步写入（下方展开），挂载后按实际高度决定是否向上翻转。
 *  期间关闭过就直接放弃——面板已卸载，不碰 null 引用 */
async function placePanel(): Promise<void> {
  const trigger = triggerRef.value;
  if (!trigger) {
    return;
  }
  const rect = trigger.getBoundingClientRect();
  pos.value = { x: rect.left, y: rect.bottom + PANEL_GAP, width: rect.width };
  await nextTick();
  const panel = panelRef.value;
  if (!panel || !open.value) {
    return;
  }
  const height = panel.offsetHeight;
  if (rect.bottom + PANEL_GAP + height > window.innerHeight && rect.top - PANEL_GAP - height > 0) {
    pos.value = { x: rect.left, y: rect.top - PANEL_GAP - height, width: rect.width };
  }
}

/** 面板开着才应答的外界事件：改窗/侧栏滚动关闭，Esc 关闭，点外部关闭 */
function bindOutside(): void {
  window.addEventListener('resize', close);
  window.addEventListener('scroll', onScroll, true);
  document.addEventListener('mousedown', onOutside, true);
  document.addEventListener('keydown', onKeydown, true);
}

function unbindOutside(): void {
  window.removeEventListener('resize', close);
  window.removeEventListener('scroll', onScroll, true);
  document.removeEventListener('mousedown', onOutside, true);
  document.removeEventListener('keydown', onKeydown, true);
}

/** 当前远端浏览的写能力（徽标悬停提示；只读/仅导入/可读写对应 store 的两道门禁） */
const remoteMode = computed(() => (store.viewerMode ? '只读' : store.limitedWrite ? '仅导入' : '可读写'));

/** 触发器悬停提示：远端/局域网浏览时给来源、设备名与写能力（来源区分落在本按钮上） */
const triggerTitle = computed(() =>
  connection.isPeer.value
    ? `${connection.isLan.value ? '局域网直连' : '远程浏览'}：${connection.label.value}（${remoteMode.value}）· 点击切换素材库`
    : `${store.library?.path ?? ''}（点击切换素材库）`,
);

/** 断开远程会话回本地库：徽标取代了原顶栏标识，断开入口收进浮层顶部（同点当前本地库的效果） */
async function disconnectRemote(): Promise<void> {
  close();
  await connection.disconnect();
}

/** 侧栏滚动时浮层不跟随，直接关闭（浮层自身滚动除外：scroll 捕获阶段也会经过 window） */
function onScroll(event: Event): void {
  if (panelRef.value?.contains(event.target as Node)) {
    return;
  }
  close();
}

function onOutside(event: Event): void {
  const target = event.target as Node;
  if (triggerRef.value?.contains(target) || panelRef.value?.contains(target)) {
    return;
  }
  // 右键菜单上的点按（移除历史项等）不算外部：面板保持，连续处理多条记录
  if (target instanceof Element && target.closest('[data-context-menu]')) {
    return;
  }
  close();
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    event.stopPropagation();
    close();
  }
}

/** 切换素材库：仅对存在的非当前库生效（已删除项置灰）；
 *  远程浏览中点本地当前库 = 断开远程会话回切 */
function switchTo(lib: LibraryHistoryItem): void {
  if (!lib.exists) {
    return;
  }
  if (lib.path === current.value) {
    if (connection.isPeer.value) {
      close();
      void connection.disconnect();
    }
    return;
  }
  close();
  void shell.openLibrary(lib.path);
}

/** 连接远程素材库（复用远程 store：直连失败按「中继回退」偏好询问）；失败 toast 展示 */
/** 连接已配对的局域网设备；失败在 toast 中展示，不阻塞浮层关闭 */
async function connectLan(peer: LanPeerProfile): Promise<void> {
  if (connection.isLan.value && connection.peerId.value === peer.id) {
    return;
  }
  close();
  await lanStore.connect(peer.id);
  if (lanStore.error) {
    store.showToast(lanStore.error);
    lanStore.error = null;
  }
}

async function connectRemote(device: RemoteDevice): Promise<void> {
  if (!device.online || !device.allow_remote || device.device_id === connection.deviceId.value) {
    return;
  }
  close();
  await remoteStore.connect(device.device_id);
  if (remoteStore.error) {
    store.showToast(remoteStore.error);
    remoteStore.error = null;
  }
}

/** 从历史移除一条记录（只删记录不动目录；当前库不可移除），浮层原地刷新 */
async function remove(lib: LibraryHistoryItem): Promise<void> {
  const res = await shell.removeLibrary(lib.path);
  current.value = res.current;
  libraries.value = res.libraries;
}

/** 条目右侧 `···`：存在库可打开目录；当前库可重命名（改名走 daemon API，非当前库无进程服务）；
 *  其余条目（含目录已删除的）可移除历史记录 */
function openItemMenu(lib: LibraryHistoryItem, e: MouseEvent): void {
  const anchor = e.currentTarget as HTMLElement;
  const items: MenuItem[] = [];
  if (lib.exists) {
    items.push({ label: '打开素材库文件夹', action: () => void shell.openLibraryFolder(lib.path) });
  }
  if (lib.path === current.value) {
    items.push({
      label: '重命名',
      // 面板先收起：改名弹窗单独存在，不压在浮层上方
      action: () => {
        close();
        renameTarget.value = lib.path;
      },
    });
  } else {
    items.push({ label: '从列表中移除', danger: true, action: () => void remove(lib) });
  }
  menu.open(items, e, anchor);
}

/** 重命名目标库路径（非空即弹输入框） */
const renameTarget = ref('');

/** 提交改名：走 daemon PATCH /library/info，成功后就地更新浮层条目；其他端经 library.updated 事件对齐 */
async function submitRename(name: string): Promise<void> {
  const path = renameTarget.value;
  renameTarget.value = '';
  if (path !== current.value) {
    return;
  }
  if (name !== store.library?.name) {
    const ok = await renameLibrary(name);
    if (!ok) {
      return;
    }
    // 浮层开着时就地改条目；重开浮层经 listLibraries 重拉兜底
    libraries.value = libraries.value.map((l) => (l.path === path ? { ...l, name } : l));
  }
}

/** 底部「打开文件夹…」：弹系统目录选择框加入新库 */
function pickFolder(): void {
  close();
  void shell.selectLibrary();
}

onBeforeUnmount(() => {
  unbindOutside();
});
</script>

<template>
  <!-- 单根 button：父 scoped 的 .library-name 规则与 in-head/in-body 显隐类均依赖根元素继承；
       Teleport 置于 button 内（不产生实际 DOM），仅为保住单根结构 -->
  <button ref="triggerRef" class="library-name" :class="{ open }" :title="triggerTitle" @click="open = !open" @dblclick.stop>
    <Icon name="library" />
    <span class="lib-text">{{ store.library?.name ?? 'hawk' }}</span>
    <!-- 远程浏览标识：远端库与本地库在视觉上必须可区分（安全与认知底线），悬停提示带写能力 -->
    <span v-if="connection.isPeer.value" class="badge">{{ connection.isLan.value ? '局域网' : '远程' }}</span>
    <Icon name="chevronDown" :size="12" class="lib-chev" />
    <Teleport to="body">
      <div v-if="open" ref="panelRef" class="lib-panel" :style="{ left: pos.x + 'px', top: pos.y + 'px', width: pos.width + 'px' }">
        <!-- 远程浏览中：显式断开入口（回到本地库，徽标随之消失） -->
        <template v-if="connection.isPeer.value">
          <button type="button" class="lib-item" :title="`当前浏览：${connection.label.value}（${remoteMode}）`" @click="disconnectRemote">
            <span class="check" />
            <span class="label">断开{{ connection.isLan.value ? '局域网连接' : '远程' }} · 回到我的库</span>
            <Icon name="close" :size="12" />
          </button>
          <div class="separator" />
        </template>
        <button
          v-for="lib in libraries"
          :key="lib.path"
          type="button"
          class="lib-item"
          :class="{ selected: lib.path === current && !connection.isPeer.value, disabled: !lib.exists }"
          :title="lib.path"
          @click="switchTo(lib)"
        >
          <span class="check">{{ lib.path === current && !connection.isPeer.value ? '✓' : '' }}</span>
          <span class="label">{{ lib.exists ? lib.name : `${lib.name}（已删除）` }}</span>
          <button type="button" class="more" title="更多操作" @click.stop="openItemMenu(lib, $event)">···</button>
        </button>
        <!-- 局域网素材库：已配对设备，直连不经服务器（与远程分区用徽标区分来源） -->
        <template v-if="lanPeers.length > 0">
          <div class="separator" />
          <button
            v-for="peer in lanPeers"
            :key="peer.id"
            type="button"
            class="lib-item"
            :class="{ selected: connection.isLan.value && peer.id === connection.peerId.value }"
            :title="`${peer.name} · ${peer.library_name} · ${peer.url}`"
            @click="connectLan(peer)"
          >
            <span class="check">{{ connection.isLan.value && peer.id === connection.peerId.value ? '✓' : '' }}</span>
            <span class="label">{{ peer.name }}（{{ peer.library_name }}）{{ lanStore.connectingId === peer.id ? ' 连接中…' : '' }}</span>
            <span class="badge">局域网</span>
          </button>
        </template>
        <!-- 远程素材库：池内其他设备，一键切换浏览（「远程」徽标区分本地历史） -->
        <template v-if="peers.length > 0">
          <div class="separator" />
          <button
            v-for="device in peers"
            :key="device.device_id"
            type="button"
            class="lib-item"
            :class="{
              selected: device.device_id === connection.deviceId.value,
              disabled: !device.online || !device.allow_remote || device.device_id === connection.deviceId.value,
            }"
            :title="`${device.name} · ${device.library_name} · ${device.platform}${device.online ? (device.allow_remote ? '' : ' · 已关闭访问') : ' · 离线'}`"
            @click="connectRemote(device)"
          >
            <span class="check">{{ device.device_id === connection.deviceId.value ? '✓' : '' }}</span>
            <span class="dot" :class="{ online: device.online }" />
            <span class="label">{{ device.name }}（{{ device.library_name }}）{{ remoteStore.connectingId === device.device_id ? ' 连接中…' : '' }}</span>
            <span class="badge">远程</span>
          </button>
        </template>
        <div class="separator" />
        <button type="button" class="lib-item" @click="pickFolder">
          <span class="check" />
          <span class="label">打开文件夹…</span>
        </button>
      </div>
    </Teleport>
    <!-- Teleport 不产生实际 DOM：置于 button 内保住单根结构（父 scoped 显隐类依赖根元素继承） -->
    <PromptDialog
      v-if="renameTarget"
      title="重命名素材库"
      placeholder="素材库名称"
      :default-value="libraries.find((l) => l.path === renameTarget)?.name"
      :dismiss-on-mask="false"
      @confirm="submitRename"
      @cancel="renameTarget = ''"
    />
  </button>
</template>

<style scoped>
/* 「远程」徽标：面板内远程设备行与触发按钮共用（触发按钮上标明当前浏览的是远端库） */
.badge {
  flex: none;
  padding: 1px 6px;
  border-radius: 4px;
  font-size: 11px;
  line-height: 1.4;
  color: var(--accent);
  border: 1px solid var(--accent);
}

/* 触发按钮的尺寸/间距由 Sidebar 的 .library-name 规则承担（父 scoped 可作用到子组件根元素）；
   本文件只补下拉态与浮层 */
.library-name.open {
  background: var(--bg-2);
}

.lib-chev {
  transition: transform 0.15s;
}

.library-name.open .lib-chev {
  transform: rotate(180deg);
}

.lib-panel {
  position: fixed;
  /* 设置对话框 170 / 预览浮层 200 / 右键菜单 400；本浮层须盖过侧栏与顶栏 */
  z-index: 300;
  max-height: 280px;
  overflow-y: auto;
  padding: 4px;
  border-radius: 6px;
  background: var(--bg-3);
  border: 1px solid var(--border);
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.5);
  display: flex;
  flex-direction: column;
}

.lib-item {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 6px 12px;
  border: none;
  border-radius: 4px;
  background: transparent;
  text-align: left;
}

@media (hover: hover) {
  .lib-item:hover {
    background: color-mix(in srgb, var(--accent) 35%, transparent);
  }
}

.lib-item.disabled {
  opacity: 0.45;
  cursor: default;
}

.lib-item .check {
  width: 12px;
  flex: none;
  color: var(--accent);
}

.lib-item .label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 项右侧 `···` 按钮：默认隐去，悬停该行时浮现，避免整列按钮造成视觉噪音 */
.lib-item .more {
  flex: none;
  margin-right: -6px;
  padding: 0 4px;
  font-size: 12px;
  line-height: 1;
  letter-spacing: 1px;
  color: var(--fg-1);
  visibility: hidden;
}

@media (hover: hover) {
  .lib-item:hover .more {
    visibility: visible;
  }
}

@media (hover: hover) {
  .lib-item .more:hover {
    color: var(--accent);
  }
}

.separator {
  height: 1px;
  margin: 4px 8px;
  background: var(--border);
}

/* 远程设备行的在线点（与设置面板设备列表同款） */
.lib-item .dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-tertiary, #999);
  flex: none;
}

.lib-item .dot.online {
  background: #4caf50;
}
</style>
