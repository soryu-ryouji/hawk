# 内容重复扫描

重复图片只按内容 hash 确认：文件内容经 BLAKE3 计算得到 item id，内容完全相同的文件共享同一 item，`paths` 记录全部库内位置。扫描时直接按 item id 分组，不做感知哈希、颜色复核或视觉相似度推断。

## 数据模型

内容相同的文件共享同一个 item：

```text
items.hash  → 内容 BLAKE3
paths.path  → 该内容的全部库内位置
```

因此「重复图片」等价于同一 item 在当前用户可见范围内存在多个非回收站位置。回收站位置不参与扫描。

## 查询契约

`POST /api/v1/item/duplicates`（admin 限定）分页返回位置数大于 1 的 item 组。

### 请求

| 字段 | 类型 | 说明 |
| ---- | ---- | ---- |
| offset | number | 组偏移，默认 0 |
| limit | number | 每组页大小，默认 50、最大 100 |

### 响应

```json
{
  "status": "success",
  "data": {
    "groups": [
      {
        "id": "9b1f2c...",
        "entries": [
          { "item": { "id": "9b1f2c...", "path": "posters/a.png", "name": "a", "ext": "png", "size": 245760, "width": 1920, "height": 1080 } },
          { "item": { "id": "9b1f2c...", "path": "posters/a-copy.png", "name": "a-copy", "ext": "png", "size": 245760, "width": 1920, "height": 1080 } }
        ]
      }
    ],
    "total": 1,
    "offset": 0,
    "limit": 50
  }
}
```

- `total` 为分页前总组数；组按成员数降序、item id 升序稳定排序。
- 仅返回当前锁守卫可见的非回收站位置；被锁定内容不会进入响应。
- 不返回相似度或差异值。

## 设置面板

「设置 → 存储 → 重复图片」打开独立弹窗，提供单一扫描：

- 扫描后分页展示组、缩略图、文件大小、像素与完整路径；缩略图未就绪时显示占位并自动重试。
- 默认不选中；删除按钮固定在弹窗顶部，跨页按 `id + path` 保留选择。
- 用户经 ConfirmDialog 二次确认后，逐位置调用 `item/delete` 移入回收站。
- 部分失败保留失败项选中并显示成功/失败计数。

## 验证

- 同一内容的多位置归入同一组，不同内容不合并。
- 锁过滤与分页顺序稳定。
- 前端默认不选中、跨页选择、逐位置删除与部分失败重试。
