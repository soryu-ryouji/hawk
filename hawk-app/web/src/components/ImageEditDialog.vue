<script setup lang="ts">
// 图片编辑窗口:网格/预览浮层右键「编辑图片…」打开(store.openEditor)。
// 全屏 Eagle 式遮罩(观感同预览浮层),底部工具条提供旋转与保存。
// 编辑在关闭前只作用于预览角(CSS 变换);「保存」或带修改退出(保存/不保存/取消三选确认)
// 才经 store.saveImageEdit 做客户端重编码(canvas + JPEG EXIF 回填)并提交 item/replace。
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { api } from '../api/endpoints';
import { useLibraryStore } from '../stores/library';
import type { Item } from '../types';

const props = defineProps<{ item: Item }>();
const emit = defineEmits<{ close: [] }>();

const store = useLibraryStore();

// 累计旋转角(顺时针),保存时才做像素重编码;编辑期间仅 CSS 变换预览
const angle = ref(0);
const saving = ref(false);
// 退出确认态:有未保存修改时,关闭请求先进三选确认(保存/不保存/取消)
const confirming = ref(false);

const dirty = computed(() => angle.value !== 0);
const imageStyle = computed(() => ({ transform: `rotate(${angle.value}deg)` }));
const imageUrl = computed(() => api.fileUrl(props.item.id));

function rotate(step: 90 | -90) {
  angle.value = (angle.value + step + 360) % 360;
}

async function save(): Promise<boolean> {
  if (saving.value) {
    return false;
  }
  saving.value = true;
  try {
    return await store.saveImageEdit(props.item.id, angle.value as 90 | 180 | 270);
  } finally {
    saving.value = false;
  }
}

// 保存并关闭;无修改时关闭即可
async function saveAndClose() {
  if (!dirty.value) {
    emit('close');
    return;
  }
  if (await save()) {
    emit('close');
  }
}

// 关闭请求:有修改先进确认态,放弃修改才直接关
function requestClose() {
  if (dirty.value) {
    confirming.value = true;
  } else {
    emit('close');
  }
}

function onKeydown(e: KeyboardEvent) {
  if (e.key !== 'Escape') {
    return;
  }
  // 确认态下 Esc = 继续编辑;编辑态下 Esc = 关闭请求(带修改则进确认)
  if (confirming.value) {
    confirming.value = false;
  } else {
    requestClose();
  }
}

onMounted(() => window.addEventListener('keydown', onKeydown));
onUnmounted(() => window.removeEventListener('keydown', onKeydown));
</script>

<template>
  <Teleport to="body">
    <div class="overlay" @click.self="requestClose">
      <img class="image" :src="imageUrl" :alt="item.name" :style="imageStyle" draggable="false" />

      <!-- 底部中间工具条:旋转 + 保存/退出(Eagle 式,同预览翻页器的位置) -->
      <div class="bar">
        <button class="bar-btn" :disabled="saving" title="逆时针旋转 90°" @click="rotate(-90)">↺</button>
        <button class="bar-btn" :disabled="saving" title="顺时针旋转 90°" @click="rotate(90)">↻</button>
        <button class="bar-btn" :disabled="saving" @click="requestClose">退出</button>
        <button class="save-btn" :disabled="saving || !dirty" @click="saveAndClose()">
          {{ saving ? '保存中…' : '保存' }}
        </button>
      </div>

      <button class="close" title="关闭" @click="requestClose">×</button>

      <!-- 带修改退出:保存 / 不保存 / 取消 -->
      <div v-if="confirming" class="confirm-mask" @click.self="confirming = false">
        <div class="confirm">
          <div class="confirm-text">图片已修改，是否保存？</div>
          <div class="confirm-actions">
            <button @click="confirming = false">取消</button>
            <button class="discard" @click="confirming = false; emit('close')">不保存</button>
            <button class="primary" @click="confirming = false; void saveAndClose()">保存</button>
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
/* Eagle 式全屏遮罩:观感同预览浮层(深色 + 磨砂),层级高于它 */
.overlay {
  position: fixed;
  inset: 0;
  z-index: 220;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  background: rgba(0, 0, 0, 0.85);
  backdrop-filter: blur(24px);
}

/* 90/270° 旋转后图像允许超出 90vw/82vh 的盒子,渲染在深色遮罩上不裁切 */
.image {
  max-width: 90vw;
  max-height: 82vh;
  object-fit: contain;
  transform-origin: center;
  user-select: none;
}

.bar {
  position: absolute;
  bottom: 14px;
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--fg-0);
}

.bar-btn {
  border: none;
  background: transparent;
  color: var(--fg-1);
  font-size: 16px;
  padding: 4px 10px;
}

.bar-btn:hover:not(:disabled) {
  color: #fff;
  background: transparent;
}

.bar-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.save-btn {
  padding: 4px 16px;
  font-size: 14px;
}

.save-btn:not(:disabled) {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}

.save-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.close {
  position: absolute;
  top: 12px;
  right: 16px;
  border: none;
  background: transparent;
  color: var(--fg-1);
  font-size: 28px;
}

.close:hover {
  color: #fff;
  background: transparent;
}

.confirm-mask {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.45);
}

.confirm {
  padding: 16px 20px;
  border-radius: 8px;
  background: var(--bg-2);
  border: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.confirm-text {
  font-weight: 600;
}

.confirm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}

/* 不用 .danger:全局样式 button.danger 是红底实心(回收站按钮),文字会被同色系吞掉 */
.discard {
  color: var(--danger);
}
</style>
