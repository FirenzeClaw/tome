//! spike 命令入口（T006-T009 的驱动）。
//!
//! 每个子命令对应一个 R5 验证项，输出**机器可读**的一行行数据，
//! 便于直接粘贴进 `BASELINE.md`——避免"手抄数字时抄错"这类不必要失误。

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("word-merge") | None => {
            println!("word_merge 的验证以 `cargo test` 运行（判据即测试断言）：");
            println!("  cargo test -p tome-s2-spike word_merge");
        }
        Some(other) => {
            eprintln!("未知子命令：{other}");
            std::process::exit(2);
        }
    }
}
