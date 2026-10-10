//! R5-3：cosmic-text 全栈体积净增量（T007）
//!
//! ## 测量的方法论
//!
//! 体积**净增量**只能用**对照**测：同一套 release profile 下，编译出
//! "空壳"二进制与"接入某栈"的二进制，两者之差才是接入该栈的代价。
//!
//! 这与 S0 spike 测 rquickjs 的方法同构（`spikes/s0/Cargo.toml` 的
//! `spike-js` feature 注释写明了同样理由）。
//!
//! ## 为什么数字要在**同一 profile** 下取
//!
//! 体积对优化配置极敏感：`lto=fat` + `codegen-units=1` + `strip` 与
//! debug 构建可以差一个数量级。故本 crate 的 `[profile.release]` 逐项
//! 复制了根 `Cargo.toml` 的配置（见其文件头）——复制而非继承是被迫的：
//! 脱离 workspace 后 `[profile]` 不再从父级继承。**若根配置变更，必须
//! 同步改这里**，否则本文档的基线失准。
//!
//! ## 本模块的职责
//!
//! 只提供**口径与算术**（可单测），实际编译与 `stat` 由
//! `scripts/measure-s2-stacks.sh` 执行——因为"编译三种配置"这件事
//! 用 shell 表达比用 Rust 表达更直接，且能在 CI 里单独跑。

/// 一次构建产物的测量记录。
#[derive(Debug, Clone, PartialEq)]
pub struct SizeMeasurement {
    /// 臂名（`empty` / `cosmic-text-stack` / `render-stack` / `full-stack`）
    pub arm: String,
    /// 二进制字节数
    pub bytes: u64,
}

impl SizeMeasurement {
    /// 以字节构造。
    #[must_use]
    pub fn new(arm: impl Into<String>, bytes: u64) -> Self {
        Self {
            arm: arm.into(),
            bytes,
        }
    }

    /// 以 MiB 表示的体积（便于与 20MB 体积基线对照）。
    #[must_use]
    pub fn mib(&self) -> f64 {
        self.bytes as f64 / (1024.0 * 1024.0)
    }
}

/// 两臂之间的净增量。
///
/// ## 为什么返回 `Option`
///
/// 若 `integrated` 比 `base` 还小（例如链接器把某些代码优化掉了，或两次
/// 测量之间基线被改动），"净增量"就是无意义的负数。静默返回负数会让报告里
/// 出现 "-1.2MB 增量"这种没人能解释的数字——故显式 `None` 并让调用方报错。
#[must_use]
pub fn net_increase(bytes_base: u64, bytes_integrated: u64) -> Option<u64> {
    bytes_integrated.checked_sub(bytes_base)
}

/// 格式化净增量（含百分比），供 BASELINE.md 直接粘贴。
#[must_use]
pub fn format_net_increase(bytes_base: u64, bytes_integrated: u64) -> String {
    match net_increase(bytes_base, bytes_integrated) {
        Some(delta) => {
            let mib = delta as f64 / (1024.0 * 1024.0);
            let percent = if bytes_base == 0 {
                f64::INFINITY
            } else {
                delta as f64 / bytes_base as f64 * 100.0
            };
            format!("{delta} 字节（{mib:.2} MiB），相对空壳臂 +{percent:.1}%")
        }
        None => format!(
            "无法计算：接入臂（{bytes_integrated} 字节）小于基线臂（{bytes_base} 字节）\
             ——体积不降反增是不可能的，说明测量本身有问题（基线被改动？链接器异常？）"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_increase_is_plain_subtraction() {
        assert_eq!(net_increase(1_000, 3_500), Some(2_500));
        assert_eq!(net_increase(1_000, 1_000), Some(0));
    }

    /// 接入臂更小时必须报 `None`，而不是负数的"增量"。
    #[test]
    fn net_increase_refuses_negative() {
        assert_eq!(net_increase(3_500, 1_000), None);
        let message = format_net_increase(3_500, 1_000);
        assert!(
            message.contains("无法计算"),
            "符号错误时必须显式报错而非输出负数增量：{message}"
        );
    }

    #[test]
    fn mib_conversion_uses_binary_units() {
        let measurement = SizeMeasurement::new("empty", 1024 * 1024);
        assert!((measurement.mib() - 1.0).abs() < 1e-9);

        let measurement = SizeMeasurement::new("empty", 20 * 1024 * 1024);
        assert!((measurement.mib() - 20.0).abs() < 1e-9);
    }

    #[test]
    fn format_reports_bytes_and_percent() {
        let message = format_net_increase(1_000_000, 3_500_000);
        assert!(message.contains("2500000 字节"), "{message}");
        assert!(message.contains("2.38 MiB"), "{message}");
        assert!(message.contains("+250.0%"), "{message}");
    }
}
