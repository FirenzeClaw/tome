//! spike 命令入口（T007-T009 的驱动）。
//!
//! ## 为什么 main 是薄的
//!
//! 大部分验证的判据是**测试断言**（`cargo test` 复跑即结论）；
//! 需要产出**数字**的部分（体积、离屏探针）才走这里。
//!
//! ## ⚠️ 体积对照的一个致命前提（T007 首轮实测踩到并修正）
//!
//! **每个 feature 臂都必须在二进制里被真正引用**，否则 LTO 会把它整条
//! 裁掉，得到与空壳**完全相同**的大小——那个数字测的是"什么都没接入"。
//!
//! 首轮实测正是如此：`empty` 与 `cosmic-text-stack` 两臂同为 290816 字节。
//! 原因是 `main.rs` 只引用了 `size`（恒在）与 `offscreen_probe`
//! （render-stack 才有）——`cosmic-text-stack` 开启时 `word_merge`/`hinting`
//! 虽然被编译进 lib，但**二进制不触碰它们**，链接器据 LTO 全部丢弃。
//!
//! 故此处新增 `stack-touch` 子命令：它按 feature **无条件引用**对应模块的
//! 一个函数，确保符号真的进入二进制。体积脚本以它的构建结果为准。
//! 这是"对照实验必须让被测对象真的在产物里"的一般教训。

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("size-self-check") | None => {
            // 体积测量的口径自检（不经编译，验证算术无误）
            use tome_s2_spike::size::{format_net_increase, net_increase};
            println!(
                "net_increase(1000000, 3500000) = {:?}",
                net_increase(1_000_000, 3_500_000)
            );
            println!("{}", format_net_increase(1_000_000, 3_500_000));
            println!(
                "net_increase(3500000, 1000000) = {:?}  （应为 None）",
                net_increase(3_500_000, 1_000_000)
            );
        }
        Some("stack-status") => {
            // 报告本构建含哪些栈（体积对照的臂标识）
            println!(
                "cosmic-text-stack = {}",
                cfg!(feature = "cosmic-text-stack")
            );
            println!("render-stack      = {}", cfg!(feature = "render-stack"));
        }
        Some("stack-touch") => {
            // **体积对照的锚点**：按 feature 无条件引用对应模块，确保符号
            // 进入二进制。返回一个由各栈参与计算的值，避免被优化成常量。
            let mut touched: u64 = 1;

            #[cfg(feature = "cosmic-text-stack")]
            {
                // 触碰排版栈：让它真的参与运行（否则 LTO 裁掉）
                let (mut font_system, mut buffer) =
                    tome_s2_spike::word_merge::shaped_buffer("体积对照", 200.0);
                let line =
                    tome_s2_spike::word_merge::shape_line_of(&mut font_system, &mut buffer, 0);
                touched += tome_s2_spike::word_merge::word_ranges(&line).len() as u64;
                touched += tome_s2_spike::hinting::measure_rows("体积对照", 200.0).len() as u64;
            }

            #[cfg(feature = "render-stack")]
            {
                // 触碰渲染栈：对齐算术与像素统计都真跑一遍
                touched += u64::from(tome_s2_spike::offscreen_probe::aligned_bytes_per_row(64, 4));
                touched +=
                    tome_s2_spike::offscreen_probe::count_non_black_pixels(&[0u8; 64]) as u64;
            }

            println!("stack-touch ok: {touched}");
        }
        Some("cross-backend") => {
            #[cfg(feature = "render-stack")]
            {
                // T009：跨后端像素比对（容差校准）
                let threshold = 0.6;
                let (comparisons, failures) =
                    tome_s2_spike::offscreen_probe::run_cross_backend_comparison(threshold);
                for failure in &failures {
                    println!("BACKEND-FAIL	{}	{}", failure.0, failure.1);
                }
                for comparison in &comparisons {
                    println!(
                        "CROSS	{} -> {}	{}	通关({:.4}%)	{}",
                        comparison.baseline_backend,
                        comparison.candidate_backend,
                        comparison.candidate_adapter,
                        comparison.report.ratio() * 100.0,
                        comparison.report.summary()
                    );
                }
                println!(
                    "CROSS-SUMMARY	对比 {} 个后端	清单为空的臂 {} 个",
                    comparisons.len(),
                    failures.len()
                );
                // 样本有效性自检：若渲染结果与"纯底色"完全相同，说明混合
                // 路径没生效（或字形层是空的），那次比对就是假绿
                if let Some(report) = comparisons.first() {
                    let stats = tome_s2_spike::offscreen_probe::sample_pixel_stats();
                    println!(
                        "SAMPLE-VALIDITY	非底色像素={}/{}	最大 YIQ 距离={:.1}",
                        stats.0, stats.1, report.report.max_delta
                    );
                }
            }
            #[cfg(not(feature = "render-stack"))]
            {
                eprintln!("需要 --features render-stack");
                std::process::exit(2);
            }
        }
        Some("list-adapters") => {
            #[cfg(feature = "render-stack")]
            {
                for (backend, name) in tome_s2_spike::offscreen_probe::enumerate_adapters() {
                    println!("ADAPTER	{backend}	{name}");
                }
            }
            #[cfg(not(feature = "render-stack"))]
            {
                eprintln!("需要 --features render-stack");
                std::process::exit(2);
            }
        }
        Some("offscreen-probe") => {
            #[cfg(feature = "render-stack")]
            {
                std::process::exit(tome_s2_spike::offscreen_probe::run_probe());
            }
            #[cfg(not(feature = "render-stack"))]
            {
                eprintln!("未启用 render-stack feature；请用 --features render-stack 构建");
                std::process::exit(2);
            }
        }
        Some(other) => {
            eprintln!("未知子命令：{other}");
            eprintln!("可用：size-self-check | stack-status | offscreen-probe");
            std::process::exit(2);
        }
    }
}
