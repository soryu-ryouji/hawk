<script setup lang="ts">
import { ref } from 'vue';

const props = defineProps<{ modelValue: number }>();
const emit = defineEmits<{ 'update:modelValue': [value: number] }>();

const hover = ref(-1);

function starClass(i: number) {
  const active = hover.value >= 0 ? hover.value : props.modelValue;
  return i < active ? 'on' : 'off';
}

function rate(i: number) {
  // 点当前星值 → 清零
  emit('update:modelValue', props.modelValue === i + 1 ? 0 : i + 1);
}
</script>

<template>
  <span class="rating" @mouseleave="hover = -1">
    <span
      v-for="i in 5"
      :key="i"
      class="star"
      :class="starClass(i - 1)"
      @mouseenter="hover = i"
      @click="rate(i - 1)"
      >★</span
    >
  </span>
</template>

<style scoped>
.rating {
  display: inline-flex;
  gap: 2px;
}

.star {
  cursor: pointer;
  font-size: 16px;
}

.star.on {
  color: #f5c518;
}

.star.off {
  color: var(--border);
}
</style>
