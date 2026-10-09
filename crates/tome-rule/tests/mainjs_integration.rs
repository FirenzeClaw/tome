//! T044 验收测试：mainJs 纯脚本源执行（FR-005 / 书源生态兼容）
//!
//! ## 这些测试在证什么
//!
//! 纯脚本源（JS 源）的全部抓取逻辑写在脚本里，经四个入口函数与引擎交互。
//! 本文件逐条验证：
//!
//! | 验收点 | 测试 |
//! |---|---|
//! | 四个入口都能调用 | [`四个入口都可调用`] |
//! | 列表类返回值形状校验 | [`search_返回非数组时报错`] |
//! | 缺入口时报可归因错误 | [`缺入口时报指明入口名的错误`] |
//! | 脚本里的全局状态在构造时初始化 | [`脚本顶层代码在构造时执行`] |
//! | 沙箱护栏生效（死循环/内存） | [`死循环的入口被中止`] |
//! | 词法特殊字符不破坏调用 | [`关键词含引号不破坏调用`] |
//!
//! ## 为什么用"真实脚本"而不是 mock 掉沙箱
//!
//! 本模块的全部价值在于"把书源的脚本跑起来"。若测试把沙箱换掉，那验证
//! 的就只是字符串拼接——而"拼接错了一个转义"恰恰是最容易出问题的地方。
//! 故这里跑**真实的 JS 脚本**。

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数。
#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::time::Duration;

use tome_js::{JsContext, Limits};
use tome_rule::mainjs::{MainJsRunner, ScriptAction, ScriptOutcome};

/// 测试用的配额（短超时让死循环测试快速失败）。
fn test_limits() -> Limits {
    Limits {
        wall_clock_timeout: Some(Duration::from_secs(5)),
        ..Limits::default()
    }
}

/// 建一个跑给定脚本的执行器。
///
/// ## 三步的顺序就是被测的顺序
///
/// `mainJs` 的顶层代码会调用宿主函数，而宿主桥必须在**任何**书源脚本
/// 求值之前注入（见 `MainJsRunner::new` 的文档）。故这里按
/// "建 runtime → 注桥 → 载脚本"的顺序装配——那正是组合根应当做的。
fn runner(script: &str) -> MainJsRunner {
    let mut runner = MainJsRunner::new(test_limits()).expect("应能建执行器");
    runner
        .install_bridge(tome_js::HostBridge::bare(JsContext::for_source(
            "https://js.test",
        )))
        .expect("应能注入宿主桥");
    runner.load(script).expect("应能载入脚本");
    runner
}

/// 一份典型纯脚本源的 mainJs。
///
/// ## 它模拟的是什么
///
/// 书源里的 JS 源通常长这样：顶层定义配置与工具函数，四个入口各自
/// 返回数据。这里用**纯计算**的数据（不触网），以便测试只验证
/// "入口约定与返回值校验"这条链路。
const SAMPLE_MAIN_JS: &str = r#"
  // 顶层配置与工具（书源里常见，验证它们在构造时就被求值）
  var BASE = "https://example.test";
  var source = { name: "示例 JS 源" };

  function book(name, author) {
    return { name: name, author: author, bookUrl: BASE + "/book/" + name };
  }

  function search(key, page) {
    return [book(key + "-1", "作者甲"), book(key + "-2", "作者乙")];
  }

  function bookInfo(url) {
    return { name: "斗破苍穹", author: "天蚕土豆", intro: "简介", tocUrl: url };
  }

  function chapterList(url) {
    return [
      { title: "第一章", url: url + "/c1" },
      { title: "第二章", url: url + "/c2" }
    ];
  }

  function chapterContent(url) {
    return "这是正文内容，来自 " + url;
  }
"#;

// ---------------------------------------------------------------------------
// 四个入口
// ---------------------------------------------------------------------------

#[test]
fn 四个入口都可调用() {
    let runner = runner(SAMPLE_MAIN_JS);

    let search = runner
        .run(ScriptAction::Search, "斗破", 1)
        .expect("search 应成功");
    let list = match &search {
        ScriptOutcome::List(value) => value.as_array().expect("应是数组"),
        other => panic!("search 应返回列表，实际 {other:?}"),
    };
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["name"], "斗破-1");
    assert_eq!(list[0]["author"], "作者甲");

    let info = runner
        .run(ScriptAction::BookInfo, "https://x/book/1", 1)
        .expect("bookInfo 应成功");
    assert_eq!(info.as_json().expect("应是 JSON")["name"], "斗破苍穹");

    let toc = runner
        .run(ScriptAction::ChapterList, "https://x/book/1", 1)
        .expect("chapterList 应成功");
    let chapters = match &toc {
        ScriptOutcome::List(value) => value.as_array().expect("应是数组"),
        other => panic!("chapterList 应返回列表，实际 {other:?}"),
    };
    assert_eq!(chapters.len(), 2);
    assert_eq!(chapters[0]["title"], "第一章");

    let content = runner
        .run(ScriptAction::ChapterContent, "https://x/c1", 1)
        .expect("chapterContent 应成功");
    assert_eq!(content.as_text(), Some("这是正文内容，来自 https://x/c1"));
}

#[test]
fn search_收到关键词与页码() {
    // 入口的参数约定：`search(key, page)`。页码传错会让书源的分页失效。
    let script = r#"
      function search(key, page) {
        return [{ name: key, author: "第" + page + "页" }];
      }
    "#;
    let runner = runner(script);
    let outcome = runner
        .run(ScriptAction::Search, "关键词", 7)
        .expect("应成功");
    let list = match &outcome {
        ScriptOutcome::List(value) => value.as_array().expect("应是数组").clone(),
        other => panic!("应返回列表，实际 {other:?}"),
    };
    assert_eq!(list[0]["name"], "关键词");
    assert_eq!(list[0]["author"], "第7页", "页码应作为第二个参数传入");
}

#[test]
fn 脚本顶层代码在构造时执行() {
    // `mainJs` 的顶层是模块级初始化（配置、工具函数定义）。
    // 它必须在**构造时**跑一次——若等到动作调用时才跑，每个动作都会
    // 重新初始化一遍（签名的密钥、缓存的 token 会被重复计算）。
    let script = r#"
      var initCount = 0;
      initCount = initCount + 1;
      function search(key, page) {
        // 若顶层代码在每次动作时都重跑，这里会永远看到 1
        return [{ name: key, author: String(initCount) }];
      }
    "#;
    let runner = runner(script);
    let first = runner.run(ScriptAction::Search, "a", 1).expect("应成功");
    let second = runner.run(ScriptAction::Search, "b", 1).expect("应成功");

    let count_of = |outcome: &ScriptOutcome| -> String {
        match outcome {
            ScriptOutcome::List(value) => {
                value[0]["author"].as_str().unwrap_or_default().to_string()
            }
            other => panic!("应返回列表，实际 {other:?}"),
        }
    };
    assert_eq!(count_of(&first), "1");
    assert_eq!(
        count_of(&second),
        "1",
        "顶层代码不应在每次动作时重跑（否则书源的初始化会被重复执行）"
    );
}

#[test]
fn 运行器可多次调用且状态保留() {
    // 书源会用模块级变量在同一次抓取的不同动作间传值（token、缓存）。
    let script = r#"
      var calls = 0;
      function search(key, page) { calls = calls + 1; return []; }
      function bookInfo(url) { return { name: String(calls) }; }
    "#;
    let runner = runner(script);
    let _ = runner.run(ScriptAction::Search, "a", 1).expect("应成功");
    let _ = runner.run(ScriptAction::Search, "b", 1).expect("应成功");
    let info = runner.run(ScriptAction::BookInfo, "u", 1).expect("应成功");
    assert_eq!(
        info.as_json().expect("应是 JSON")["name"],
        "2",
        "模块级变量应跨动作保留（书源靠它传值）"
    );
}

// ---------------------------------------------------------------------------
// 返回值校验
// ---------------------------------------------------------------------------

#[test]
fn search_返回非数组时报错() {
    // ## 这是本模块最要紧的一条校验
    //
    // 静默接受非数组会让"脚本写错了"表现为"这个源没有结果"——
    // 排查方向被完全带偏（用户会去怀疑站点改版，而真实原因是脚本 bug）。
    let script = r#"
      function search(key, page) { return { wrong: "shape" }; }
    "#;
    let runner = runner(script);
    let error = runner
        .run(ScriptAction::Search, "k", 1)
        .expect_err("非数组应报错");
    let message = format!("{error}");
    assert!(
        message.contains("数组") || message.contains("对象"),
        "错误应说明期望的形状：{message}"
    );
}

#[test]
fn search_返回标量时报错并说明实际类型() {
    let script = r#"
      function search(key, page) { return 42; }
    "#;
    let runner = runner(script);
    let error = runner
        .run(ScriptAction::Search, "k", 1)
        .expect_err("标量应报错");
    let message = format!("{error}");
    assert!(message.contains("数字"), "应说明实际是数字：{message}");
}

#[test]
fn search_返回带_list_的对象被接受() {
    // 兼容：`return {list: [...]}` 是书源里的常见写法
    // （脚本从接口取回后直接返回整个响应对象）。
    let script = r#"
      function search(key, page) { return { list: [{ name: key }], total: 1 }; }
    "#;
    let runner = runner(script);
    let outcome = runner.run(ScriptAction::Search, "k", 1).expect("应成功");
    match outcome {
        ScriptOutcome::List(value) => {
            assert_eq!(value.as_array().expect("应是数组").len(), 1);
        }
        other => panic!("应返回列表，实际 {other:?}"),
    }
}

#[test]
fn search_返回空内容时报错而非空列表() {
    // `undefined` 与"空列表"语义不同：前者是"脚本忘了 return"（bug），
    // 后者是"确实没结果"。静默把前者当后者会让 bug 隐形。
    let script = r#"
      function search(key, page) { /* 忘了 return */ }
    "#;
    let runner = runner(script);
    let error = runner
        .run(ScriptAction::Search, "k", 1)
        .expect_err("无返回值应报错");
    assert!(
        format!("{error}").contains("没有返回"),
        "错误应说明没返回内容：{error}"
    );
}

#[test]
fn 正文返回字符串() {
    let script = r#"
      function chapterContent(url) { return "正文"; }
    "#;
    let runner = runner(script);
    let outcome = runner
        .run(ScriptAction::ChapterContent, "u", 1)
        .expect("应成功");
    assert_eq!(outcome.as_text(), Some("正文"));
}

#[test]
fn 详情返回对象() {
    let script = r#"
      function bookInfo(url) { return { name: "书名", author: "作者" }; }
    "#;
    let runner = runner(script);
    let outcome = runner.run(ScriptAction::BookInfo, "u", 1).expect("应成功");
    assert_eq!(outcome.as_json().expect("应是 JSON")["name"], "书名");
}

#[test]
fn 脚本抛错时报出可读错误() {
    let script = r#"
      function search(key, page) { throw new Error("接口返回了意外结构"); }
    "#;
    let runner = runner(script);
    let error = runner
        .run(ScriptAction::Search, "k", 1)
        .expect_err("脚本抛错应报出");
    let message = format!("{error}");
    assert!(
        message.contains("意外结构"),
        "应带上脚本的错误信息：{message}"
    );
}

// ---------------------------------------------------------------------------
// 缺入口
// ---------------------------------------------------------------------------

#[test]
fn 缺入口时报指明入口名的错误() {
    // 缺入口时引擎给的是 `undefined is not a function`——书源作者看到
    // 那个信息不知道是"我的脚本没实现这个动作"还是"引擎调错了名字"。
    let script = r#"
      function search(key, page) { return []; }
      // 刻意不定义 bookInfo / chapterList / chapterContent
    "#;
    let runner = runner(script);
    // 已定义的入口照常可用
    assert!(runner.run(ScriptAction::Search, "k", 1).is_ok());

    for action in [
        ScriptAction::BookInfo,
        ScriptAction::ChapterList,
        ScriptAction::ChapterContent,
    ] {
        assert!(
            runner.run(action, "u", 1).is_err(),
            "{action:?} 缺入口时应报错"
        );
    }
}

#[test]
fn 缺入口的错误消息指向具体入口名() {
    let script = "var notAFunction = 42;";
    let runner = runner(script);
    let error = runner
        .run(ScriptAction::Search, "k", 1)
        .expect_err("应报错");
    let message = format!("{error}");
    assert!(
        message.contains("search"),
        "应指明缺的是哪个入口：{message}"
    );
    assert!(
        message.contains("入口函数") || message.contains("缺少"),
        "应说明这是缺入口而非其它失败：{message}"
    );
}

#[test]
fn 入口存在但不是函数时报错() {
    // 书源可能把 `search` 定义成一个变量（写错了）。
    let script = r#"
      var search = "im not a function";
    "#;
    let runner = runner(script);
    let error = runner
        .run(ScriptAction::Search, "k", 1)
        .expect_err("非函数应报错");
    let message = format!("{error}");
    assert!(
        message.contains("string") || message.contains("实际类型"),
        "应说明实际类型：{message}"
    );
}

// ---------------------------------------------------------------------------
// 沙箱护栏
// ---------------------------------------------------------------------------

#[test]
fn 死循环的入口被中止() {
    // FR-007 不区分脚本来自规则片段还是 mainJs——两者都要能被中止。
    let script = r#"
      function search(key, page) { while (true) {} }
    "#;
    let mut runner = MainJsRunner::new(Limits {
        wall_clock_timeout: Some(Duration::from_millis(300)),
        ..Limits::default()
    })
    .expect("应能建");
    runner
        .install_bridge(tome_js::HostBridge::bare(JsContext::for_source("s")))
        .expect("应能注入");
    // 载入**应当成功**：死循环在函数体里，定义时不执行。
    runner
        .load(script)
        .expect("定义阶段不执行函数体，载入应成功");

    // 调用时才进入死循环——护栏应在此刻生效。
    let error = runner
        .run(ScriptAction::Search, "k", 1)
        .expect_err("死循环的入口应被中止");
    let message = format!("{error}");
    assert!(
        message.contains("中止") || message.contains("超时") || message.contains("interrupt"),
        "应说明是被中止：{message}"
    );
}

#[test]
fn 沙箱被中止后仍可继续服务后续调用() {
    // 一次动作超时不该让整个源不可用——那与"每个源独立 runtime"的
    // 隔离初衷相悖（一个动作失败拖垮整个源）。
    let script = r#"
      var calls = 0;
      function search(key, page) {
        calls = calls + 1;
        if (calls === 1) { while (true) {} }
        return [{ name: key }];
      }
    "#;
    let mut runner = MainJsRunner::new(Limits {
        wall_clock_timeout: Some(Duration::from_millis(300)),
        ..Limits::default()
    })
    .expect("应能建");
    runner
        .install_bridge(tome_js::HostBridge::bare(JsContext::for_source("s")))
        .expect("应能注入");
    runner.load(script).expect("应能载入");

    let first = runner.run(ScriptAction::Search, "a", 1);
    assert!(first.is_err(), "第一次调用应被中止");
    assert!(
        !runner.is_poisoned(),
        "超时不该废弃沙箱（只有内存耗尽才废弃）"
    );

    let second = runner
        .run(ScriptAction::Search, "b", 1)
        .expect("中止之后应仍可服务后续调用");
    match second {
        ScriptOutcome::List(value) => assert_eq!(value[0]["name"], "b"),
        other => panic!("应返回列表，实际 {other:?}"),
    }
}

#[test]
fn 构造时的死循环也被中止() {
    // 顶层代码里写死循环同样要能停（构造阶段也在沙箱里跑）。
    let script = "while (true) {}";
    let mut runner = MainJsRunner::new(Limits {
        wall_clock_timeout: Some(Duration::from_millis(300)),
        ..Limits::default()
    })
    .expect("应能建");
    runner
        .install_bridge(tome_js::HostBridge::bare(JsContext::for_source("s")))
        .expect("应能注入");
    let outcome = runner.load(script);
    assert!(outcome.is_err(), "载入阶段的死循环应被中止");
}

#[test]
fn 前导脚本在_mainjs_运行时里可用() {
    // 纯脚本源会直接引用 `java` / `source` 等宿主全局。缺了 prelude
    // 会得到 ReferenceError，而那个信息误导作者。
    let script = r#"
      function search(key, page) {
        return [{ name: typeof java, author: typeof Packages }];
      }
    "#;
    let runner = runner(script);
    let outcome = runner.run(ScriptAction::Search, "k", 1).expect("应成功");
    match outcome {
        ScriptOutcome::List(value) => {
            // `java` 是前导脚本放的 Proxy 桩（可点可调用），故 typeof 是 function
            assert_eq!(
                value[0]["name"], "function",
                "`java` 应存在（前导脚本的桩）"
            );
            assert_eq!(value[0]["author"], "function", "`Packages` 桩应存在");
        }
        other => panic!("应返回列表，实际 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 词法特殊字符
// ---------------------------------------------------------------------------

#[test]
fn 关键词含引号不破坏调用() {
    // 用户可能搜 `他说"你好"`——参数拼接的转义必须正确。
    let script = r#"
      function search(key, page) { return [{ name: key }]; }
    "#;
    let runner = runner(script);
    let outcome = runner
        .run(ScriptAction::Search, r#"他说"你好""#, 1)
        .expect("含引号的关键词应成功");
    match outcome {
        ScriptOutcome::List(value) => {
            assert_eq!(value[0]["name"], r#"他说"你好""#);
        }
        other => panic!("应返回列表，实际 {other:?}"),
    }
}

#[test]
fn url_含反斜杠与特殊字符不破坏调用() {
    let script = r#"
      function chapterContent(url) { return url; }
    "#;
    let runner = runner(script);
    for url in [
        "https://x/a?b=c&d=e",
        r"https://x/a\b",
        r#"https://x/q="v""#,
        "https://x/中文/路径",
    ] {
        let outcome = runner
            .run(ScriptAction::ChapterContent, url, 1)
            .unwrap_or_else(|error| panic!("URL `{url}` 应成功：{error}"));
        assert_eq!(outcome.as_text(), Some(url), "URL `{url}` 应原样传回");
    }
}

#[test]
fn 中文关键词完整传递() {
    let script = r#"
      function search(key, page) { return [{ name: key, author: key.length }]; }
    "#;
    let runner = runner(script);
    let outcome = runner
        .run(ScriptAction::Search, "斗破苍穹", 1)
        .expect("应成功");
    match outcome {
        ScriptOutcome::List(value) => {
            assert_eq!(value[0]["name"], "斗破苍穹");
            // JS 的字符串长度按 UTF-16 码元：4 个汉字 = 4
            assert_eq!(value[0]["author"], 4);
        }
        other => panic!("应返回列表，实际 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 配置可诊断
// ---------------------------------------------------------------------------

#[test]
fn 运行器可诊断() {
    let runner = runner(SAMPLE_MAIN_JS);
    let rendered = format!("{runner:?}");
    assert!(rendered.contains("MainJsRunner"), "{rendered}");
}
