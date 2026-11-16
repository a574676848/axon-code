# 子进程资源围栏：默认值与平台差异

本文记录 shell 工具子进程资源围栏的取值原则、平台落点与已知限制。用户可见的配置入口（环境变量与关闭方式）见 [../configuration.md](../configuration.md)。

---

## 1. 默认值的选取原则

默认值的选取原则是「宽到不影响正常构建，紧到能阻断失控写入与进程爆炸」：`RLIMIT_FSIZE` 远高于单次构建产物量级；`RLIMIT_AS` 默认不限制，因为 rustc/LLVM 在个别 crate 上会用到数 GiB 地址空间，设限会误伤正常构建。PTY 交互式会话（`tty: true`）使用单独一组更宽的限额（`FSIZE` 8 GiB、`NPROC` 16384、`NOFILE` 8192），因为它会自建并行作业编排。

平台差异：

| 维度 | Unix | Windows |
| --- | --- | --- |
| 文件大小 | `RLIMIT_FSIZE`，超限进程被 `SIGXFSZ` 终止 | 无等价物，只能依赖 artifact 上限与静态分析 |
| 进程数 | `RLIMIT_NPROC`（对 root 不生效） | `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` |
| 打开文件数 | `RLIMIT_NOFILE` | 无等价物 |
| 地址空间 | `RLIMIT_AS` | `JOB_OBJECT_LIMIT_PROCESS_MEMORY` |

Windows 侧的 PTY（ConPTY）会话不做围栏：`portable_pty` 不返回子进程句柄，无法绑定 Job Object。此外 Windows 的 Job Object 绑定发生在进程启动之后，子进程在该窗口内派生的孙进程不受限额约束。

`setrlimit` 只能**降低**硬限制，因此当容器或宿主已经给了更低的硬限制时，Axon 静默取两者的较小值而不是报错。

---

## 相关文档

- [../configuration.md](../configuration.md) - 环境变量与关闭方式
