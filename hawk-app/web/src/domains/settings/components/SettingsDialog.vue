<script setup lang="ts">
// 设置面板壳：左侧导航 + 右侧分区（子组件）的两栏结构（窄屏折叠为顶部横向页签）。
// 分区实现各自独立：SettingsAppearance（外观，实时生效）/ 网络分区内再分三个标签页
// （本机服务 / 局域网访问 / 远程访问）/ SettingsStorage / SettingsUpdate（仅 Electron）/
// SettingsConnection（连接，仅局域网 web 端）。外层分区与网络内标签都用 v-show 保活：
// LAN 编辑中字段、发现结果与更新下载进度在切换后不丢。
// 交互要点：
// - 遮罩「按下与抬起都落在遮罩上」才关闭：在端口输入框里拖动选择文本、拖动滑杆时滑出面板松开，
//   click 事件落在 mousedown/mouseup 目标的共同祖先（遮罩）上，按 click.self 判定会误关面板丢失未保存
//   的配置——改用 pointerdown/pointerup 配对判定，从面板内开始的拖拽不再触发关闭。
// - Esc 关闭（捕获阶段拦截并阻断全局快捷键；IME 组合态已被 main.ts 更早的捕获监听拦下）。
// - 打开期间挂 body.dialog-open 挂起窗口拖拽区（同 ContextMenu 的 body.menu-open）：Electron 的
//   -webkit-app-region: drag 由 OS 命中测试优先消费，不禁用的话点遮罩盖住的标题栏会变成拖动窗口。
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useEventListener } from '@vueuse/core';
import { hasShell } from '@/shared/lib/platform';
import { useLibraryStore } from '@/domains/library';
import Icon from '@/shared/ui/Icon.vue';
import SettingsAppearance from './SettingsAppearance.vue';
import SettingsHiding from './SettingsHiding.vue';
import SettingsLan from './SettingsLan.vue';
import SettingsLanConnect from './SettingsLanConnect.vue';
import SettingsLanPairActions from './SettingsLanPairActions.vue';
import SettingsAdvanced from './SettingsAdvanced.vue';
import SettingsRemote from './SettingsRemote.vue';
import SettingsStorage from './SettingsStorage.vue';
import SettingsUpdate from './SettingsUpdate.vue';
import SettingsConnection from './SettingsConnection.vue';

const emit = defineEmits<{ close: []; logout: [] }>();

/** 当前分区：外观 / 隐藏项 / 网络 / 存储 / 更新 / 连接（局域网 web 端，含 token 注销） */
const section = ref<'appearance' | 'hiding' | 'network' | 'storage' | 'update' | 'connection'>('appearance');

/** 网络分区内的二级标签：局域网访问 / 远程访问 / 高级（端口等低频配置放最右侧） */
type NetworkTab = 'lan-access' | 'remote' | 'advanced';
const networkTab = ref<NetworkTab>('lan-access');
const networkTabs = computed<{ id: NetworkTab; label: string }[]>(() => [
  { id: 'lan-access', label: '局域网访问' },
  { id: 'remote', label: '远程访问' },
  { id: 'advanced', label: '高级' },
]);
const visibleNetworkTabs = computed(() => networkTabs.value);

function selectNetworkTab(tab: NetworkTab): void {
  networkTab.value = tab;
  // 错误条可能来自上一个标签页；切 tab 时清掉，避免在无关页面看到旧错误
  error.value = null;
}
/** 错误条（各分区经 v-model:error 写入；LAN 分区的读取/校验/保存错误） */
const error = ref<string | null>(null);

const store = useLibraryStore();
/** 当前连接是否 admin：远程浏览/局域网 viewer 下隐藏 admin 限定内容（局域网块），
 *  避免只读 token 调管理端点 403（viewer token cannot access admin settings） */
const isAdmin = computed(() => store.access === 'admin');

/** 「局域网访问」里的共享设置需要 footer 保存；访问其他设备、本机端口与远程均为内联操作 */
const showLanSave = computed(() => hasShell && isAdmin.value && section.value === 'network' && networkTab.value === 'lan-access');
const closeLabel = computed(() => (showLanSave.value ? '取消' : '关闭'));

/** 「网络 → 局域网访问 → 共享本机素材库」块实例（v-show 保活，footer 保存按钮委托其 save） */
const lanPane = ref<InstanceType<typeof SettingsLan> | null>(null);
/** footer 按钮禁用：LAN 分区加载/保存中 */
function lanBusy(): boolean {
  return lanPane.value?.busy() ?? false;
}

onMounted(() => {
  document.body.classList.add('dialog-open');
});
onUnmounted(() => {
  document.body.classList.remove('dialog-open');
});

// ---- 遮罩关闭：按下与抬起都落在遮罩上才关（面板内开始的拖拽出面板松开不误关） ----
let downOnMask = false;

function onMaskDown(e: PointerEvent) {
  downOnMask = e.target === e.currentTarget;
}

function onMaskUp(e: PointerEvent) {
  if (downOnMask && e.target === e.currentTarget) {
    emit('close');
  }
  downOnMask = false;
}

// Esc 关闭：捕获阶段处理并阻断冒泡，避免全局快捷键（关预览/菜单）跟着触发
useEventListener(
  window,
  'keydown',
  (e) => {
    // 配对二维码弹层在上层自己处理 Esc；不能同时关掉底层设置面板
    if (document.body.classList.contains('pairing-open')) {
      return;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      emit('close');
    }
  },
  { capture: true },
);

/** 保存：委托 LAN 块（footer 保存的唯一语义所在）；失败切到网络分区暴露错误字段 */
async function save() {
  if (!lanPane.value) {
    return;
  }
  const ok = await lanPane.value.save();
  if (ok) {
    emit('close');
  } else {
    section.value = 'network';
    networkTab.value = 'lan-access';
  }
}
</script>

<template>
  <Teleport to="body">
    <div class="mask" @pointerdown="onMaskDown" @pointerup="onMaskUp">
      <div class="dialog" role="dialog" aria-modal="true" aria-label="设置">
        <header class="dialog-head">
          <span class="dialog-title">设置</span>
          <button class="icon-btn" title="关闭 (Esc)" @click="emit('close')">
            <Icon name="close" :size="14" />
          </button>
        </header>

        <div class="dialog-main">
          <!-- 左侧导航：桌面（外观/隐藏项/网络/存储/更新）与局域网 web 端（外观/隐藏项/连接）；
               窄屏折叠为顶部横向页签 -->
          <nav class="nav">
            <button class="nav-item" :class="{ active: section === 'appearance' }" @click="section = 'appearance'">外观</button>
            <button class="nav-item" :class="{ active: section === 'hiding' }" @click="section = 'hiding'">隐藏项</button>
            <button v-if="hasShell" class="nav-item" :class="{ active: section === 'network' }" @click="section = 'network'">网络</button>
            <button v-if="hasShell" class="nav-item" :class="{ active: section === 'storage' }" @click="section = 'storage'">存储</button>
            <button v-if="hasShell" class="nav-item" :class="{ active: section === 'update' }" @click="section = 'update'">更新</button>
            <button v-else class="nav-item" :class="{ active: section === 'connection' }" @click="section = 'connection'">连接</button>
          </nav>

          <!-- 分区保活挂载（v-show）：网络分区的字段编辑与更新下载进度切分区不丢；web 端不挂载网络/更新 -->
          <SettingsAppearance v-show="section === 'appearance'" />
          <SettingsHiding v-show="section === 'hiding'" />
          <!-- 网络：二级标签隔离四块配置，避免本机端口 / LAN 共享 / LAN 互联 / 远程访问堆在一页 -->
          <div v-if="hasShell" v-show="section === 'network'" class="pane network-pane">
            <div class="subnav" role="tablist" aria-label="网络设置">
              <button
                v-for="tab in visibleNetworkTabs"
                :key="tab.id"
                type="button"
                role="tab"
                class="subnav-item"
                :class="{ active: networkTab === tab.id }"
                :aria-selected="networkTab === tab.id"
                @click="selectNetworkTab(tab.id)"
              >
                {{ tab.label }}
              </button>
            </div>
            <div v-show="networkTab === 'lan-access'" class="tab-content lan-access" role="tabpanel">
              <h3 class="section-title">访问其他设备</h3>
              <SettingsLanConnect />
              <SettingsLanPairActions v-if="isAdmin" />
              <template v-if="isAdmin">
                <h3 class="section-title">局域网网站访问</h3>
                <SettingsLan ref="lanPane" v-model:error="error" embedded />
              </template>
            </div>
            <div v-show="networkTab === 'remote'" class="tab-content" role="tabpanel">
              <SettingsRemote v-model:error="error" embedded />
            </div>
            <div v-show="networkTab === 'advanced'" class="tab-content" role="tabpanel">
              <SettingsAdvanced />
            </div>
          </div>
          <SettingsStorage v-if="hasShell" v-show="section === 'storage'" />
          <SettingsUpdate v-if="hasShell" v-show="section === 'update'" />
          <SettingsConnection v-if="!hasShell" v-show="section === 'connection'" @logout="emit('logout')" />
        </div>

        <div v-if="error" class="dialog-error">{{ error }}</div>

        <footer class="actions">
          <button :disabled="showLanSave && lanBusy()" @click="emit('close')">{{ closeLabel }}</button>
          <button v-if="showLanSave" class="primary" :disabled="lanBusy()" @click="save">保存网站设置</button>
        </footer>
      </div>
    </div>
  </Teleport>
</template>

<style src="./settings-shared.css"></style>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 170;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.5);
}

/* 标题栏/底部按钮常驻，只有内容区滚动。
   高度固定（钳制视口）：分区切换/开关展开细节/错误条出现只改变内容区滚动，
   面板尺寸不变，避免界面跳跃 */
.dialog {
  width: min(560px, calc(100vw - 32px));
  height: min(520px, 86vh);
  display: flex;
  flex-direction: column;
  border-radius: 10px;
  background: var(--bg-1);
  border: 1px solid var(--border);
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.4);
  overflow: hidden;
}

.dialog-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 12px 10px 16px;
  border-bottom: 1px solid var(--border);
}

.dialog-title {
  font-weight: 600;
}

/* ---- 左导航 + 右内容；窄屏折叠为顶部横向页签 ---- */
.dialog-main {
  flex: 1;
  min-height: 0;
  display: flex;
}

.nav {
  flex: none;
  width: 112px;
  padding: 10px 8px;
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.nav-item {
  text-align: left;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--fg-1);
  padding: 6px 10px;
}

@media (hover: hover) {
  .nav-item:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
}

.nav-item.active {
  background: var(--bg-3);
  color: var(--fg-0);
  font-weight: 600;
}

.dialog-error {
  padding: 8px 16px;
  border-top: 1px solid var(--border);
  color: var(--danger);
  font-size: 12px;
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--border);
}

.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}

/* ---- 网络分区内的二级标签：独立滚动的仍是 .pane，标签是内容区首行 ---- */
.network-pane {
  gap: 12px;
  /* 手动配对展开触发纵向滚动时预留滚动条宽度，避免内容区横向抖动 */
  scrollbar-gutter: stable;
}

.subnav {
  flex: none;
  display: flex;
  gap: 18px;
  padding: 0 2px;
  border-bottom: 1px solid var(--border);
  overflow-x: auto;
  overflow-y: hidden;
  /* 标签不足一屏时不要出现滚动条；窄屏仍可横向滚动 */
  scrollbar-width: none;
}

.subnav::-webkit-scrollbar {
  display: none;
}

.subnav-item {
  flex: none;
  margin-bottom: -1px;
  padding: 2px 0 8px;
  border: none;
  border-bottom: 2px solid transparent;
  border-radius: 0;
  background: transparent;
  color: var(--fg-1);
  font-size: 12px;
  white-space: nowrap;
}

@media (hover: hover) {
  .subnav-item:hover {
    color: var(--fg-0);
  }
}

.subnav-item.active {
  color: var(--fg-0);
  border-bottom-color: var(--accent);
  font-weight: 600;
}

.tab-content {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.lan-access .section-title {
  padding-top: 10px;
  font-size: 12px;
  color: var(--fg-1);
}

.lan-access .section-title:first-child {
  padding-top: 0;
}

/* 窄屏（手机竖屏）：导航折叠为顶部横向页签，内容占满剩余宽度 */
@media (max-width: 520px) {
  .dialog-main {
    flex-direction: column;
  }

  .nav {
    width: auto;
    flex-direction: row;
    border-right: none;
    border-bottom: 1px solid var(--border);
    padding: 8px 10px;
  }

  .nav-item {
    flex: 1;
    text-align: center;
  }
}
</style>
