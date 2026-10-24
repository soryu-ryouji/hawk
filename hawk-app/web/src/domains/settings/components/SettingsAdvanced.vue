<script setup lang="ts">
// 高级：本机 daemon API 端口与局域网端口。保存统一由设置面板 footer 触发。
import { computed, onMounted, ref } from 'vue';
import { shell } from '@/shared/lib/platform';
import { useLanSettingsStore } from '../lanSettings';

const lan = useLanSettingsStore();
const initialPort = ref('');
const initialPeerPort = ref('');
const lanDirty = computed(() => lan.port !== initialPort.value || lan.peerPort !== initialPeerPort.value);

const apiPortCurrent = ref<number | null>(null);
const apiPortInput = ref('');
const apiPortRange = ref<readonly [number, number]>([27371, 27400]);
const apiPortSaving = ref(false);
const apiPortError = ref('');
const apiPortNotice = ref('');

const apiPortDirty = computed(() => apiPortCurrent.value !== null && apiPortInput.value.trim() !== String(apiPortCurrent.value));
const apiPortValid = computed(() => {
  const port = Number(apiPortInput.value.trim());
  return Number.isInteger(port) && port >= 1024 && port <= 65535;
});
const apiPortOutsideRange = computed(() => {
  const port = Number(apiPortInput.value.trim());
  return Number.isInteger(port) && (port < apiPortRange.value[0] || port > apiPortRange.value[1]);
});

const dirty = computed(() => lanDirty.value || apiPortDirty.value || !lan.portValid || !lan.peerPortValid || !apiPortValid.value);
const busy = computed(() => lan.saving || apiPortSaving.value);

const notice = ref('');

onMounted(async () => {
  await lan.load();
  initialPort.value = lan.port;
  initialPeerPort.value = lan.peerPort;
  try {
    const info = await shell.getApiPort();
    apiPortCurrent.value = info.port;
    apiPortRange.value = info.range;
    apiPortInput.value = String(info.port);
  } catch (e) {
    apiPortError.value = e instanceof Error ? e.message : String(e);
  }
});

async function save(): Promise<boolean> {
  notice.value = '';
  apiPortError.value = '';
  apiPortNotice.value = '';
  if (!apiPortValid.value) {
    apiPortError.value = '端口须为 1024–65535 之间的整数';
    return false;
  }
  if (!lan.portValid || !lan.peerPortValid) {
    lan.error = lan.PORT_ERROR;
    return false;
  }

  let ok = true;
  if (apiPortDirty.value) {
    apiPortSaving.value = true;
    try {
      const rejected = await shell.setApiPort(Number(apiPortInput.value.trim()));
      if (rejected) {
        apiPortError.value = rejected;
        ok = false;
      } else {
        apiPortCurrent.value = Number(apiPortInput.value.trim());
        apiPortInput.value = String(apiPortCurrent.value);
        apiPortNotice.value = '本地 API 端口已保存，重启 hawk 后生效。';
      }
    } catch (e) {
      apiPortError.value = e instanceof Error ? e.message : String(e);
      ok = false;
    } finally {
      apiPortSaving.value = false;
    }
  }

  if (lanDirty.value) {
    if (await lan.save()) {
      initialPort.value = lan.port;
      initialPeerPort.value = lan.peerPort;
    } else {
      ok = false;
    }
  }

  if (ok) {
    notice.value = '设置已保存';
    window.setTimeout(() => {
      if (notice.value === '设置已保存') {
        notice.value = '';
      }
    }, 3000);
  }
  return ok;
}

defineExpose({ save, dirty, busy });
</script>

<template>
  <div class="advanced">
    <section class="settings-section">
      <h3 class="settings-section-title">本机服务</h3>
      <div class="settings-group">
        <div class="settings-row">
          <label class="settings-label" for="settings-api-port">本地 API 端口</label>
          <input
            id="settings-api-port"
            v-model="apiPortInput"
            class="settings-input"
            :class="{ invalid: apiPortInput.trim() !== '' && !apiPortValid }"
            type="text"
            inputmode="numeric"
            autocomplete="off"
            spellcheck="false"
            placeholder="1024 – 65535"
          />
        </div>
        <p v-if="apiPortOutsideRange" class="settings-message error">
          端口 {{ apiPortInput.trim() }} 在自动发现区间 {{ apiPortRange[0] }}–{{ apiPortRange[1] }} 之外，浏览器插件将无法自动找到它。
        </p>
        <p v-if="apiPortDirty" class="settings-message">保存后仍需重启 hawk，本地 API 才会使用新端口。</p>
        <p v-if="apiPortNotice" class="settings-message ok">{{ apiPortNotice }}</p>
        <p v-if="apiPortError" class="settings-message error">{{ apiPortError }}</p>
      </div>
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
          />
        </div>
        <p v-if="!lan.portValid || !lan.peerPortValid" class="settings-message error">{{ lan.PORT_ERROR }}</p>
        <p v-if="lan.error" class="settings-message error">{{ lan.error }}</p>
        <p v-if="lan.peerError" class="settings-message error">{{ lan.peerError }}</p>
        <p v-if="notice" class="settings-message ok">{{ notice }}</p>
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
