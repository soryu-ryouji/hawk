<script setup lang="ts">
// 配对二维码弹层：把桌面端生成的 JSON 载荷渲染成二维码，附手动输入用的字段明文。
// 载荷格式见 domains/settings/pairing.ts（与 hawk-ios PairingPayload 对齐）。
// 挂载即生成；关闭由父组件控制（v-if）。Esc 在捕获阶段消费，避免设置面板的 Esc 先关掉底层对话框。
import { onMounted, onUnmounted, ref, watch } from 'vue';
import { toDataURL } from 'qrcode';
import { useClipboard, useEventListener } from '@vueuse/core';
import Icon from '@/shared/ui/Icon.vue';

interface PairingDetail {
  label: string;
  value: string;
}

const props = defineProps<{
  title: string;
  payload: string;
  details: PairingDetail[];
  note: string;
  warning?: string;
}>();

const emit = defineEmits<{ close: [] }>();

const qrDataUrl = ref('');
const qrError = ref<string | null>(null);
const copied = ref('');
const { copy } = useClipboard({ legacy: true });

async function renderQr(): Promise<void> {
  qrDataUrl.value = '';
  qrError.value = null;
  try {
    // 480px 足够打印/放大；白色留白（margin）保证手机相机识别
    qrDataUrl.value = await toDataURL(props.payload, {
      width: 480,
      margin: 1,
      errorCorrectionLevel: 'M',
      color: { dark: '#000000', light: '#ffffff' },
    });
  } catch (e) {
    qrError.value = e instanceof Error ? e.message : '二维码生成失败';
  }
}

watch(
  () => props.payload,
  () => void renderQr(),
  { immediate: true },
);

async function copyValue(value: string, tag: string): Promise<void> {
  await copy(value);
  copied.value = tag;
  window.setTimeout(() => {
    if (copied.value === tag) {
      copied.value = '';
    }
  }, 1500);
}

onMounted(() => document.body.classList.add('pairing-open'));
onUnmounted(() => document.body.classList.remove('pairing-open'));

useEventListener(
  window,
  'keydown',
  (e) => {
    if (e.key !== 'Escape') {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    emit('close');
  },
  { capture: true },
);
</script>

<template>
  <Teleport to="body">
    <div class="pair-mask" @click.self="emit('close')">
      <div class="pair-dialog" role="dialog" aria-modal="true" :aria-label="title">
        <header class="pair-head">
          <span class="pair-title">{{ title }}</span>
          <button class="icon-btn" title="关闭 (Esc)" @click="emit('close')">
            <Icon name="close" :size="14" />
          </button>
        </header>

        <div class="pair-body">
          <div class="qr-card">
            <img v-if="qrDataUrl" :src="qrDataUrl" alt="hawk 配对二维码" />
            <div v-else class="qr-placeholder">{{ qrError ?? '正在生成二维码…' }}</div>
          </div>

          <p class="pair-note">{{ note }}</p>
          <p v-if="warning" class="pair-warning">{{ warning }}</p>
          <p class="pair-hint">二维码包含访问凭证，请勿截图转发或分享给他人。</p>

          <dl class="pair-details">
            <div v-for="detail in details" :key="detail.label" class="pair-detail">
              <dt>{{ detail.label }}</dt>
              <dd>
                <code>{{ detail.value }}</code>
                <button class="icon-btn" :title="`复制${detail.label}`" @click="copyValue(detail.value, detail.label)">
                  <Icon :name="copied === detail.label ? 'check' : 'copy'" :size="13" />
                </button>
              </dd>
            </div>
          </dl>
        </div>

        <footer class="pair-actions">
          <a v-if="qrDataUrl" class="pair-download" :href="qrDataUrl" download="hawk-pair-qr.png">保存二维码图片</a>
          <button :disabled="!qrDataUrl" @click="copyValue(payload, 'payload')">
            {{ copied === 'payload' ? '已复制' : '复制配对内容' }}
          </button>
          <button class="primary" @click="emit('close')">完成</button>
        </footer>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.pair-mask {
  position: fixed;
  inset: 0;
  /* 高于设置面板（170），低于导入进度/全局上下文菜单的顶层 */
  z-index: 190;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.62);
  padding: 16px;
}

.pair-dialog {
  width: min(460px, calc(100vw - 32px));
  max-height: calc(100vh - 48px);
  display: flex;
  flex-direction: column;
  border-radius: 12px;
  background: var(--bg-1);
  border: 1px solid var(--border);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  overflow: hidden;
}

.pair-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 12px 10px 16px;
  border-bottom: 1px solid var(--border);
}

.pair-title {
  font-weight: 600;
}

.pair-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 18px 20px 16px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
}

.qr-card {
  width: min(264px, 100%);
  aspect-ratio: 1;
  padding: 12px;
  border-radius: 10px;
  background: #fff;
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
}

.qr-card img {
  width: 100%;
  height: 100%;
  display: block;
  image-rendering: pixelated;
}

.qr-placeholder {
  padding: 12px;
  color: #555;
  font-size: 12px;
  text-align: center;
  word-break: break-word;
}

.pair-note,
.pair-hint,
.pair-warning {
  width: 100%;
  margin: 0;
  font-size: 12px;
  line-height: 1.5;
}

.pair-note {
  color: var(--fg-0);
}

.pair-hint {
  color: var(--fg-1);
}

.pair-warning {
  color: var(--danger);
}

.pair-details {
  width: 100%;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding-top: 4px;
  border-top: 1px solid var(--border);
}

.pair-detail {
  display: flex;
  gap: 8px;
  align-items: flex-start;
}

.pair-detail dt {
  flex: none;
  width: 72px;
  color: var(--fg-1);
  font-size: 12px;
  padding-top: 3px;
}

.pair-detail dd {
  flex: 1;
  min-width: 0;
  margin: 0;
  display: flex;
  align-items: flex-start;
  gap: 4px;
}

.pair-detail code {
  flex: 1;
  min-width: 0;
  font-family: ui-monospace, monospace;
  font-size: 11px;
  color: var(--fg-0);
  word-break: break-all;
  padding-top: 3px;
  user-select: all;
}

.pair-actions {
  flex: none;
  display: flex;
  justify-content: flex-end;
  align-items: center;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--border);
}

.pair-download {
  margin-right: auto;
  color: var(--fg-1);
  font-size: 12px;
  text-decoration: none;
}

.pair-download:hover {
  color: var(--accent);
}

.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}
</style>
