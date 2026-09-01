// 网格布局的共享常量与算法（ItemGrid 的行布局数学 + ItemCard 的卡片样式共同消费）。
// 单一来源：改这里即全局生效，ItemCard 通过根元素 CSS 变量接收（避免「两处必须一致」的隐式耦合）。
// 齐行布局纯函数（layoutRows）与本文件同处，避免文件碎片化。

/** 行间距（ItemGrid .row 的 gap 与行推进共用） */
export const GRID_GAP = 10;

/** 卡片 meta 区定高（Eagle 式 3 行：标题 2 + 像素 1）：行距按它计算，ItemCard 的 .meta 经 --meta-h 消费 */
export const CARD_META_H = 54;

/** 卡片总边框宽（2px × 2）：行槽位必须计入，否则下一行图片盖住上一行的 meta 文字；ItemCard 经 --card-border 消费 */
export const CARD_BORDER = 4;
