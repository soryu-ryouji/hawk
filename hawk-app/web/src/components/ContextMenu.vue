<script setup lang="ts">
import { nextTick, onUnmounted, ref, watch } from 'vue';
import { CONTEXT_MENU_OPEN_GUARD_MS, useContextMenu } from '../composables/useContextMenu';

const { state, close } = useContextMenu();

/** 开场守卫：打开后 OPEN_GUARD_MS 内的遮罩 click/contextmenu 忽略——长按开菜单后松手跟发的
 *  click/contextmenu（Android 原生长按、指针捕获回投）落在遮罩上，不挡会把刚开的菜单秒关。
 *  人手不可能在 250ms 内点到遮罩，正常「点空白关菜单」不受影响 */
function canClose(): boolean {
  return Date.now() - state.openedAt >= CONTEXT_MENU_OPEN_GUARD_MS;
}

function onMaskClick(): void {
  if (canClose()) {
    close();
  }
}

const menuRef = ref<HTMLElement | null>(null);
const pos = ref({ x: 0, y: 0 });

/**
 * 菜单打开期间挂起窗口拖拽区（body.menu-open，全局样式把顶栏/侧栏/检查器顶条改为 no-drag）。
 * Electron 的 -webkit-app-region: drag 由 OS 命中测试优先消费：浮层遮罩盖在拖拽区上也收不到点击，
 * 点击空白会被当成窗口拖动，菜单无法关闭——禁用拖拽区后点击空白处才能正常关菜单（不选 = 保持不变）。
 */
watch(
  () => state.visible,
  (visible) => {
    document.body.classList.toggle('menu-open', visible);
  },
);

onUnmounted(() => {
  document.body.classList.remove('menu-open');
});

// 打开后按菜单实际尺寸防出屏翻转
watch(
  () => state.visible,
  async (visible) => {
    if (!visible) {
      return;
    }
    const anchor = state.anchor;
    // 锚点模式（按钮触发的下拉）：先放锚点下方，量完尺寸再修正出屏；鼠标模式（右键）：从点击点展开
    pos.value = anchor ? { x: anchor.left, y: anchor.bottom + 4 } : { x: state.x, y: state.y };
    await nextTick();
    const el = menuRef.value;
    if (!el) {
      return;
    }
    const rect = el.getBoundingClientRect();
    if (anchor) {
      // 水平：默认左对齐锚点左缘；右侧出屏改右对齐锚点右缘（顶栏右侧按钮场景）
      let x = anchor.left;
      if (x + rect.width > window.innerWidth) {
        x = Math.max(0, anchor.right - rect.width);
      }
      // 垂直：下方空间不足翻到锚点上方
      let y = anchor.bottom + 4;
      if (y + rect.height > window.innerHeight) {
        y = Math.max(0, anchor.top - rect.height - 4);
      }
      pos.value = { x, y };
      return;
    }
    if (state.x + rect.width > window.innerWidth) {
      pos.value = { ...pos.value, x: Math.max(0, state.x - rect.width) };
    }
    if (state.y + rect.height > window.innerHeight) {
      pos.value = { ...pos.value, y: Math.max(0, state.y - rect.height) };
    }
  },
);
</script>

<template>
  <Teleport to="body">
    <div v-if="state.visible" class="mask" @click="onMaskClick" @contextmenu.prevent="onMaskClick">
      <div ref="menuRef" class="menu" data-context-menu="" :style="{ left: pos.x + 'px', top: pos.y + 'px' }" @click.stop>
        <template v-for="(item, i) in state.items" :key="i">
          <div v-if="item.separator" class="separator" />
          <button
            v-else
            class="item"
            :class="{ danger: item.danger, checked: item.checked }"
            :disabled="item.disabled"
            :title="item.title"
            @click="
              close();
              item.action?.();
            "
          >
            <span class="check">{{ item.checked ? '✓' : '' }}</span>
            <span class="label">{{ item.label }}</span>
          </button>
        </template>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  /* 预览浮层是 200，右键菜单必须更高 */
  z-index: 400;
}

.menu {
  position: fixed;
  min-width: 160px;
  padding: 4px;
  border-radius: 6px;
  background: var(--bg-3);
  border: 1px solid var(--border);
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.5);
  display: flex;
  flex-direction: column;
}

.item {
  display: flex;
  align-items: center;
  gap: 6px;
  text-align: left;
  padding: 6px 12px;
  border: none;
  border-radius: 4px;
  background: transparent;
}

.item .check {
  width: 12px;
  flex: none;
  color: var(--accent);
}

.item.checked .label {
  color: var(--fg-0);
}

.item.danger {
  color: var(--danger);
}

.item:disabled {
  opacity: 0.45;
  cursor: default;
}

.separator {
  height: 1px;
  margin: 4px 8px;
  background: var(--border);
}
</style>
