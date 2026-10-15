# Snow Shot

由 **trrywui** 维护的 Snow Shot Intel macOS 兼容分支，基于
[mg-chao/snow-apps](https://github.com/mg-chao/snow-apps)，保留截图、标注、贴图及完整版本的其他功能。
本分支独立发布，不是上游官方发行版。

## 下载和安装

从 [GitHub Releases](https://github.com/trrywui/snow-shot/releases) 下载 DMG，打开后将
**Snow Shot.app** 拖到“应用程序”。首次启动按提示授予屏幕录制、辅助功能等所需权限。

- 支持 **Intel / x86_64，macOS 14.4 及以上**。
- 当前版本：**1.1.0+up.1.3.0**；上游基线为 `8982d6660c03`（Snow Apps 1.3.0）。
- Bundle ID：`io.github.trrywui.snowshot`。
- Apple Silicon 不在当前发布包的支持范围内；不声明其他机器已经过交互验证。
- 使用 ad-hoc 签名，未进行 Apple 公证；重建或升级后可能需要重新授予权限。

应用使用独立配置和历史，不自动迁移旧个人兼容版或官方版数据。
首次从旧个人兼容版或官方版切换时，由用户自行处理旧版。维护版之间升级可正常退出后替换应用，无需清理现有配置和历史。
构建脚本不会安装、卸载应用，也不会清理已安装应用的数据。

## 更新与默认行为

采用手动更新，请从本仓库 Release 下载新版。应用不会查询上游安装包，避免安装要求更高系统版本的官方包。

- 保留手动 OCR、二维码识别，默认关闭自动贴图 OCR 和截图自动二维码识别，可在设置中开启。
- 开机启动恢复上游默认值，可在设置中关闭；是否允许注册取决于安装位置、签名和系统批准。
- 本地 MCP 桥接未编入此发布配置。
- macOS 14 不调用 macOS 15 新增的鼠标点击圆圈设置；HDR 仍受系统和硬件限制。

兼容处理没有增加后台轮询、常驻兼容进程或截图转换缓冲区。
与其他截图工具的内存和 CPU 对比需要在相同工作负载下测量，不宣称未经验证的节省比例。

## 构建与维护

参见 [构建说明](docs-macos14-personal.md)。工具、依赖、缓存、测试和产物留在仓库内被 Git 忽略的隔离目录，
不修改全局 Rust 默认工具链、Homebrew 或 shell 配置。开发工具不会随应用常驻。

版本采用 `维护版本+up.上游版本`。每次发布递增前面的维护版本，不能只修改不参与 SemVer 排序的 `+up.…`。
发布元数据统一维护版本、上游基线和 macOS 构建号。

上游自动化流程存档在 `.github/upstream-workflows/`，不会向原作者发布渠道推送；当前本机构建、手动发布。

## 来源与许可证

原作者：**mg-chao**；本分支兼容修改和维护：**trrywui**，2026 年。
原始项目见 [上游仓库](https://github.com/mg-chao/snow-apps/tree/8982d6660c03)。

这是多许可证仓库，范围见 [LICENSE.md](LICENSE.md)：

| 组件 | 许可证 |
| --- | --- |
| Snow Shot、Snow Image、Snow Image Viewer、原始仓库构建编排 | GPL-3.0-or-later |
| Ant Design Qt、Rust crates、绘图引擎、Rust FFI | Apache-2.0 |

第三方材料继续遵循各自许可证，参见 [Snow Shot 第三方声明](snow_shot/THIRD_PARTY_NOTICES.md)
和 [Ant Design Qt 第三方声明](ant_design_qt/THIRD_PARTY_NOTICES.md)。
发布包保留许可证和版权声明，Release 提供与安装包对应的源码材料。
