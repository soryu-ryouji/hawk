<script setup lang="ts">
// 高级：本机 daemon API 端口与局域网网站端口。两个端口职责不同，集中放到网络分区最右侧的
// 「高级」标签，避免日常访问配置被端口等低频设置打断。
import { computed, onMounted, ref } from 'vue';
import SettingsLocalService from './SettingsLocalService.vue';
import { useLanSettingsStore } from '../lanSettings';

const lan = useLanSettingsStore();
const notice = ref('');
const initialPort = ref('');
const initialPeerPort = ref('');
const lanDirty = computed(() => lan.port !== initialPort.value || lan.peerPort !== initialPeerPort.value);

onMounted(async () => {
  await lan.load();
  initialPort.value = lan.port;
  initialPeerPort.value = lan.peerPort;
});

async function saveLanPort(): Promise<void> {
  notice.value = '';
  if (await lan.save()) {
    initialPort.value = lan.port;
    initialPeerPort.value = lan.peerPort;
    notice.value = '局域网端口已保存并生效';
    window.setTimeout(() => (notice.value = ''), 3000);
  }
}
</script>

<template>
  <div class="advanced">
    <section class="settings-section">
      <h3 class="settings-section-title">本机服务</h3>
      <SettingsLocalService />
    </section>

    <section class="settings-section">
      <h3 class="settings-section-title">局域网</h3>
      <div class="settings-group">
        <div class="settings-row">
          <label class="settings-label" for="settings-web-port">局域网网站端口</label>
          <input
            id="settings-web-port"
            v-model="lan.port"
            class="settings-input"
            :class="{ invalid: !lan.portValid }"
            type="text"
            inputmode="numeric"
            autocomplete="off"
            spellcheck="false"
            placeholder="1 – 65535"
            @keydown.enter="saveLanPort"
          />
        </div>
        <div class="settings-row">
          <label class="settings-label" for="settings-peer-port">hawk-app 直连端口</label>
          <input
            id="settings-peer-port"
            v-model="lan.peerPort"
            class="settings-input"
            :class="{ invalid: !lan.peerPortValid }"
            type="text"
            inputmode="numeric"
            autocomplete="off"
            spellcheck="false"
            placeholder="1 – 65535"
            @keydown.enter="saveLanPort"
          />
        </div>
        <div class="settings-row">
          <span class="settings-status">{{ notice }}</span>
          <span />
          <button class="settings-action primary" :disabled="lan.saving || !lanDirty || !lan.portValid || !lan.peerPortValid" @click="saveLanPort">
            {{ lan.saving ? '保存中…' : '应用端口' }}
          </button>
        </div>
        <p v-if="!lan.portValid || !lan.peerPortValid" class="settings-message error">{{ lan.PORT_ERROR }}</p>
        <p v-if="lan.error" class="settings-message error">{{ lan.error }}</p>
        <p v-if="lan.peerError" class="settings-message error">{{ lan.peerError }}</p>
      </div>
    </section>
  </div>
</template>

<style src="./settings-shared.css"></style>

<style scoped>
.advanced {
  display: flex;
  flex-direction: column;
  gap: 20px;
  padding-top: 2px;
}
</style>
