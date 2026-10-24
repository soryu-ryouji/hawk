<script setup lang="ts">
// 高级：本机 daemon API 端口与局域网网站端口。两个端口职责不同，集中放到网络分区最右侧的
// 「高级」标签，避免日常访问配置被端口等低频设置打断。
import { onMounted, ref } from 'vue';
import SettingsLocalService from './SettingsLocalService.vue';
import { useLanSettingsStore } from '../lanSettings';

const lan = useLanSettingsStore();
const notice = ref('');

onMounted(async () => {
  await lan.load();
});

async function saveLanPort(): Promise<void> {
  notice.value = '';
  if (await lan.save()) {
    notice.value = '局域网端口已保存并生效';
    window.setTimeout(() => (notice.value = ''), 3000);
  }
}
</script>

<template>
  <div class="advanced">
    <h3 class="section-title">本机 API 端口</h3>
    <SettingsLocalService />

    <h3 class="section-title">局域网端口</h3>
    <div class="field column">
      <span class="field-label">局域网网站端口</span>
      <div class="port-row">
        <input
          v-model="lan.port"
          type="text"
          inputmode="numeric"
          autocomplete="off"
          spellcheck="false"
          placeholder="1 – 65535"
          :class="{ invalid: !lan.portValid }"
        />
      </div>
      <span class="field-label">hawk-app 直连端口</span>
      <div class="port-row">
        <input
          v-model="lan.peerPort"
          type="text"
          inputmode="numeric"
          autocomplete="off"
          spellcheck="false"
          placeholder="1 – 65535"
          :class="{ invalid: !lan.peerPortValid }"
        />
        <button :disabled="lan.saving || !lan.portValid || !lan.peerPortValid" @click="saveLanPort">
          {{ lan.saving ? '保存中…' : '应用端口' }}
        </button>
      </div>
      <p v-if="!lan.portValid || !lan.peerPortValid" class="field-error">{{ lan.PORT_ERROR }}</p>
      <p v-if="notice" class="ok">{{ notice }}</p>
      <p v-if="lan.error" class="error">{{ lan.error }}</p>
      <p v-if="lan.peerError" class="error">{{ lan.peerError }}</p>
    </div>
  </div>
</template>

<style src="./settings-shared.css"></style>

<style scoped>
.advanced {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.section-title {
  margin: 4px 0 0;
  padding-top: 12px;
  border-top: 1px solid var(--border);
  font-size: 13px;
  font-weight: 600;
}
.section-title:first-child {
  padding-top: 0;
  border-top: none;
}
.port-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.port-row input {
  width: 180px;
  padding: 5px 8px;
}
.port-row input.invalid {
  border-color: var(--danger);
}
.ok {
  margin: 0;
  color: var(--accent);
  font-size: 12px;
}
.error {
  margin: 0;
  color: var(--danger);
  font-size: 12px;
}
</style>
