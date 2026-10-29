# 上游 1.4.3 完整合并验证

日期：2026-10-29。主机：Intel x86_64，macOS 14.8.9，Qt 6.12.0。

## 范围

- 合并前维护分支：`e344bc73c`，上游基线为 `59782f9bb2ef`（1.3.2）。
- 完整合并稳定标签 `v1.4.3_snow-shot`，目标提交为
  `17fad782a3f94f8baa5ebcff6474d2b1c76a0822`，包含其间全部 130 个上游提交。
- 维护版元数据：`1.1.3+up.1.4.3`，macOS 构建号 `5`。
- 保留 Intel / macOS 14.4 兼容、独立身份、数据隔离和手动更新策略。
- OCR 子模块使用上游锁定的 `fa4283a64b26e8839caa358b2df157ca2f8a7ac9`。

## 低占用策略

- 保留 macOS 截图交互默认 30 Hz 上限和放大镜静态装饰缓存。
- 自动贴图 OCR、自动二维码识别和模型热启动继续默认关闭。
- OCR 常驻沿用上游 1.4.3 完整版的可配置默认值；用户可在 App 设置中关闭，
  无需为该选项增加维护版默认值补丁。回归检查覆盖关闭偏好的保存和重新加载。
- 截图准备阶段遇到显示配置变化后取消请求的逻辑沿用上游；本次合并不宣称
  已修复长时间闲置后第一次快捷键无法显示截图界面的问题。

## 已完成的检查

- 维护版构建脚本：4 项通过。
- 远程 macOS 发布脚本的本地契约检查：20 项通过，未触发远程发布。
- macOS minizip / FFmpeg 构建支持检查：5 项通过。
- 维护版依赖源码收集：6 项通过。
- Intel OCR 包架构、最低系统版本和动态依赖审计契约：2 项通过。
- 维护版身份及上游更新隔离的本机测试：2 项通过。
- `snow-macos` 的 `availability::`、`capabilities::`：各 2 项通过。
- `cargo clippy -p snow-macos --lib -- -D warnings` 通过。
- 应用级翻译提取、全部翻译目录一致性检查、CMake 预设解析和名称唯一性检查通过。
- 已通过的 14 项 CTest：截图工作流、维护版配置及数据隔离、旧快捷键迁移、
  内存优化控制器、macOS 原生内存回收、关于页面、截图导出产物、原生贴图窗口、
  取色器、选区呈现 / 30 Hz 调度、导出对话框、最终应用原生启动、
  贴图辅助窗口所有权、截图覆盖层初始化。
  取色器另通过 `QT_SCALE_FACTOR=2` 的重复验证。
- 相对上游保留差异涉及的 27 个 C++ / Objective-C++ 源文件与头文件，
  `clang-format --dry-run --Werror` 通过。

## Release 离屏处理成本

使用上述 performance / Release 构建，`offscreen` 平台，DPR 1，逻辑尺寸
1680 × 1050；每个场景运行一次，预热 1 帧，测量 60 帧。采样期间没有并行构建。
以下为 `total_ms`（提交、显式呈现与 Qt 事件处理 / 绘制）的每帧耗时。

| 场景 | 平均 ms | P95 ms | 原始数据 |
| --- | ---: | ---: | --- |
| 带放大镜框选 | 1.068 | 1.360 | [JSON](benchmark-manual_marquee_magnifier_1.json) |
| 放大镜移动 | 0.299 | 0.411 | [JSON](benchmark-magnifier_visible_1.json) |
| 选区缩放 | 0.940 | 1.327 | [JSON](benchmark-manual_resize_1.json) |

命令参数为 `--offscreen --scenario <name> --iterations 60 --warmup 1 --width 1680
--height 1050 --output <path>`；对应场景名见原始数据文件名。

这是短时、受控路径的成本检查，没有运行旧版对照，不包含真实截图采集、
WindowServer 合成或 input-to-photon 延迟，也没有测量整机 CPU / 应用闲置内存降幅。
30 Hz 限制、结束交互后停止调度和装饰缓存复用由确定性回归验证；
这些结果不能替代安装后的长时间闲置和真实桌面占用对比。

## 合并适配

- 上游已移除取色器的 `QGraphicsOpacityEffect`，并将透明度参数改为可见性参数。
  保留上游实现，更新维护版缓存回归测试；测试继续检查移动时复用装饰、内容更新、
  主题和尺寸变化后的失效重建，以及透明边缘。
- 快捷键配置迁移使用上游实现，去掉维护版已被上游覆盖的重复差异。
- 维护版关于页保留项目和上游归属链接，同时接入上游新增的隐私政策入口。
- 保留维护版数据路径与缓存隔离，并接入上游 Store 发行版分支。
- 为贴图窗口和截图覆盖层测试目标显式导入静态 Qt 的 `offscreen` 插件，
  修复已有离屏 CTest 无法启动的问题；该修正只涉及两个测试目标。

## 构建环境说明

- 使用 `snow-shot-macos14-x64-performance`，`CMAKE_BUILD_TYPE=Release`，静态 Qt，
  `MACOSX_DEPLOYMENT_TARGET=14.4`；重型链接串行执行。
- Rust 业务代码使用 1.97.1；OCR 使用子模块指定的 `nightly-2026-09-08`。
  本地构建驱动通过 rustup proxy 选择工具链，避免 nightly Cargo 误调用 stable rustc。
- SVG 图标生成需要 macOS 图像服务；允许访问该服务后已生成成功。
- 构建存在上游 `winres` 未使用依赖警告和第三方静态库 / LTO 链接警告，
  以及 Qt 私有接口版本约束提示，不将本次构建描述为全局零警告。

应用和指定的截图、贴图目标已完成 Release 构建，原生启动探针通过。
应用 `Info.plist` 保留 `io.github.trrywui.snowshot`、数字版本 `1.1.3`、构建号 `5`，
并与 Mach-O 一致声明最低 macOS 14.4。两个测试目标的 offscreen 插件增量链接完成，
离屏窗口回归及对应 Cocoa 贴图回归均通过。

构建日志和测试输出留在 `.tools/macos14-personal/logs/upstream-1.4.3-*`：
完整应用构建日志为 `upstream-1.4.3-build-main-and-regression-targets.log`，
最后的增量构建日志为 `upstream-1.4.3-build.log`，均成功。
两个窗口目标的最终结果分别在 `upstream-1.4.3-pinned-window-final-tests.log`、
`upstream-1.4.3-overlay-final-tests.log`；初次缺插件的失败日志保留为
`upstream-1.4.3-native-window-before-static-offscreen-import.log`。

本次验证不运行全量测试，不修改 `/Applications` 中的已安装应用及其配置。

## Standards

复核依据为 `AGENTS.md`、`CONTRIBUTING.md`、各语言格式规范以及 code-review
skill 的代码异味基线。没有可用的委派工具，由同一 agent 单独完成标准复核和需求复核。
固定点为 `e344bc73c2a5295a41ba77cb56ba3434f56a364c`，复核命令为
`git diff e344bc73c2a5295a41ba77cb56ba3434f56a364c`（包含暂存和未暂存更改），
并使用 `git diff v1.4.3_snow-shot` 检查维护版集成差异。本次复核重点是合并冲突、
维护补丁与上游新接口的衔接，不是对全部 130 个上游提交重新进行独立源码审计。
复核结果：没有未解决的规范问题。格式、翻译和相关静态检查通过；
测试插件缺失和旧取色器断言均已修正，并以相关回归验证。

## Spec

需求来源为本次对话中的“开始完整合并稳定版，同时保留并验证我的低占用策略”。
实现保留完整上游历史、macOS 14.4 / Intel 兼容和原有低占用补丁；
OCR 常驻采用可关闭的上游默认值，不额外引入维护版默认值差异。
复核结果：没有未解决的需求偏差。完整上游集成、30 Hz 调度、放大镜缓存、
自动识别默认值及关闭 OCR 常驻偏好的持久化均已验证。
实际应用的长时闲置占用和最初快捷键问题仍需安装后复测；本次未安装、推送或发布。

复核汇总：Standards 0 项未解决问题；Spec 0 项未解决问题。
