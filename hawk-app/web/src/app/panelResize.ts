// 侧栏/检查器面板宽度拖拽（装配层）：拖拽手柄的 mousemove 生命周期、区间钳制与 localStorage 持久化。
// 从 App.vue 抽出的纯 DOM 交互，返回模板所需状态。
import { onMounted, onUnmounted, ref } from 'vue';
import { loadJSON, saveJSON, STORAGE_KEYS } from '@/shared/lib/persist';

const SIDEBAR_MIN = 180;
const SIDEBAR_MAX = 480;
const INSPECTOR_MIN = 240;
const INSPECTOR_MAX = 560;

function clamp(v: number, min: number, max: number) {
  return Math.min(max, Math.max(min, Math.round(v)));
}

export function usePanelResize() {
  const sidebarWidth = ref(220);
  const inspectorWidth = ref(280);
  const dragSide = ref<'left' | 'right' | null>(null);

  function loadPanelWidths() {
    const saved = loadJSON(STORAGE_KEYS.panelWidths, {} as { sidebar?: number; inspector?: number });
    if (typeof saved.sidebar === 'number') {
      sidebarWidth.value = clamp(saved.sidebar, SIDEBAR_MIN, SIDEBAR_MAX);
    }
    if (typeof saved.inspector === 'number') {
      inspectorWidth.value = clamp(saved.inspector, INSPECTOR_MIN, INSPECTOR_MAX);
    }
  }

  function savePanelWidths() {
    saveJSON(STORAGE_KEYS.panelWidths, { sidebar: sidebarWidth.value, inspector: inspectorWidth.value });
  }

  function onResizeMove(e: MouseEvent) {
    if (dragSide.value === 'left') {
      sidebarWidth.value = clamp(e.clientX, SIDEBAR_MIN, SIDEBAR_MAX);
    } else if (dragSide.value === 'right') {
      inspectorWidth.value = clamp(window.innerWidth - e.clientX, INSPECTOR_MIN, INSPECTOR_MAX);
    }
  }

  function stopResize() {
    dragSide.value = null;
    document.body.classList.remove('col-resizing');
    window.removeEventListener('mousemove', onResizeMove);
    window.removeEventListener('mouseup', stopResize);
    savePanelWidths();
  }

  function startResize(side: 'left' | 'right') {
    dragSide.value = side;
    document.body.classList.add('col-resizing');
    window.addEventListener('mousemove', onResizeMove);
    window.addEventListener('mouseup', stopResize);
  }

  onMounted(loadPanelWidths);
  onUnmounted(() => {
    // 拖拽中组件被卸载（如切到引导页）时兜底清理
    if (dragSide.value) {
      stopResize();
    }
  });

  return { sidebarWidth, inspectorWidth, dragSide, startResize };
}
