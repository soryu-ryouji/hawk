# 发布流程

hawk 的桌面端发布全部由 CI（[release.yml](../.github/workflows/release.yml)）驱动，双通道分发：**stable**（正式 `v*` Release）与 **nightly**（main 分支滚动预发布）。客户端内置自动更新（见 [hawk-app 设计 → 应用自动更新](frontend/hawk-app.md)），发布即对存量用户可见。

## 触发条件一览

| 事件 | 动作 |
| ---- | ---- |
| push `v*` tag | 构建三平台产物 + sha256 边车，创建**正式 Release** |
| push main 且提交信息以 `feat` / `fix` 开头 | 删除并重建 `nightly` Release（prerelease），滚动覆盖 |
| `workflow_dispatch` 手动触发 | 只构建上传 Artifacts，**不**创建/修改任何 Release |

## 版本号规则

- **唯一来源**：`hawk-app/package.json` 的 `version`（semver，首版 `0.1.0`）。electron-builder 打进 app，`app.getVersion()` 读取，设置面板「更新」分区显示
- **正式版 tag 必须为 `v<version>`**，与 package.json 不一致时 CI 一致性守卫直接失败（防止版本号漂移的包流出）
- **nightly 版本覆写**：CI 构建时经 `--config.extraMetadata.version` 覆写为 `0.0.0-nightly.<sha7>`——只改打进 asar 的 package.json，**不改仓库文件**。作用：元数据不冒充稳定版；nightly 用户切 stable 通道检查时 `0.0.0` 低于任何正式版，可随时切回
- **开发态不追加 dev 后缀**：package.json 始终保持「下一个发布版本」（改后缀发版前要记得改回，易忘）；开发/无 git 构建由 `build-info.json` 的 `sha='dev'` 识别，设置面板显示「开发版」
- **自动更新新旧判定与版本号的关系**：stable 通道比 semver（tag vs `app.getVersion()`）；nightly 通道**不走版本号**，比 Release body 末尾的 `<!-- hawk-nightly-sha: <sha> -->` 注释与本机构建 sha（Release 的 `target_commitish` 是分支名，不可用作比较）

## 发布正式版

```bash
# 1. bump 版本（若当前号已发布过）
cd hawk-app
#    编辑 package.json: "version": "0.2.0"

# 2. 提交。用非 feat/fix 前缀，避免顺带触发一轮 nightly 重建
git commit -am "chore(release): v0.2.0"
git push

# 3. 打附注 tag——tag 注释会成为 Release 的发布说明正文
git tag -a v0.2.0 -m "0.2.0

- 新功能 A
- 修复 B"
git push origin v0.2.0
```

等价捷径：`gh release create v0.2.0 --title v0.2.0 --notes "..."`（在 main HEAD 建 tag + 说明，同样触发 CI）。

**tag 推送后 CI 自动接管**：

1. 一致性守卫：`v0.2.0` ≠ package.json `0.2.0` → 立即失败
2. windows job：web 构建与 cargo 并行 → 打包 `hawk.zip` + `hawk.zip.sha256` 边车（正式版 mx=9 最小体积）→ 创建 Release 并附资产
3. macos job（arm64/x64 双架构 matrix 并行）：各腿产出 `hawk-mac-<arch>.zip` + 边车 → 并行附到 Release

**发布后验证**：

- Release 资产齐全：上述 6 个文件（3 产物 + 3 边车；缺边车则存量客户端无法自动更新到该版本，只能手动下载）
- stable 通道：任一客户端 → 设置 → 更新 → 检查更新，应发现新版本并完成下载/安装
- nightly 客户端：切 stable 通道检查，应能拿到本次发布

**发布之后**：无需立即改版本号——下次开发中决定下个版本时再 bump（package.json 始终代表「下一个发布版本」）。

## nightly（全自动，无需操作）

main 分支出现 `feat` / `fix` 开头的提交即触发（`concurrency` 串行，避免滚动覆盖竞争）：

1. windows job 删除旧 `nightly` Release 与 tag → 重建（name = `Nightly <sha7>`，body = 触发提交信息 + 末尾注入完整 sha 注释，prerelease）→ 上传产物
2. macos 双架构 matrix 两腿并行补充产物

特性与边界：

- **滚动覆盖，无历史**：旧 nightly 即删，要看历史版本用正式 Release；打包用 mx=5 快速出包（正式版才用 mx=9 最小体积）
- prerelease 不出现在 `releases/latest`，stable 通道查询天然隔离
- nightly 通道客户端（含局域网 web 端刷新）自动拿到最新构建
- 版本覆写与 sha 注入见上文「版本号规则」

## 手动构建（不经 CI）

`tools/build.sh` / `tools/install.sh`（以及 `npm run pack`）在本机构建：产物**没有** sha256 边车、不上传 Release，仅本机使用；`build-info.json` 写入本机 git HEAD—— nightly 通道检查时会与本机 sha 比较，无实际意义。自动更新链路（含边车校验）只对 GitHub Release 上的产物生效。

## 常见问题

- **tag 打错/版本号写错**：CI 已失败或产物不对时，删 Release + 删 tag 后修正重推；守卫会重新校验
- **手动 dispatch 的 Artifacts 是什么**：各平台产物的未发布副本（含边车），用于调试打包配置，不会进入任何 Release
- **客户端检查更新失败提示限速**：GitHub API 未认证限速 60 次/时/IP，仅影响极端频繁的手动检查；默认启动后静默检查一次 + 手动按钮的频率远低于限额
