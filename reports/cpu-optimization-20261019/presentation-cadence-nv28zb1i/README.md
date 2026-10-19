# 截图呈现频率实验

基线提交：31600ee7a。候选只增加 macOS 进程环境变量 SNOW_SHOT_CAPTURE_PRESENTATION_FPS（30、45、60），未设置及不支持的值保持屏幕刷新率。两模式均启用 Qt GUI 线程池，禁用旧静态底图实验。

优先比较默认屏幕频率与 30 Hz；需要时再比较 45 Hz 或重复基线。真实输入必须从隔离副本主窗口截图按钮进入。各轮独立启动并分别确认，记录指针位置及左键状态用于分段，不注入输入，不自动读取剪贴板。

--paint-trace 只启用 canvas 的单行 paint_ns 日志，用于检验实际绘制次数。CPU 数据属于相同日志条件下的实测；这些行仍有成本，不直接与以前无日志样本比较。画布 paint 次数不等于显示器实际呈现帧数。

Qt steady_clock 与 Python monotonic / mach_absolute_time 的参考点不同，必须按每轮启动时运行 check-clock 得出的偏移对齐，退出时复核；偏移变化超过 10 ms 则拒绝对齐绘制日志。CPU 由进程累计时间差除以单调时钟区间得出，100% 是一个逻辑核心。纯 hover / drag / idle 区间从指针状态识别，排除阶段转换，不按手工回复时间划分。

本目录中的配置、IPC descriptor、原始指针坐标和测试副本不提交 Git。experiment.patch 在构建和验证完成后更新归档。未跑全量测试，不使用本 release 构建运行 benchmark targets。
