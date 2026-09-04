<script setup lang="ts">
// 设置面板壳：左侧导航 + 右侧分区（子组件）的两栏结构（窄屏折叠为顶部横向页签）。
// 分区实现各自独立：SettingsAppearance（外观，实时生效）/ SettingsLan（局域网，仅 Electron，
// 状态自管 + 暴露 save 给 footer 委托调用）/ SettingsUpdate（更新，仅 Electron）/
// SettingsConnection（连接，仅局域网 web 端）。分区切换用 v-show 保活：LAN 编辑中字段与
// 更新下载进度在切换分区后不丢。
// 交互要点：
// - 遮罩「按下与抬起都落在遮罩上」才关闭：在端口输入框里拖动选择文本、拖动滑杆时滑出面板松开，
//   click 事件落在 mousedown/mouseup 目标的共同祖先（遮罩）上，按 click.self 判定会误关面板丢失未保存
//   的配置——改用 pointerdown/pointerup 配对判定，从面板内开始的拖拽不再触发关闭。
// - Esc 关闭（捕获阶段拦截并阻断全局快捷键；IME 组合态已被 main.ts 更早的捕获监听拦下）。
// - 打开期间挂 body.dialog-open 挂起窗口拖拽区（同 ContextMenu 的 body.menu-open）：Electron 的
//   -webkit-app-region: drag 由 OS 命中测试优先消费，不禁用的话点遮罩盖住的标题栏会变成拖动窗口。
import { onMounted, onUnmounted, ref } from 'vue';
import { useEventListener } from '@vueuse/core';
import { hasShell } from '../platform';
import Icon from './Icon.vue';
import SettingsAppearance from './SettingsAppearance.vue';
import SettingsLan from './SettingsLan.vue';
import SettingsUpdate from './SettingsUpdate.vue';
import SettingsConnection from './SettingsConnection.vue';

const emit = defineEmits<{ close: []; logout: [] }>();

/** 当前分区：外观 / 局域网（Electron）/ 更新（Electron）/ 连接（局域网 web 端，含 token 注销） */
const section = ref<'appearance' | 'lan' | 'update' | 'connection'>('appearance');
/** 错误条（各分区经 v-model:error 写入；LAN 分区的读取/校验/保存错误） */
const error = ref<string | null>(null);

/** LAN 分区实例（v-show 保活，footer 保存按钮委托其 save；外观/更新分区无保存语义） */
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
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      emit('close');
    }
  },
  { capture: true },
);

/** 保存：委托 LAN 分区（设置面板唯一有保存语义的分区）；失败切到局域网分区暴露错误字段 */
async function save() {
  if (!lanPane.value) {
    return;
  }
  const ok = await lanPane.value.save();
  if (ok) {
    emit('close');
  } else if (section.value !== 'lan') {
    section.value = 'lan';
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
          <!-- 左侧导航：桌面（外观/局域网/更新）与局域网 web 端（外观/连接）；窄屏折叠为顶部横向页签 -->
          <nav class="nav">
            <button class="nav-item" :class="{ active: section === 'appearance' }" @click="section = 'appearance'">
              外观
            </button>
            <button
              v-if="hasShell"
              class="nav-item"
              :class="{ active: section === 'lan' }"
              @click="section = 'lan'"
            >
              局域网
            </button>
            <button
              v-if="hasShell"
              class="nav-item"
              :class="{ active: section === 'update' }"
              @click="section = 'update'"
            >
              更新
            </button>
            <button
              v-else
              class="nav-item"
              :class="{ active: section === 'connection' }"
              @click="section = 'connection'"
            >
              连接
            </button>
          </nav>

          <!-- 分区保活挂载（v-show）：LAN 字段编辑与更新下载进度切分区不丢；web 端不挂载 LAN/更新 -->
          <SettingsAppearance v-show="section === 'appearance'" />
          <SettingsLan v-if="hasShell" v-show="section === 'lan'" ref="lanPane" v-model:error="error" />
          <SettingsUpdate v-if="hasShell" v-show="section === 'update'" />
          <SettingsConnection v-if="!hasShell" v-show="section === 'connection'" @logout="emit('logout')" />
        </div>

        <div v-if="error" class="dialog-error">{{ error }}</div>

        <footer class="actions">
          <button :disabled="lanBusy()" @click="emit('close')">{{ hasShell ? '取消' : '关闭' }}</button>
          <button v-if="hasShell" class="primary" :disabled="lanBusy()" @click="save">保存</button>
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

.nav-item:hover {
  background: var(--bg-3);
  color: var(--fg-0);
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
