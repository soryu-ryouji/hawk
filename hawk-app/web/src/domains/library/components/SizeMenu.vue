<script setup lang="ts">
// 尺寸筛选下拉（Unity Game 视图分辨率选择器风格的区间化演进）：
// 面板自上而下 = 最小/最大区间输入（自定义像素）→ 标志性档位列表（480p–8K，点击填入
// 最近聚焦的输入框，无聚焦填下限）→ 模式下拉（短边区间 / 宽高区间：后者展开为
// min/max 宽 × min/max 高四框，档位列表同样可用，填到聚焦的宽/高框）。
// 短边模式按 min(width, height) 比较（横竖图通吃）；宽高模式宽高独立区间。
// 清除入口：chip 激活时的 ×（与颜色 chip 一致）或清空全部输入后应用。
// 自绘触发 chip + Teleport 浮层（SelectBox 同模式：fixed 定位、外点关闭、翻转防出屏）
import { computed, nextTick, onBeforeUnmount, ref } from 'vue';
import Icon from '@/shared/ui/Icon.vue';
import type { SizeFilter } from '@/shared/types';

/** 标志性分辨率档位（短边像素值；label 为右侧淡色别名） */
const TIERS = [
  { value: 480, label: 'SD' },
  { value: 720, label: 'HD' },
  { value: 1080, label: 'FHD' },
  { value: 1440, label: 'QHD · 2K' },
  { value: 2160, label: 'UHD · 4K' },
  { value: 4320, label: '8K' },
];

/** 六个输入框的键（档位点击按最近聚焦的框填充） */
type BoxKey = 'side-min' | 'side-max' | 'wh-minW' | 'wh-maxW' | 'wh-minH' | 'wh-maxH';

/** 档位填入目标的显示名（下限蓝 / 上限红，与输入框颜色一致） */
const BOX_NAMES: Record<BoxKey, string> = {
  'side-min': '最小',
  'side-max': '最大',
  'wh-minW': '宽 · 最小',
  'wh-maxW': '宽 · 最大',
  'wh-minH': '高 · 最小',
  'wh-maxH': '高 · 最大',
};

const props = defineProps<{ size?: SizeFilter }>();
const emit = defineEmits<{ select: [size: SizeFilter | undefined] }>();

const open = ref(false);
/** 当前编辑模式：打开时继承激活条件的模式，缺省短边；切换只换输入区，应用才生效 */
const mode = ref<'side' | 'wh'>('side');
/** 模式下拉展开态 */
const modeOpen = ref(false);
/** 最近聚焦的输入框（档位点击的填充目标；blur 不清除，面板打开时重置） */
const focusedBox = ref<BoxKey | null>(null);
const triggerRef = ref<HTMLElement | null>(null);
const panelRef = ref<HTMLElement | null>(null);
const pos = ref({ x: 0, y: 0 });

// 短边区间输入草稿（空串 = 无边界）
const minSide = ref('');
const maxSide = ref('');
// 宽高区间输入草稿
const minW = ref('');
const maxW = ref('');
const minH = ref('');
const maxH = ref('');

/** 值恰为档位值时用 p 记法（1080 → 1080p），自定义值带 px */
function tierName(value: number): string {
  return TIERS.some((t) => t.value === value) ? `${value}p` : `${value}px`;
}

/** chip 文案：短边区间 ≥/≤/区间；宽高按边组合 */
const chipLabel = computed(() => {
  if (!props.size) {
    return '尺寸';
  }
  if (props.size.kind === 'side') {
    const { min, max } = props.size;
    if (min !== undefined && max !== undefined) {
      return `${tierName(min)} – ${tierName(max)}`;
    }
    return min !== undefined ? `≥ ${tierName(min)}` : `≤ ${tierName(props.size.max!)}`;
  }
  const box = (min?: number, max?: number) => (min !== undefined && max !== undefined ? `${min}–${max}` : min !== undefined ? `${min}+` : `≤${max}`);
  const w = props.size.minWidth !== undefined || props.size.maxWidth !== undefined ? box(props.size.minWidth, props.size.maxWidth) : '';
  const h = props.size.minHeight !== undefined || props.size.maxHeight !== undefined ? box(props.size.minHeight, props.size.maxHeight) : '';
  if (w && h) {
    return `${w} × ${h}`;
  }
  return w ? `宽 ${w}` : `高 ${h}`;
});

/** 单框解析：空 = 无边界（null 区别于非法）；非法返回 NaN */
function parseBox(v: string): number | null {
  if (v.trim() === '') {
    return null;
  }
  const n = Number(v);
  return Number.isInteger(n) && n >= 1 && n <= 99999 ? n : Number.NaN;
}

/** 应用可用：全部框合法（空或 1–99999）且各区间不倒挂（min ≤ max）；全空合法（= 清除） */
const sideValid = computed(() => {
  const min = parseBox(minSide.value);
  const max = parseBox(maxSide.value);
  return [min, max].every((v) => v === null || !Number.isNaN(v)) && (min === null || max === null || min <= max);
});
const whValid = computed(() => {
  const wMin = parseBox(minW.value);
  const wMax = parseBox(maxW.value);
  const hMin = parseBox(minH.value);
  const hMax = parseBox(maxH.value);
  const ok = [wMin, wMax, hMin, hMax].every((v) => v === null || !Number.isNaN(v));
  const ordered = (a: number | null, b: number | null) => a === null || b === null || a <= b;
  return ok && ordered(wMin, wMax) && ordered(hMin, hMax);
});

async function openPanel(): Promise<void> {
  if (open.value) {
    return;
  }
  const rect = triggerRef.value!.getBoundingClientRect();
  open.value = true;
  await nextTick();
  const panel = panelRef.value!;
  const gap = 4;
  // 下方放不下且上方空间足够时向上翻转
  const up = rect.bottom + gap + panel.offsetHeight > window.innerHeight && rect.top - gap - panel.offsetHeight > 0;
  // 面板比 chip 宽：默认左对齐锚点左缘，右缘出屏时收缩到屏内（不小于 0）
  let x = rect.left;
  if (x + panel.offsetWidth > window.innerWidth) {
    x = Math.max(0, window.innerWidth - panel.offsetWidth);
  }
  pos.value = { x, y: up ? rect.top - gap - panel.offsetHeight : rect.bottom + gap };
  // 模式与草稿继承激活条件（模式不匹配时草稿留空）
  mode.value = props.size?.kind ?? 'side';
  modeOpen.value = false;
  focusedBox.value = null;
  minSide.value = props.size?.kind === 'side' && props.size.min !== undefined ? String(props.size.min) : '';
  maxSide.value = props.size?.kind === 'side' && props.size.max !== undefined ? String(props.size.max) : '';
  minW.value = props.size?.kind === 'wh' && props.size.minWidth !== undefined ? String(props.size.minWidth) : '';
  maxW.value = props.size?.kind === 'wh' && props.size.maxWidth !== undefined ? String(props.size.maxWidth) : '';
  minH.value = props.size?.kind === 'wh' && props.size.minHeight !== undefined ? String(props.size.minHeight) : '';
  maxH.value = props.size?.kind === 'wh' && props.size.maxHeight !== undefined ? String(props.size.maxHeight) : '';
  window.addEventListener('resize', close);
  document.addEventListener('mousedown', onOutside, true);
}

function close(): void {
  if (!open.value) {
    return;
  }
  open.value = false;
  modeOpen.value = false;
  window.removeEventListener('resize', close);
  document.removeEventListener('mousedown', onOutside, true);
}

function toggle(): void {
  if (open.value) {
    close();
  } else {
    void openPanel();
  }
}

/** 档位点击的填充目标：最近聚焦的框属于当前模式则用之，否则用该模式下限（side-min / wh-minW） */
function tierTargetBox(): BoxKey {
  const prefix = mode.value === 'side' ? 'side' : 'wh';
  const fallback: BoxKey = mode.value === 'side' ? 'side-min' : 'wh-minW';
  return focusedBox.value?.startsWith(prefix) ? focusedBox.value : fallback;
}

/** 档位勾选状态：值命中下限（蓝）/上限（红）；同时命中时下限优先 */
function tierCheckKind(value: number): 'min' | 'max' | null {
  const s = props.size;
  if (!s) {
    return null;
  }
  if (s.kind === 'side') {
    if (s.min === value) return 'min';
    if (s.max === value) return 'max';
    return null;
  }
  if (s.minWidth === value || s.minHeight === value) return 'min';
  if (s.maxWidth === value || s.maxHeight === value) return 'max';
  return null;
}

/** 档位命中徽章：标明该值绑定到哪个框（side：≥/≤ 蓝红；wh：轴徽章宽蓝底/高橙底 + ≥/≤ 符号） */
function tierBadges(value: number): { text: string; cls: string }[] {
  const s = props.size;
  if (!s) {
    return [];
  }
  if (s.kind === 'side') {
    const out: { text: string; cls: string }[] = [];
    if (s.min === value) out.push({ text: '≥', cls: 'b-min' });
    if (s.max === value) out.push({ text: '≤', cls: 'b-max' });
    return out;
  }
  const out: { text: string; cls: string }[] = [];
  if (s.minWidth === value) out.push({ text: '宽 ≥', cls: 'b-w' });
  if (s.maxWidth === value) out.push({ text: '宽 ≤', cls: 'b-w' });
  if (s.minHeight === value) out.push({ text: '高 ≥', cls: 'b-h' });
  if (s.maxHeight === value) out.push({ text: '高 ≤', cls: 'b-h' });
  return out;
}

const targetBox = computed(tierTargetBox);
const targetBoxName = computed(() => BOX_NAMES[targetBox.value]);
const targetIsMax = computed(() => targetBox.value.includes('max'));
const tiersView = computed(() => TIERS.map((t) => ({ ...t, check: tierCheckKind(t.value), badges: tierBadges(t.value) })));

/** 目标轴当前是否无限制（「不限」项的勾选态） */
const unlimitedChecked = computed(() => {
  const s = props.size;
  if (mode.value === 'side') {
    return !s || s.kind !== 'side';
  }
  if (!s || s.kind !== 'wh') {
    return true;
  }
  const wide = targetBox.value.startsWith('wh-minW') || targetBox.value === 'wh-maxW';
  return wide ? s.minWidth === undefined && s.maxWidth === undefined : s.minHeight === undefined && s.maxHeight === undefined;
});

/** 「不限」：清空目标轴的限制并立即生效（另一轴保留；全空即整体清除） */
function clearTargetAxis(): void {
  if (mode.value === 'side') {
    minSide.value = '';
    maxSide.value = '';
  } else {
    const wide = targetBox.value.startsWith('wh-minW') || targetBox.value === 'wh-maxW';
    if (wide) {
      minW.value = '';
      maxW.value = '';
    } else {
      minH.value = '';
      maxH.value = '';
    }
  }
  applyDraft();
}

/** 档位点击：填入目标框的草稿并立即生效，面板不关——可继续编辑其余框后统一应用 */
function chooseTier(value: number): void {
  const target = tierTargetBox();
  const v = String(value);
  if (target === 'side-min') minSide.value = v;
  else if (target === 'side-max') maxSide.value = v;
  else if (target === 'wh-minW') minW.value = v;
  else if (target === 'wh-maxW') maxW.value = v;
  else if (target === 'wh-minH') minH.value = v;
  else maxH.value = v;
  applyDraft();
}

function apply(): void {
  applyDraft();
  close();
}

/** 按当前草稿立即应用（不关闭面板；档位/不限点击与「应用」按钮共用）。全空 = 清除筛选 */
function applyDraft(): void {
  if (mode.value === 'side') {
    if (!sideValid.value) {
      return;
    }
    const min = parseBox(minSide.value);
    const max = parseBox(maxSide.value);
    if (min === null && max === null) {
      emit('select', undefined);
      return;
    }
    emit('select', { kind: 'side', min: min ?? undefined, max: max ?? undefined });
  } else {
    if (!whValid.value) {
      return;
    }
    const wMin = parseBox(minW.value);
    const wMax = parseBox(maxW.value);
    const hMin = parseBox(minH.value);
    const hMax = parseBox(maxH.value);
    if (wMin === null && wMax === null && hMin === null && hMax === null) {
      emit('select', undefined);
      return;
    }
    emit('select', {
      kind: 'wh',
      minWidth: wMin ?? undefined,
      maxWidth: wMax ?? undefined,
      minHeight: hMin ?? undefined,
      maxHeight: hMax ?? undefined,
    });
  }
}

/** chip 上的 × 就地清除（颜色 chip 同模式） */
function clearAll(): void {
  emit('select', undefined);
  close();
}

function onOutside(event: Event): void {
  const target = event.target as Node;
  if (triggerRef.value?.contains(target) || panelRef.value?.contains(target)) {
    return;
  }
  close();
}

onBeforeUnmount(() => {
  window.removeEventListener('resize', close);
  document.removeEventListener('mousedown', onOutside, true);
});
</script>

<template>
  <div ref="triggerRef" class="chip" :class="{ active: !!size }" title="尺寸筛选（短边档位区间或宽高区间）">
    <button type="button" class="trigger" @click="toggle" @keydown.esc.prevent.stop="close">
      <Icon name="size" :size="13" />
      <span>{{ chipLabel }}</span>
    </button>
    <button v-if="size" type="button" class="clear" title="清除尺寸筛选" @click.stop="clearAll">×</button>
  </div>
  <Teleport to="body">
    <div v-if="open" ref="panelRef" class="panel" :style="{ left: pos.x + 'px', top: pos.y + 'px' }">
      <!-- 短边区间：最小 – 最大（自定义像素，空 = 无边界） -->
      <div v-if="mode === 'side'" class="range">
        <input
          v-model="minSide"
          class="min"
          type="text"
          inputmode="numeric"
          placeholder="最小"
          aria-label="短边下限（像素）"
          @focus="focusedBox = 'side-min'"
          @keydown.enter.prevent="apply"
        />
        <span class="dash">–</span>
        <input
          v-model="maxSide"
          class="max"
          type="text"
          inputmode="numeric"
          placeholder="最大"
          aria-label="短边上限（像素）"
          @focus="focusedBox = 'side-max'"
          @keydown.enter.prevent="apply"
        />
        <button type="button" class="apply" :disabled="!sideValid" @click="apply">应用</button>
      </div>

      <!-- 宽高区间：min/max 宽 × min/max 高（宽高独立） -->
      <div v-if="mode === 'wh'" class="range wh">
        <div class="wh-row">
          <span class="axis axis-w">宽</span>
          <input
            v-model="minW"
            class="min"
            type="text"
            inputmode="numeric"
            placeholder="最小"
            aria-label="宽度下限（像素）"
            @focus="focusedBox = 'wh-minW'"
            @keydown.enter.prevent="apply"
          />
          <span class="dash">–</span>
          <input
            v-model="maxW"
            class="max"
            type="text"
            inputmode="numeric"
            placeholder="最大"
            aria-label="宽度上限（像素）"
            @focus="focusedBox = 'wh-maxW'"
            @keydown.enter.prevent="apply"
          />
        </div>
        <div class="wh-row">
          <span class="axis axis-h">高</span>
          <input
            v-model="minH"
            class="min"
            type="text"
            inputmode="numeric"
            placeholder="最小"
            aria-label="高度下限（像素）"
            @focus="focusedBox = 'wh-minH'"
            @keydown.enter.prevent="apply"
          />
          <span class="dash">–</span>
          <input
            v-model="maxH"
            class="max"
            type="text"
            inputmode="numeric"
            placeholder="最大"
            aria-label="高度上限（像素）"
            @focus="focusedBox = 'wh-maxH'"
            @keydown.enter.prevent="apply"
          />
        </div>
        <button type="button" class="apply" :disabled="!whValid" @click="apply">应用</button>
      </div>

      <!-- 档位填入目标提示（实时跟随聚焦框；颜色与目标输入框一致） -->
      <div class="tier-hint" :class="targetIsMax ? 'to-max' : 'to-min'">档位填入：{{ targetBoxName }}</div>

      <!-- 不限：清空目标轴（提示行所指）的限制，另一轴保留；全空即整体清除 -->
      <button type="button" class="item" :class="{ 'checked-unlimited': unlimitedChecked }" @click="clearTargetAxis">
        <span class="check">{{ unlimitedChecked ? '✓' : '' }}</span>
        <span class="label">不限</span>
        <span class="alias">{{ mode === 'wh' ? '清除' + (targetBoxName.split(' · ')[0] ?? '') + '限制' : '清除区间' }}</span>
      </button>

      <!-- 标志性档位：点击填入最近聚焦的输入框（无聚焦填下限），立即生效、面板不关；
           勾随匹配侧着色，命中时别名位量换为绑定框徽章（轴底色 + ≥/≤ 符号） -->
      <button
        v-for="t in tiersView"
        :key="t.value"
        type="button"
        class="item"
        :class="t.check === 'min' ? 'checked-min' : t.check === 'max' ? 'checked-max' : ''"
        @click="chooseTier(t.value)"
      >
        <span class="check">{{ t.check ? '✓' : '' }}</span>
        <span class="label">{{ t.value }}p</span>
        <template v-if="t.badges.length">
          <span v-for="(b, i) in t.badges" :key="i" class="badge" :class="b.cls">{{ b.text }}</span>
        </template>
        <span v-else class="alias">{{ t.label }}</span>
      </button>

      <!-- 模式下拉：短边区间 / 宽高区间（面板底部，展开式选项列表） -->
      <div class="mode">
        <button type="button" class="mode-btn" @click="modeOpen = !modeOpen">
          <span>{{ mode === 'side' ? '按短边区间' : '按宽高区间' }}</span>
          <span class="caret" :class="{ open: modeOpen }">▾</span>
        </button>
        <div v-if="modeOpen" class="mode-list">
          <button type="button" :class="{ on: mode === 'side' }" @click="((mode = 'side'), (modeOpen = false))">按短边区间</button>
          <button type="button" :class="{ on: mode === 'wh' }" @click="((mode = 'wh'), (modeOpen = false))">按宽高区间</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.chip {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  padding: 3px 6px 3px 0;
  border-radius: 11px;
  background: var(--bg-3);
  color: var(--fg-1);
  font-size: 12px;
}

.trigger {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 0 4px;
  border: none;
  background: transparent;
  color: inherit;
  font-size: inherit;
}

@media (hover: hover) {
  .trigger:hover {
    color: var(--fg-0);
  }
}

.chip.active {
  color: var(--accent);
}

.clear {
  padding: 0 4px;
  border: none;
  background: transparent;
  color: var(--fg-1);
}

@media (hover: hover) {
  .clear:hover {
    color: var(--danger);
    background: transparent;
  }
}

.panel {
  position: fixed;
  /* 右键菜单 400，与 SelectBox 同级 */
  z-index: 300;
  min-width: 220px;
  max-height: min(460px, 70vh);
  overflow-y: auto;
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
  width: 100%;
  padding: 5px 12px;
  border: none;
  border-radius: 4px;
  background: transparent;
  text-align: left;
  color: var(--fg-0);
}

@media (hover: hover) {
  .item:hover {
    background: color-mix(in srgb, var(--accent) 35%, transparent);
  }
}

.item .check {
  width: 12px;
  flex: none;
}

/* 勾随匹配侧着色：下限蓝 / 上限红（与输入框颜色一致） */
.item .label {
  flex: 1;
  white-space: nowrap;
}

.item.checked-min .check,
.item.checked-min .label {
  color: var(--accent);
}

.item.checked-max .check,
.item.checked-max .label {
  color: var(--danger);
}

/* 档位填入目标提示行（跟随聚焦框变色） */
.tier-hint {
  margin: 4px 10px 2px;
  font-size: 11px;
  color: var(--fg-1);
  user-select: none;
}

.tier-hint.to-min {
  color: var(--accent);
}

.tier-hint.to-max {
  color: var(--danger);
}

/* 右侧别名标注（SD/HD/FHD 等） */
.item .alias {
  flex: none;
  font-size: 11px;
  color: var(--fg-1);
}

/* 区间输入区 */
.range {
  display: flex;
  align-items: center;
  gap: 4px;
  margin: 4px 2px;
  padding: 6px 6px 4px;
  border-top: 1px solid var(--border);
}

.range input {
  width: 52px;
  min-width: 0;
  padding: 3px 4px;
  border-radius: 4px;
  background: var(--bg-2);
  border: 1px solid var(--border);
  color: var(--fg-0);
  font-size: 12px;
  text-align: center;
}

/* 下限蓝 / 上限红：数值文字、占位符、聚焦边框同色系，点档位前即可辨认填充目标 */
.range input.min {
  color: var(--accent);
}

.range input.max {
  color: var(--danger);
}

.range input.min::placeholder {
  color: color-mix(in srgb, var(--accent) 45%, var(--fg-1));
}

.range input.max::placeholder {
  color: color-mix(in srgb, var(--danger) 45%, var(--fg-1));
}

.range input:focus {
  outline: none;
}

.range input.min:focus {
  border-color: var(--accent);
}

.range input.max:focus {
  border-color: var(--danger);
}

.range .dash {
  flex: none;
  color: var(--fg-1);
}

.range .apply {
  margin-left: auto;
  flex: none;
  padding: 4px 10px;
  border-radius: 4px;
  border: none;
  background: var(--accent);
  color: #fff;
  font-size: 12px;
}

.range .apply:disabled {
  opacity: 0.4;
  cursor: default;
}

/* 宽高区间：两行输入 + 应用 */
.range.wh {
  flex-direction: column;
  align-items: stretch;
}

.wh-row {
  display: flex;
  align-items: center;
  gap: 4px;
}

.wh-row .axis {
  flex: none;
  width: 20px;
  padding: 2px 0;
  border-radius: 3px;
  font-size: 11px;
  text-align: center;
  color: #fff;
}

/* 轴徽章：宽蓝底 / 高橙底（档位命中徽章同色呼应，轴靠底色区分） */
.axis-w,
.badge.b-w {
  background: var(--accent);
}

.axis-h,
.badge.b-h {
  background: #d29922;
}

/* 档位命中徽章：side 模式 ≥/≤ 淡底蓝红；wh 模式轴底色 + ≥/≤ 符号 */
.badge {
  flex: none;
  padding: 1px 5px;
  border-radius: 3px;
  font-size: 10px;
  color: #fff;
}

.badge.b-min {
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 18%, transparent);
}

.badge.b-max {
  color: var(--danger);
  background: color-mix(in srgb, var(--danger) 18%, transparent);
}

.badge + .badge {
  margin-left: 4px;
}

/* 「不限」勾：中性色 */
.item.checked-unlimited .check {
  color: var(--fg-0);
}

.range.wh .apply {
  align-self: flex-end;
}

/* 模式下拉（面板底部） */
.mode {
  position: relative;
  margin-top: 2px;
  padding-top: 4px;
  border-top: 1px solid var(--border);
}

.mode-btn {
  display: flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
  padding: 5px 10px;
  border: none;
  border-radius: 4px;
  background: var(--bg-2);
  color: var(--fg-0);
  font-size: 12px;
}

.mode-btn .caret {
  color: var(--fg-1);
  transition: transform 0.15s;
}

.mode-btn .caret.open {
  transform: rotate(180deg);
}

.mode-list {
  position: absolute;
  bottom: calc(100% - 2px);
  left: 0;
  right: 0;
  z-index: 1;
  padding: 3px;
  border-radius: 5px;
  background: var(--bg-2);
  border: 1px solid var(--border);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  display: flex;
  flex-direction: column;
}

.mode-list button {
  padding: 5px 10px;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: var(--fg-0);
  font-size: 12px;
  text-align: left;
}

@media (hover: hover) {
  .mode-list button:hover {
    background: color-mix(in srgb, var(--accent) 35%, transparent);
  }
}

.mode-list button.on {
  color: var(--accent);
}
</style>
