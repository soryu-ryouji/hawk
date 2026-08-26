<script setup lang="ts">
defineProps<{ text: string }>();
</script>

<template>
  <div class="empty">{{ text }}</div>
</template>

<style scoped>
.empty {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--fg-1);
}
</style>
