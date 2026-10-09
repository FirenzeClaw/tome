// 兼容前导脚本（tasks.md T035）——见 prelude.rs 的模块文档。
//
// ## 设计约束
//
// 1. **立即执行**，不留全局名字。书源脚本可能遍历全局对象，
//    多出的名字会污染它的视图。
// 2. **只补齐，不覆盖**：若宿主已经注入过同名成员（例如组合根装了
//    真正的 `java` 对象），前导脚本**不得**把它换成桩——那会让
//    正常能力变成"不支持"。
// 3. **抛出的错误带 `capability` 属性**（FR-006）：书源的 catch 据此
//    判断"是本产品做不到"，从而走降级路径而不是重试。
// 4. 遇到**只读**全局（某些环境下 globalThis 的属性不可写）时不能崩：
//    包在 try/catch 里，失败也只是少一层补丁，不该让整个沙箱失败。
(function () {
  'use strict';

  var global = (function () {
    // 取全局对象：`globalThis` 是 ES2020 标准；老引擎退回 `this`
    // （严格模式下模块级的 `this` 是 undefined，故再做一层兜底）。
    if (typeof globalThis !== 'undefined') {
      return globalThis;
    }
    if (typeof self !== 'undefined') {
      return self;
    }
    if (typeof global !== 'undefined') {
      return global;
    }
    return Function('return this')();
  })();

  // ---------------------------------------------------------------------
  // 1. 结构化能力错误
  // ---------------------------------------------------------------------
  //
  // 所有"明确不支持"的能力都经这里构造异常。三个属性都要有：
  // 不同的书源用不同的判据写降级逻辑，只给一个会让一部分源认不出来。
  function unsupportedCapability(capability, what) {
    var error = new Error(
      '不支持的能力：' + capability + (what ? '（' + what + '）' : '')
    );
    error.name = 'UnsupportedCapabilityError';
    error.capability = capability;
    error.isUnsupportedCapability = true;
    return error;
  }

  function throwUnsupported(capability, what) {
    throw unsupportedCapability(capability, what);
  }

  // 供宿主（Rust 侧）复用：某些能力是 Rust 侧注册的，
  // 它们也需要抛同一形状的错误。
  global.__tomeThrowUnsupported = throwUnsupported;
  global.__tomeUnsupportedCapability = unsupportedCapability;

  // ---------------------------------------------------------------------
  // 2. String 别名：trimLeft/trimRight → trimStart/trimEnd
  // ---------------------------------------------------------------------
  //
  // Rhino 与老引擎提供 `trimLeft`/`trimRight`（非标准），而 ES5+ 用
  // `trimStart`/`trimEnd`。语义完全一致，故补别名。
  //
  // 只在**缺失**时补：若引擎已提供（未来的 QuickJS 版本可能加上），
  // 覆盖会让一个原生实现变成我们的包装，没有好处。
  (function patchStringTrim() {
    var proto = String.prototype;
    if (typeof proto.trimLeft !== 'function') {
      proto.trimLeft = function () {
        return this.replace(/^[\s\uFEFF\xA0]+/, '');
      };
    }
    if (typeof proto.trimRight !== 'function') {
      proto.trimRight = function () {
        return this.replace(/[\s\uFEFF\xA0]+$/, '');
      };
    }
    // `trimLeft`/`trimRight` 的原生实现（若有）与 `trimStart`/`trimEnd`
    // 是同一个函数，故也补上反向别名——书源两个方向的写法都见过。
    if (typeof proto.trimStart !== 'function' && typeof proto.trimLeft === 'function') {
      proto.trimStart = proto.trimLeft;
    }
    if (typeof proto.trimEnd !== 'function' && typeof proto.trimRight === 'function') {
      proto.trimEnd = proto.trimRight;
    }
  })();

  // ---------------------------------------------------------------------
  // 3. Java / Packages：没有 JVM，抛能力错误
  // ---------------------------------------------------------------------
  //
  // Rhino 提供 `Java.type("java.lang.String")` 与 `Packages.java.util.*`
  // 访问 JVM 类。本项目是原生实现，没有 JVM。这类书源需要 JVM 才能工作，
  // 属"明确不支持的能力"——故抛结构化错误，而不是让脚本撞上
  // `TypeError: Cannot read property 'type' of undefined`（那个错误
  // 会让作者以为是自己的语法问题）。
  //
  // ## 为什么用 Proxy 而不是普通对象
  //
  // 书源可以写 `Packages.java.util.HashMap`、`Packages.org.jsoup.Jsoup`
  // 等任意深度的路径。普通对象只能预置已知的几个键，其余返回 undefined
  // 并导致 `undefined.xxx` 的 TypeError。`Proxy` 的 `get` 捕获任意键，
  // 每次访问都返回"同样的桩"——代码可以一直点下去，直到真正**调用**它时
  // 才抛能力错误。
  //
  // 为什么延迟到调用：书源里常见 `var Jsoup = Packages.org.jsoup.Jsoup;`
  // 这样的"取引用但这次不调用"的写法。在取值时就抛会让整段脚本失败，
  // 而它本可以在不调用该分支的情况下正常工作。
  function makeUnsupportedNamespace(capability, displayName) {
    var target = function () {
      throwUnsupported(capability, displayName + '（原生实现无 JVM）');
    };
    target.toString = function () {
      return '[unsupported: ' + displayName + ']';
    };

    if (typeof Proxy !== 'function') {
      // 极老的引擎没有 Proxy：退回普通对象，至少已知的几个路径可用。
      return target;
    }
    return new Proxy(target, {
      get: function (receiver, property) {
        // ## 为什么只有这几个属性落回真实 target
        //
        // 书源会写 `Packages.a.b.c.d` 一直点下去，故**默认**必须返回桩。
        // 但有几个属性若也返回桩，会让桩自身无法被正常使用与调试：
        //
        // - `Symbol.toPrimitive` / `toString` / `valueOf`：JS 在做字符串
        //   拼接与比较时会自动取它们。若返回桩函数，`"" + Packages` 这类
        //   无害操作也会抛错。
        // - `then`：库代码会用它判断"是不是 Promise"。返回桩会让
        //   `Promise.resolve(x)` 误当作 thenable 而去调用它。
        // - `constructor` / `name` / `length`：反射与调试常用。
        //
        // **其余一切属性**（含 `valueOf` 之外的所有方法名）返回桩——
        // 这正是"任意深度路径都能点到、且调用时抛能力错误"的实现方式。
        //
        // ## `[UNCERTAIN]` 一个已知的边界
        //
        // 因为 JS 里函数天然继承 `Function.prototype`，`Packages.x.call(...)`、
        // `.apply(...)`、`.bind(...)`、`.valueOf()` 等**原型上的方法**也会
        // 落回真实实现而不抛。我们无法区分"书源真的在调 JS 原型方法"与
        // "书源以为在调 JVM 方法"，因为两者语法完全相同。
        // 取"不抛"是更安全的一侧：这些调用不会产生错误的结果，
        // 只是静默返回一个 JS 语义的产物（例如 `valueOf()` 返回函数本身）。
        // 书源若依赖它做实际工作，后续的取值会失败——那是可归因的。
        if (
          property === 'then' ||
          property === 'toString' ||
          property === 'valueOf' ||
          property === 'toJSON' ||
          property === 'inspect' ||
          property === 'constructor' ||
          property === Symbol.toPrimitive ||
          property === Symbol.toStringTag ||
          property === 'name' ||
          property === 'length'
        ) {
          return target[property];
        }
        // 其余情况返回同一个 namespace（可无限点下去）
        return makeUnsupportedNamespace(
          capability,
          displayName + '.' + String(property)
        );
      },
      apply: function () {
        throwUnsupported(capability, displayName);
      },
      construct: function () {
        throwUnsupported(capability, displayName);
      },
    });
  }

  (function patchJava() {
    // 只在缺失时补（见设计约束 2）
    if (typeof global.Java === 'undefined') {
      global.Java = {
        type: function (name) {
          throwUnsupported('javaClass', String(name));
        },
        // Rhino 的 `Java.extend` / `Java.to` 等同理
        extend: function () {
          throwUnsupported('javaClass', 'Java.extend');
        },
        to: function () {
          throwUnsupported('javaClass', 'Java.to');
        },
        from: function () {
          throwUnsupported('javaClass', 'Java.from');
        },
      };
    }
    if (typeof global.Packages === 'undefined') {
      global.Packages = makeUnsupportedNamespace('javaClass', 'Packages');
    }
    if (typeof global.java === 'undefined') {
      // `java` 是宿主运行时对象（`java.ajax` 等）——它应由组合根注入
      // **真实实现**。走到这里说明没注入，故给一个会抛能力错误的桩：
      // 书源的 try/catch 能捕获它并降级，而不是 ReferenceError。
      global.java = makeUnsupportedNamespace('hostRuntime', 'java');
    }
  })();

  // ---------------------------------------------------------------------
  // 4. source / book / chapter：上下文对象
  // ---------------------------------------------------------------------
  //
  // 书源脚本引用 `source.bookSourceName`、`book.name`、`chapter.title` 等。
  // 真实值由组合根在 eval 前注入（T036-T041 的宿主状态类）。
  // 此处只保证**引用不缺**：缺失时给空对象而不是 undefined，
  // 让 `source.x` 得到 undefined 而不抛 TypeError。
  //
  // ## 为什么给空对象而不是能力错误
  //
  // 上下文缺失**不是**能力问题——引擎当然支持上下文，只是这次没提供
  // （例如在非抓取场景下跑脚本）。抛能力错误会让书源执行无意义的降级。
  (function patchContexts() {
    var names = ['source', 'book', 'chapter', 'cookie', 'cache'];
    for (var i = 0; i < names.length; i++) {
      if (typeof global[names[i]] === 'undefined') {
        try {
          global[names[i]] = {};
        } catch (ignored) {
          // 只读全局：跳过。少一层补丁好过让整个沙箱失败。
        }
      }
    }
  })();

  // ---------------------------------------------------------------------
  // 5. 杂项别名
  // ---------------------------------------------------------------------
  (function patchMisc() {
    // `Array.prototype.contains`（老引擎的非标准写法）→ `includes`
    var arrayProto = Array.prototype;
    if (typeof arrayProto.contains !== 'function' && typeof arrayProto.includes === 'function') {
      arrayProto.contains = arrayProto.includes;
    }

    // `String.prototype.startsWith` 等在 QuickJS 上原生可用，无需补。

    // `console` 的最小实现：书源用 `console.log` 调试很常见，
    // 缺失会让整段脚本 ReferenceError。真实实现（把日志接到宿主）由
    // 组合根覆盖；此处只想"不缺引用"。
    if (typeof global.console === 'undefined') {
      var noop = function () {};
      global.console = {
        log: noop,
        debug: noop,
        info: noop,
        warn: noop,
        error: noop,
      };
    }
  })();
})();
