# Snow Shot 截图渲染实验记录

日期：2026-10-16。范围：前两次优化后的 macOS 截图交互，未修改系统安装版或正式配置。

## 本轮结论

未新增能够稳定降低 CPU 的运行时优化。两条“不透明绘制”实验均撤回，保留默认关闭的耗时日志和分析工具。此前的放大镜装饰缓存、原生透明度和 BGRA 截图帧两次优化保留。

本轮测试程序是实验产物，已退出；deploy/snow_shot.app 中仍是撤回前的 canvas-only 实验二进制，不代表最终源码，不应作为日常版本安装。

## 环境与方法

本机 Intel Mac，macOS 14.8.9，静态 Qt 6.12.0，x86_64 Release -O3；显示器逻辑分辨率 1680×1050，DPR 2，60 Hz。使用现有个人构建环境；没有构建 benchmark target。

1. 手动基线：静止、移动鼠标、拖动选区、静止；只采 Qt backing-store 日志，不把带日志的 CPU 作为性能结论。
2. 固定输入：通过测试副本自身的本地 MCP 接口，固定轨迹移动和调整已建立的矩形选区，目标每秒 30 次、每阶段 10 秒。每次新启进程，按基线/实验/实验/基线/基线/实验顺序运行，共六次。
3. CPU 单独采集，关闭详细日志、不运行 sample、不同时编译；由进程累计 CPU 时间增量除以阶段实际时长得到。100% 表示一个逻辑核心。WindowServer 是整台机器共享的进程，其数据可能包含其它窗口的影响。
4. 这是固定频率的选区 API 操作，包含编辑状态更新等接口成本，不能把绝对 CPU 数值与之前的手动鼠标数据或 Snipaste 直接比较。RSS 也不是活动监视器的完整内存占用口径。

## 关闭日志的 CPU 结果

表内为阶段 CPU 中位数。

| 场景 | 基线 | 画布不透明提示实验 | 有效样本数（基线/实验） |
| --- | ---: | ---: | ---: |
| 移动选区：应用 CPU | 43.89% | 44.17% | 3/3 |
| 调整选区：应用 CPU | 43.89% | 43.28% | 2/3 |
| 移动选区：WindowServer CPU | 40.29% | 39.58% | 3/3 |
| 调整选区：WindowServer CPU | 41.59% | 38.78% | 2/3 |

移动阶段各次实际完成 299–300 次更新。调整阶段有一轮基线只完成 262 次，其应用 CPU 为 39.29%、WindowServer 为 53.29%，负载不同，未计入上表；其余各次均完成 300 次。原始数据没有删除。

有效调整阶段应用 CPU：基线 43.58%、44.19%；实验 46.57%、43.28%、43.19%。波动与样本数限制使得 0.61 个百分点的中位数差不能作为稳定改善。移动阶段更没有下降，因此撤回实验。

操作停止后 settle 阶段应用 CPU 为 0–0.50%。本轮 RSS 在约 202–211 MiB 间变化，未证明实验具有内存收益。

原始数据：controlled-cpu-runs.json；各次 stdout/stderr：controlled-*-cpu-*.log。

## 区域与耗时记录（与 CPU 分开）

手动基线截图窗口为 1680×1051，额外 1 个逻辑像素用于原有顶部输入边界。502 次 paint 的平均重绘面积约为完整窗口的 3.99%；累计实际绘制 141,433,020 个物理像素。470 次缓冲复制累计 757,655,040 个物理像素，相当于约 3.03 GB 的目标写入，约为绘制像素量的 5.36 倍。

这些是整段复现的累计量，不是每秒带宽；目标写入不包括读源缓冲等全部内存流量，也不能直接换算成 CPU 贡献比例。

固定输入的单轮详细日志（移动、调整各 5 秒）：

| 指标 | 基线 | 画布不透明提示实验 |
| --- | ---: | ---: |
| 完成选区更新 | 300 | 299 |
| 重绘物理像素 | 60,940,688 | 58,568,904 |
| Qt 清屏目标写入 | 215,875,184 B | 207,142,688 B |
| 缓冲复制目标写入 | 197,143,040 B | 168,134,272 B |
| canvas paint 中位数 | 1.338 ms | 1.309 ms |
| QWidget UpdateRequest 中位数 | 3.757 ms | 3.595 ms |

该单轮有详细日志开销、总更新数略有不同；部分复制量和中位耗时变小，不代表整体 CPU 一定下降。paint 和 UpdateRequest 相互包含，不能相加；二者都是墙钟耗时，不是呈现延迟或进程 CPU 时间。实验 paint 最大值 45.80 ms、UpdateRequest 最大值 48.27 ms，也没有证明尾延迟改善。

## 已撤回的两种实验

- 整个原生窗口改为不透明：能去掉窗口层的自动清屏，但记录中的缓冲复制约从 454 MB 增至 1,057 MB；输入处理数量和节奏也不等价，未得到可采信的稳定 CPU 收益。原生 alpha、顶部透明边界和多显示器边缘还需要额外兼容处理，因此不保留。补丁和早期数据在 full-window-experiment/。
- 只将完整覆盖背景的 canvas 标记为 WA_OpaquePaintEvent：保留原生透明窗口、顶部边界和 native window ID，具备透明图片、缩放空白、滚动截图等回退测试；但没有稳定 CPU 收益，仍撤回。补丁在 canvas-only-experiment/experiment.patch。

Qt 6.12.0 本地源码 qcocoabackingstore.mm 表明：beginPaint 在窗口带 alpha 时清理更新区域；finalizeBackBuffer 在交换缓冲前同步未更新的脏区域。子控件的不透明标记不会直接消除这两项原生窗口操作。

## 最终保留内容

- scripts/analyze-macos-render-trace.py：归一化 QRegion 统计，避免重复统计 bounding rect，区分逻辑/物理像素与写入字节；新增 paint/update 耗时汇总和分数 DPR 估算标识。
- scripts/test-macos-render-trace.py：只针对分析器的相关回归测试。
- canvas/window 两个日志类别：默认关闭，显式启用后记录 paint_ns 和 update_ns；Release 使用 qCInfo，避免 qCDebug 被编译选项移除。

示例环境变量：QT_LOGGING_RULES='qt.qpa.backingstore.debug=true;qt.qpa.backingstore.info=true;snow_shot.capture.render.*.info=true'。

分析：python3 scripts/analyze-macos-render-trace.py /path/to/trace.log --dpr 2。仅对同一显示比例的记录使用；多 DPR 应分开记录。CPU 测试时关闭上述日志。

## 后续优先级

下一个值得独立验证的方向是减少截图静态背景与动态选区共同参与的窗口缓冲维护，例如拆分静态底图与动态覆盖层。需要验证实际鼠标输入下的应用 CPU、WindowServer、拖动帧率和多显示器边界，不能仅用重绘面积下降判断效果。

45/30 fps 可以作为可选省电模式另做对比，须明确评估交互流畅度；本轮没有降低默认帧率。原生/GPU 呈现属于更大的改造，目前没有实现或收益承诺。

最终源码验证：

- 相关 C++ 测试目标重新编译、链接成功；链接器仍有现有 Qt available_externally 警告，无构建错误。
- 只运行 background-coverage、rectangular-selection-rendering、scrolling-overlay、overlay-pool-prewarm 四项 CTest，4/4 通过。
- 日志分析器回归测试 5/5 通过。
- 两个修改的 C++ 文件 clang-format --dry-run --Werror 通过；git diff --check 通过。
- 未运行全套测试。

最终分支：codex/screenshot-rendering-cpu。本轮两个本地提交：6b7d5870b（区域日志分析）、f51d700c5（默认关闭的耗时记录及实验文档）。未推送；工作区干净。此前 59281a0ad、794cf291f 两次有效优化保留。

最终源码没有 SNOW_SHOT_CAPTURE_OPAQUE 开关或不透明绘制实验逻辑。当前 deploy 下的应用是已归档实验产物，不是最终源码的完整应用重建；本轮最终源码只重建了受影响的相关测试目标。实验应用均已退出。

本轮隔离目录占用约 169 MiB，磁盘可用约 23 GiB；未清理其它用户文件。
