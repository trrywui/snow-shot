# Snow Shot 截图交互 CPU 优化验证

2026-10-16；本机 Intel macOS 14.8.9、Qt 6.12.0、Release；1680×1050 逻辑分辨率，2× 缩放。原版应用及其配置未替换，实测使用隔离数据目录的测试副本，全局快捷键禁用。

本轮落实三项改动：缓存放大镜固定阴影和背景；Cocoa 下使用原生窗口透明度；macOS 普通桌面截图请求 BGRA，使不透明底图以 Qt RGB32 格式绘制。

| 版本 | 持续移动 CPU 主要区间 | 拖动 CPU 主要区间 |
|---|---:|---:|
| 优化前 | 约 38%–63% | 约 85%–108% |
| 阴影缓存＋原生透明度 | 约 37%–46% | 约 56%–79% |
| 再加 BGRA 底图 | 约 32%–48% | 约 52%–94% |

完整版本静止后约 0%–0.3%。这些数据来自人工复现并包含 sample 采样开销，鼠标速度、选区大小及背景负载未严格固定；不能据此计算精确降幅。BGRA 调整没有显示稳定的总 CPU 进一步下降，不能宣称 40%+ 的交互 CPU 问题已经解决。

最终调用栈未采到 fetchRGBA8888ToARGB32PM 或 QGraphicsOpacityEffect::draw，固定阴影也未成为重复热点。剩余明确热点包括 memmove、画布底图重绘、QBackingStore::beginPaint/flush、QCALayerBackingStore::finalizeBackBuffer/blitBuffer。后续应围绕实际失效区域、画布更新和窗口缓冲复制继续分析；不要再把这些开销归因于已关闭的历史、动画或智能选择。

新增装饰缓存约 0.52 MiB（当前 2× 缩放）；BGRA 输出复用现有采集帧，不新增另一份整屏图像。

验证通过：

- snow-shot-screenshot-color-picker-tests：offscreen 缩放 1、1.25、2，以及原生 Cocoa。
- snow-shot-macos-color-picker-animation-native-tests。
- snow-shot-capture-worker-tests：包括新增的 RGB32 格式、颜色通道和会话销毁后帧生命周期检查。
- snow-shot-direct-capture-frame-tests：颜色、透明度、色彩空间和帧租约。
- 修改文件 clang-format --dry-run --Werror；git diff --check。
- Release 应用构建、隔离副本部署及 codesign --verify --deep --strict。

未运行全量测试。用户两次回复“已复现”，未额外提供外观或流畅度描述。

最终采样原始数据：[CPU](/Users/lee/workspace/code/snow-shot/.tools/macos14-personal/tmp/magnifier-cpu-nxsx4qps/interaction-bgra-cpu.txt)、[符号化调用栈](/Users/lee/workspace/code/snow-shot/.tools/macos14-personal/tmp/magnifier-cpu-nxsx4qps/interaction-bgra-symbolicated.txt)。

前一阶段：[CPU](/Users/lee/workspace/code/snow-shot/.tools/macos14-personal/tmp/magnifier-cpu-nxsx4qps/interaction-cpu.txt)、[符号化调用栈](/Users/lee/workspace/code/snow-shot/.tools/macos14-personal/tmp/magnifier-cpu-nxsx4qps/interaction-symbolicated.txt)。

[完整代码补丁](/Users/lee/workspace/code/snow-shot/.tools/macos14-personal/tmp/magnifier-cpu-nxsx4qps/optimization.patch)。[隔离测试启动脚本](/Users/lee/workspace/code/snow-shot/.tools/macos14-personal/tmp/magnifier-cpu-nxsx4qps/run-test.command) 会保留禁用全局快捷键及独立数据目录的设置；请从测试副本窗口按钮进入截图。
