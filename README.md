# hawk

对标 Eagle 的开源图片素材管理工具

- 非侵入式资源管理：资源以文件夹的形式进行管理，非侵入式
- 自由开放：开放的 REST API，方便生态接入
- 免费

## 核心特性

### 非侵入式资源管理

hawk 不会将你的素材"导入"到某个专有仓库中。你的文件始终保存在原来的文件夹里，素材目录中也不会出现任何 hawk 的文件——所有数据都收敛在一个 `.hawk/` 隐藏文件夹中。

- 卸载 hawk 后，你的素材文件纹丝不动
- 原有的文件夹组织习惯完全保留
- 与网盘（Dropbox、iCloud、Syncthing、OneDrive）天然兼容

### 纯文本元数据存储

素材参数（标签、评分、备注等）以独立的纯文本文件存放在 `.hawk/metadata/` 中。网盘同步冲突只影响单个素材，可以用 Git 管理素材库，没有数据锁定。

### 前后端解耦

后端是独立的 C# 服务，前端只通过 REST API 通信。桌面版用 Electron 壳拉起后端进程；同一套后端未来可直接部署为多人使用的服务器版本。

### 开放 REST API

```text
# 搜索素材
POST http://localhost:27371/api/v1/item/list
{ "keywords": ["logo"], "tags": ["品牌"], "star": 5 }

# 获取缩略图
GET http://localhost:27371/api/v1/item/thumbnail?id=abc123&size=256

# 更新标签
POST http://localhost:27371/api/v1/item/update
{ "id": "abc123", "tags": ["待审核"] }
```

## 文档

- [架构设计](docs/architecture.md)：进程模型、桌面/服务器部署形态、仓库结构
- [技术栈](docs/tech-stack.md)：语言与框架选型
- [hawk-server（C# 过渡实现）](docs/server-csharp.md)：第一版后端实现细节
- [hawk-server 代码导读](docs/server-code-structure.md)：逐文件职责与关键流程串联
- [hawk-app 设计](docs/hawk-app.md)：Electron 壳 + Vue 前端的界面与接入设计
- [REST API V1](docs/server-rest-api-v1.md)：接口定义
- [存储设计](docs/storage.md)：`.hawk/` 目录结构、同步边界、索引与缓存
