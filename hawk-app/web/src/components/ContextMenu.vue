<script setup lang="ts">
import { nextTick, ref, watch } from 'vue';
import { useContextMenu } from '../composables/useContextMenu';

const { state, close } = useContextMenu();

const menuRef = ref<HTMLElement | null>(null);
const pos = ref({ x: 0, y: 0 });

// 打开后按菜单实际尺寸防出屏翻转
watch(
  () => state.visible,
  async (visible) => {
    if (!visible) {
      return;
    }
    pos.value = { x: state.x, y: state.y };
    await nextTick();
    const el = menuRef.value;
    if (!el) {
      return;
    }
    const rect = el.getBoundingClientRect();
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
    <div v-if="state.visible" class="mask" @click="close" @contextmenu.prevent="close">
      <div ref="menuRef" class="menu" :style="{ left: pos.x + 'px', top: pos.y + 'px' }" @click.stop>
        <template v-for="(item, i) in state.items" :key="i">
          <div v-if="item.separator" class="separator" />
          <button
            v-else
            class="item"
            :class="{ danger: item.danger }"
            @click="
              close();
              item.action?.();
            "
          >
            {{ item.label }}
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
  text-align: left;
  padding: 6px 12px;
  border: none;
  border-radius: 4px;
  background: transparent;
}

.item.danger {
  color: var(--danger);
}

.separator {
  height: 1px;
  margin: 4px 8px;
  background: var(--border);
}
</style>
