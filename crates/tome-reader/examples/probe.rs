//! 快照后端探针（T036 / research R4 §5）
//!
//! ## CI 为什么先跑它
//!
//! R4 的诚实模式："CI 测试前跑 probe 探针（小二进制 request_adapter 打印
//! 后端名，失败则 job 直接红）"。先跑的理由是把两类失败**分开**：
//!
//! | 探针结果 | 含义 | 处置 |
//! |---|---|---|
//! | 打印后端名 | 环境能离屏渲染 | 继续跑快照 |
//! | 打印 SKIPPED 并退出 3 | 环境**没有**可用后端 | job 红——那说明矩阵配错了（如 lavapipe 没装） |
//!
//! 若没有这一步，"没有后端"会表现为**所有快照测试都跳过**——而跳过是
//! 合法的（本地无 GPU 的开发机就该跳过）。两者在日志里长得一样，
//! 于是 CI 会"绿着"而其实一张快照都没比。这正是 FR-053 要防的伪装。
//!
//! ## 用法
//!
//! ```bash
//! cargo run -p tome-reader --example probe
//! ```
//!
//! 退出码：`0` = 有后端（stdout 打印 `PROBE OK ...`）；
//! `3` = 无可用后端（打印 `SKIPPED: ...`）——与 001 CLI 的"不支持能力"码一致。

#[cfg(feature = "render")]
fn main() -> std::process::ExitCode {
    use tome_reader::render::pipeline::{PipelineError, RenderPipeline};

    println!("[probe] 渲染面：{}", tome_reader::render::backend_name());

    match RenderPipeline::probe_backend() {
        Ok(backend) => {
            // CI 日志靠这一行判定"跑了哪个后端"（R4 的单基线策略依赖它）
            println!(
                "PROBE OK backend={} adapter={} software={}",
                backend.name(),
                backend.adapter_name(),
                backend.is_software
            );
            // 进一步验证离屏链路真的能建起来（只探到 adapter 不算完整证据）
            match RenderPipeline::new(backend, 64, 64) {
                Ok(pipeline) => {
                    println!(
                        "PROBE OK offscreen device ready {}x{}",
                        pipeline.width(),
                        pipeline.height()
                    );
                    std::process::ExitCode::SUCCESS
                }
                Err(error) => {
                    // 有 adapter 但建不起 device：是**故障**而非缺失能力，
                    // 故不退 3（那会被上层当"可跳过"）
                    println!("PROBE FAIL 建离屏管线失败：{error}");
                    std::process::ExitCode::from(1)
                }
            }
        }
        Err(PipelineError::NoAdapter { reason }) => {
            // 退出码 3 = 不支持能力（001 CLI 的同一约定）
            println!("SKIPPED: 无可用图形后端——{reason}");
            std::process::ExitCode::from(3)
        }
        Err(other) => {
            println!("PROBE FAIL 探测以意外方式失败：{other}");
            std::process::ExitCode::from(1)
        }
    }
}

/// 无 `render` feature 时（headless 构建）探针如何回应。
///
/// ## 为什么不直接编译失败
///
/// headless 构建下**刻意**没有渲染面——那是 `render` 门控的设计（FR-052）。
/// 若探针在这种情况下编译失败，CI 的 headless job 就跑不了它，而那条 job
/// 恰恰需要证明"没有图形栈时本 crate 仍可构建"。故此处给一个明确的
/// "本构建无渲染面"回应 + 退出码 3。
#[cfg(not(feature = "render"))]
fn main() -> std::process::ExitCode {
    println!("SKIPPED: 本构建以 --no-default-features 编译，渲染面不存在（FR-052 的预期形态）");
    std::process::ExitCode::from(3)
}
