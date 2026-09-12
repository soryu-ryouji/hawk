<script setup lang="ts">
// 检查器「文件位置」列表（哑组件）：只读/编辑共用一份结构。
// 路径点击 → navigate（跳转所在文件夹视图，由调用方决定跳转语义）；
// removable 且多位置时每行带「删除此位置」按钮 → remove（位置级删除，其余位置保留）。
import Icon from '@/shared/ui/Icon.vue';

defineProps<{ paths: string[]; removable?: boolean }>();
defineEmits<{ navigate: [path: string]; remove: [path: string] }>();
</script>

<template>
  <div v-if="paths.length">
    <div v-for="path in paths" :key="path" class="path-row">
      <button class="path jump" :title="`查看所在文件夹：${path}`" @click="$emit('navigate', path)">{{ path }}</button>
      <button v-if="removable && paths.length > 1" class="finder danger-btn" title="删除此位置（其余位置保留）" @click="$emit('remove', path)">
        <Icon name="trash" :size="13" />
      </button>
    </div>
  </div>
  <span v-else class="ro-empty">—</span>
</template>

<style src="./inspector-shared.css"></style>
